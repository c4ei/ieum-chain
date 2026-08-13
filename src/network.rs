use crate::communication::{CommunicationAck, CommunicationEnvelope};
use crate::consensus::{ConsensusMessage, DoubleVoteEvidence, FinalityCertificate, SignedProposal};
use crate::model::{Block, Transaction};
use crate::peer_guard::{PeerDecision, PeerGuard};
use crate::snapshot_sync::{SnapshotAttestation, SyncTip};
use futures::StreamExt;
use libp2p::core::ConnectedPoint;
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, SwarmBuilder, autonat, dcutr, gossipsub, identify,
    identity::Keypair,
    kad, mdns,
    multiaddr::Protocol,
    noise, ping, relay, request_response,
    swarm::{ConnectionId, NetworkBehaviour, SwarmEvent},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};
use tokio::net::lookup_host;
use tokio::sync::mpsc;

pub const BLOCK_TOPIC: &str = "ieum-chain/blocks/1";
pub const CONSENSUS_TOPIC: &str = "ieum-chain/consensus/1";
pub const SYNC_TOPIC: &str = "ieum-chain/sync/2";
pub const COMMUNICATION_PROTOCOL: &str = "/ieum-chain/communication/1";
// The suffix is one binary framing-version byte, not the four text bytes "\x01".
const COMPRESSED_WIRE_MAGIC: &[u8; 6] = b"IEUMZ\x01";
const COMPRESSION_THRESHOLD_BYTES: usize = 1_024;
const WIRE_HEADER_BYTES: usize = COMPRESSED_WIRE_MAGIC.len() + 4;

#[derive(Clone, Debug, Default)]
struct LocalNetworkView {
    public_ipv4: Vec<Ipv4Addr>,
}

impl LocalNetworkView {
    fn discover() -> Self {
        let mut view = Self::default();
        // connect는 UDP 패킷을 보내지 않고 운영체제 라우팅 표에서 기본 외부 경로에
        // 사용할 로컬 주소만 선택한다. Docker/VMware/에어포트 보조 인터페이스를
        // 기본 경로로 오인하지 않으면서 Linux/Windows/macOS에서 동일하게 동작한다.
        if let Ok(socket) = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            && socket.connect((Ipv4Addr::new(1, 1, 1, 1), 53)).is_ok()
            && let Ok(std::net::SocketAddr::V4(address)) = socket.local_addr()
            && is_public_ipv4(*address.ip())
        {
            view.public_ipv4.push(*address.ip());
        }
        view
    }

    fn accepts_learned_address(&self, address: &Multiaddr, same_lan_peer: bool) -> bool {
        address.iter().all(|protocol| match protocol {
            Protocol::Ip4(ip) => is_public_ipv4(ip) || (same_lan_peer && is_lan_ipv4(ip)),
            Protocol::Ip6(ip) => !ip.is_loopback() && !ip.is_unspecified(),
            _ => true,
        })
    }

    fn log_summary(&self) {
        if self.public_ipv4.is_empty() {
            crate::log_info!(
                "[네트워크 환경] 로컬 NIC에 공인 IPv4가 없습니다. NAT 내부 노드로 판정하며 AutoNAT 역접속 결과로 외부 접근 가능 여부를 확인합니다."
            );
        } else {
            let addresses = self
                .public_ipv4
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            crate::log_info!(
                "[네트워크 환경] 로컬 NIC 공인 IPv4 감지: {addresses} · 방화벽과 UDP 7001 허용 여부는 AutoNAT으로 추가 확인합니다."
            );
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidatorRegistration {
    pub validator_id: String,
    pub peer_id: String,
    pub signature_hex: String,
}

/// 노드 보상 지갑과 영구 PeerId의 소유권을 함께 증명합니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeRewardRegistration {
    pub reward_address: String,
    pub peer_id: String,
    pub signature_hex: String,
    #[serde(default)]
    pub registration_signer: String,
    pub node_public_key_hex: String,
    pub node_signature_hex: String,
}

impl NodeRewardRegistration {
    pub fn registration_signer(&self) -> &str {
        if self.registration_signer.is_empty() {
            &self.reward_address
        } else {
            &self.registration_signer
        }
    }

    pub fn bytes_to_sign(reward_address: &str, peer_id: &str) -> Vec<u8> {
        format!("ieum-node-reward-registration-v1:{reward_address}:{peer_id}").into_bytes()
    }

    pub fn verify_node_identity(&self) -> Result<(), String> {
        let public_key_bytes =
            hex::decode(&self.node_public_key_hex).map_err(|_| "노드 공개키 hex 오류")?;
        let public_key = libp2p::identity::PublicKey::try_decode_protobuf(&public_key_bytes)
            .map_err(|_| "노드 공개키 형식 오류")?;
        let expected_peer_id = PeerId::from(public_key.clone()).to_string();
        if expected_peer_id != self.peer_id {
            return Err("노드 공개키에서 계산한 PeerId가 등록 PeerId와 다릅니다.".into());
        }
        let signature = hex::decode(&self.node_signature_hex).map_err(|_| "노드 서명 hex 오류")?;
        if !public_key.verify(
            &Self::bytes_to_sign(&self.reward_address, &self.peer_id),
            &signature,
        ) {
            return Err("PeerId 개인키 소유권 서명이 올바르지 않습니다.".into());
        }
        Ok(())
    }
}

impl ValidatorRegistration {
    pub fn bytes_to_sign(validator_id: &str, peer_id: &str) -> Vec<u8> {
        format!("ieum-validator-registration-v1:{validator_id}:{peer_id}").into_bytes()
    }
}

/// P2P 실행 시 바꿀 수 있는 네트워크·방어 설정입니다.
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub listen_port: u16,
    /// CI 테스트에서는 호스트의 LAN/Docker 인터페이스를 사용하지 않습니다.
    pub loopback_only: bool,
    pub bootstrap_peers: Vec<Multiaddr>,
    /// NAT 포트포워딩 뒤에서 다른 피어에게 알릴 공개 주소입니다.
    pub external_addresses: Vec<Multiaddr>,
    pub identity_key: Option<Keypair>,
    pub max_message_bytes: usize,
    pub idle_timeout: Duration,
    pub ban_duration: Duration,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_port: 7001,
            loopback_only: false,
            bootstrap_peers: Vec::new(),
            external_addresses: Vec::new(),
            identity_key: None,
            max_message_bytes: 2 * 1024 * 1024,
            idle_timeout: Duration::from_secs(30),
            ban_duration: Duration::from_secs(10 * 60),
        }
    }
}

/// Gossipsub으로 전파하는 메시지의 허용 종류입니다.
/// 합의 메시지는 내부 Ed25519 서명까지 검증한 뒤 상태기계에 전달해야 합니다.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WireMessage {
    Block(Block),
    Transaction(Transaction),
    Proposal(SignedProposal),
    Consensus(ConsensusMessage),
    Evidence(DoubleVoteEvidence),
    ValidatorRegistration(ValidatorRegistration),
    NodeRewardRegistration(NodeRewardRegistration),
    UpdateAvailable {
        version: String,
    },
    SyncRequest {
        requester: String,
        from_height: u64,
    },
    SyncResponse {
        requester: String,
        tip: SyncTip,
        certificates: Vec<FinalityCertificate>,
    },
    SnapshotAttestation(SnapshotAttestation),
}

/// 노드 코어가 비동기 P2P 작업에 보내는 명령입니다.
#[derive(Clone, Debug)]
pub enum NetworkCommand {
    PublishBlock(Block),
    PublishTransaction(Transaction),
    PublishConsensus(ConsensusMessage),
    PublishProposal(SignedProposal),
    PublishEvidence(DoubleVoteEvidence),
    PublishValidatorRegistration(ValidatorRegistration),
    PublishNodeRewardRegistration(NodeRewardRegistration),
    PublishUpdateAvailable {
        version: String,
    },
    RequestSync {
        from_height: u64,
    },
    RespondSync {
        requester: String,
        tip: SyncTip,
        certificates: Vec<FinalityCertificate>,
    },
    PublishSnapshotAttestation(SnapshotAttestation),
    PenalizePeer {
        peer_id: PeerId,
        points: i32,
    },
    Dial(Multiaddr),
    SendCommunication(CommunicationEnvelope),
    Shutdown,
}

/// P2P 작업이 노드 코어에 전달하는 검증 전 이벤트입니다.
#[derive(Debug)]
pub enum NetworkEvent {
    PeerDiscovered(PeerId),
    OutgoingConnectionFailed {
        peer_id: Option<PeerId>,
        connection_id: String,
        error: String,
    },
    PeerConnected {
        peer_id: PeerId,
        remote_address: Multiaddr,
        remote_ip: Option<String>,
        direction: &'static str,
        connection_id: String,
        unique_peers: usize,
        peer_connections: usize,
    },
    PeerDisconnected {
        peer_id: PeerId,
        remote_address: Multiaddr,
        remote_ip: Option<String>,
        direction: &'static str,
        connection_id: String,
        connected_for: Option<Duration>,
        unique_peers: usize,
        peer_connections: usize,
        cause: Option<String>,
    },
    BlockReceived {
        source: PeerId,
        block: Block,
    },
    TransactionReceived {
        source: PeerId,
        transaction: Transaction,
    },
    ConsensusReceived {
        source: PeerId,
        message: ConsensusMessage,
    },
    ProposalReceived {
        source: PeerId,
        proposal: SignedProposal,
    },
    EvidenceReceived {
        source: PeerId,
        evidence: DoubleVoteEvidence,
    },
    ValidatorRegistrationReceived {
        source: PeerId,
        registration: ValidatorRegistration,
    },
    NodeRewardRegistrationReceived {
        source: PeerId,
        registration: NodeRewardRegistration,
    },
    UpdateAvailableReceived {
        source: PeerId,
        version: String,
    },
    SyncRequested {
        source: PeerId,
        requester: String,
        from_height: u64,
    },
    SyncReceived {
        source: PeerId,
        tip: SyncTip,
        certificates: Vec<FinalityCertificate>,
    },
    SnapshotAttestationReceived {
        source: PeerId,
        attestation: SnapshotAttestation,
    },
    CommunicationReceived {
        source: PeerId,
        envelope: CommunicationEnvelope,
    },
}

impl fmt::Display for NetworkEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PeerConnected {
                peer_id,
                remote_address,
                remote_ip,
                direction,
                connection_id,
                unique_peers,
                peer_connections,
            } => write!(
                formatter,
                "[P2P 연결]\n  방향: {direction}\n  PeerId: {peer_id}\n  원격 주소: {remote_address}\n  원격 IP: {}\n  연결 ID: {connection_id}\n  고유 연결 피어: {unique_peers}\n  이 피어의 연결: {peer_connections}",
                remote_ip.as_deref().unwrap_or("확인 불가")
            ),
            Self::PeerDisconnected {
                peer_id,
                remote_address,
                remote_ip,
                direction,
                connection_id,
                connected_for,
                unique_peers,
                peer_connections,
                cause,
            } => write!(
                formatter,
                "[P2P 종료]\n  방향: {direction}\n  PeerId: {peer_id}\n  원격 주소: {remote_address}\n  원격 IP: {}\n  연결 ID: {connection_id}\n  연결 시간: {}\n  종료 원인: {}\n  고유 연결 피어: {unique_peers}\n  이 피어의 남은 연결: {peer_connections}",
                remote_ip.as_deref().unwrap_or("확인 불가"),
                connected_for
                    .map(format_duration)
                    .unwrap_or_else(|| "확인 불가".into()),
                cause.as_deref().unwrap_or("정상 종료")
            ),
            Self::PeerDiscovered(peer_id) => write!(formatter, "[P2P 발견] PeerId: {peer_id}"),
            Self::OutgoingConnectionFailed {
                peer_id,
                connection_id,
                error,
            } => write!(
                formatter,
                "[P2P 접속 실패]\n  PeerId: {}\n  연결 ID: {connection_id}\n  오류: {error}",
                peer_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "확인 불가".into())
            ),
            Self::BlockReceived { source, block } => {
                write!(
                    formatter,
                    "[P2P 블록 수신] PeerId: {source}, 높이: {}, 해시: {}, 거래: {}개, 시스템 이벤트: {}개",
                    block.height,
                    block.hash,
                    block.transactions.len(),
                    block.system_events.len()
                )
            }
            Self::TransactionReceived {
                source,
                transaction,
            } => write!(
                formatter,
                "[P2P 거래 수신] PeerId: {source}, 거래: {}",
                transaction.id()
            ),
            Self::ConsensusReceived { source, message } => {
                write!(
                    formatter,
                    "[P2P 합의 수신] PeerId: {source}, 메시지: {message:?}"
                )
            }
            Self::ProposalReceived { source, proposal } => write!(
                formatter,
                "[P2P 제안 수신] PeerId: {source}, 높이: {}, 해시: {}",
                proposal.height, proposal.block.hash
            ),
            Self::EvidenceReceived { source, evidence } => write!(
                formatter,
                "[P2P 이중투표 증거] PeerId: {source}, 증거: {}",
                evidence.id()
            ),
            Self::ValidatorRegistrationReceived {
                source,
                registration,
            } => write!(
                formatter,
                "[검증자 등록 수신] PeerId: {source}, 검증자: {}",
                registration.validator_id
            ),
            Self::NodeRewardRegistrationReceived {
                source,
                registration,
            } => write!(
                formatter,
                "[노드 보상 등록 수신] PeerId: {source}, 보상 주소: {}",
                registration.reward_address
            ),
            Self::UpdateAvailableReceived { source, version } => write!(
                formatter,
                "[P2P 업데이트 알림] PeerId: {source}, 버전: {version}"
            ),
            Self::SyncRequested {
                source,
                from_height,
                ..
            } => write!(
                formatter,
                "[P2P 동기화 요청] PeerId: {source}, 시작 높이: {from_height}"
            ),
            Self::SyncReceived {
                source,
                certificates,
                ..
            } => write!(
                formatter,
                "[P2P 동기화 응답] PeerId: {source}, 확정 블록: {}개",
                certificates.len()
            ),
            Self::SnapshotAttestationReceived {
                source,
                attestation,
            } => write!(
                formatter,
                "[snapshot 인증 투표] PeerId: {source}, 높이: {}, 검증자: {}",
                attestation.height, attestation.validator_id
            ),
            Self::CommunicationReceived { source, envelope } => write!(
                formatter,
                "[보안 통신 신호 수신] PeerId: {source}, 종류: {:?}, id: {}",
                envelope.kind, envelope.id
            ),
        }
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "IeumBehaviourEvent")]
struct IeumBehaviour {
    relay_client: relay::client::Behaviour,
    relay_server: relay::Behaviour,
    dcutr: dcutr::Behaviour,
    autonat: autonat::Behaviour,
    ping: ping::Behaviour,
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    identify: identify::Behaviour,
    communication: request_response::cbor::Behaviour<CommunicationEnvelope, CommunicationAck>,
}

#[derive(Debug)]
enum IeumBehaviourEvent {
    RelayClient(Box<relay::client::Event>),
    RelayServer(Box<relay::Event>),
    Dcutr(Box<dcutr::Event>),
    Autonat(Box<autonat::Event>),
    Ping(ping::Event),
    Gossipsub(gossipsub::Event),
    Mdns(mdns::Event),
    Kademlia(kad::Event),
    Identify(Box<identify::Event>),
    Communication(request_response::Event<CommunicationEnvelope, CommunicationAck>),
}

impl From<relay::client::Event> for IeumBehaviourEvent {
    fn from(value: relay::client::Event) -> Self {
        Self::RelayClient(Box::new(value))
    }
}
impl From<relay::Event> for IeumBehaviourEvent {
    fn from(value: relay::Event) -> Self {
        Self::RelayServer(Box::new(value))
    }
}
impl From<dcutr::Event> for IeumBehaviourEvent {
    fn from(value: dcutr::Event) -> Self {
        Self::Dcutr(Box::new(value))
    }
}
impl From<autonat::Event> for IeumBehaviourEvent {
    fn from(value: autonat::Event) -> Self {
        Self::Autonat(Box::new(value))
    }
}
impl From<ping::Event> for IeumBehaviourEvent {
    fn from(value: ping::Event) -> Self {
        Self::Ping(value)
    }
}
impl From<gossipsub::Event> for IeumBehaviourEvent {
    fn from(value: gossipsub::Event) -> Self {
        Self::Gossipsub(value)
    }
}
impl From<mdns::Event> for IeumBehaviourEvent {
    fn from(value: mdns::Event) -> Self {
        Self::Mdns(value)
    }
}
impl From<kad::Event> for IeumBehaviourEvent {
    fn from(value: kad::Event) -> Self {
        Self::Kademlia(value)
    }
}
impl From<identify::Event> for IeumBehaviourEvent {
    fn from(value: identify::Event) -> Self {
        Self::Identify(Box::new(value))
    }
}
impl From<request_response::Event<CommunicationEnvelope, CommunicationAck>> for IeumBehaviourEvent {
    fn from(value: request_response::Event<CommunicationEnvelope, CommunicationAck>) -> Self {
        Self::Communication(value)
    }
}

pub struct P2pNode {
    config: NetworkConfig,
}

impl P2pNode {
    pub fn new(config: NetworkConfig) -> Self {
        Self { config }
    }

    /// QUIC 리스너와 피어 검색을 시작하고, 명령/이벤트 채널을 돌려줍니다.
    pub async fn run(
        self,
    ) -> Result<
        (
            PeerId,
            mpsc::Sender<NetworkCommand>,
            mpsc::Receiver<NetworkEvent>,
        ),
        String,
    > {
        let config = self.config;
        let max_message_bytes = config.max_message_bytes;
        let idle_timeout = config.idle_timeout;
        let loopback_only = config.loopback_only;

        let identity_key = config
            .identity_key
            .unwrap_or_else(Keypair::generate_ed25519);
        let mut swarm = SwarmBuilder::with_existing_identity(identity_key)
            .with_tokio()
            .with_quic()
            .with_relay_client(noise::Config::new, libp2p::yamux::Config::default)
            .map_err(|error| error.to_string())?
            .with_behaviour(
                move |key,
                      relay_client|
                      -> Result<
                    IeumBehaviour,
                    Box<dyn std::error::Error + Send + Sync>,
                > {
                    let peer_id = PeerId::from(key.public());
                    let gossip_config = gossipsub::ConfigBuilder::default()
                        .max_transmit_size(max_message_bytes)
                        .validation_mode(gossipsub::ValidationMode::Strict)
                        .heartbeat_interval(Duration::from_secs(1))
                        .build()?;
                    let mut gossipsub = gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key.clone()),
                        gossip_config,
                    )?;
                    gossipsub.subscribe(&gossipsub::IdentTopic::new(BLOCK_TOPIC))?;
                    gossipsub.subscribe(&gossipsub::IdentTopic::new(CONSENSUS_TOPIC))?;
                    gossipsub.subscribe(&gossipsub::IdentTopic::new(SYNC_TOPIC))?;
                    let mdns = mdns::tokio::Behaviour::new(mdns::Config::default(), peer_id)?;
                    let store = kad::store::MemoryStore::new(peer_id);
                    let mut kademlia = kad::Behaviour::new(peer_id, store);
                    kademlia.set_mode(Some(kad::Mode::Server));
                    let identify = identify::Behaviour::new(identify::Config::new(
                        "/ieum-chain/1.1.0".into(),
                        key.public(),
                    ));
                    let communication = request_response::cbor::Behaviour::new(
                        [(
                            StreamProtocol::new(COMMUNICATION_PROTOCOL),
                            request_response::ProtocolSupport::Full,
                        )],
                        request_response::Config::default()
                            .with_request_timeout(Duration::from_secs(10)),
                    );
                    // 채굴장 내부의 다른 NAT 노드를 공인 판정 서버로 임의 선택하지
                    // 않고, bootstrap.json에 고정한 공개 노드만 사용합니다.
                    let autonat_config = autonat::Config {
                        use_connected: false,
                        ..Default::default()
                    };
                    Ok(IeumBehaviour {
                        relay_client,
                        relay_server: relay::Behaviour::new(peer_id, relay::Config::default()),
                        dcutr: dcutr::Behaviour::new(peer_id),
                        autonat: autonat::Behaviour::new(peer_id, autonat_config),
                        ping: ping::Behaviour::new(
                            ping::Config::new().with_interval(Duration::from_secs(20)),
                        ),
                        gossipsub,
                        mdns,
                        kademlia,
                        identify,
                        communication,
                    })
                },
            )
            .map_err(|error| error.to_string())?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(idle_timeout))
            .build();

        let local_peer_id = *swarm.local_peer_id();
        let local_network = LocalNetworkView::discover();
        local_network.log_summary();
        let listen_ip = if loopback_only {
            "127.0.0.1"
        } else {
            "0.0.0.0"
        };
        let listen: Multiaddr = format!("/ip4/{listen_ip}/udp/{}/quic-v1", config.listen_port)
            .parse()
            .map_err(|error| format!("리스닝 주소 오류: {error}"))?;
        swarm.listen_on(listen).map_err(|error| error.to_string())?;
        for address in config.external_addresses {
            crate::log_info!("[P2P 공개 주소 광고] {address}");
            swarm.add_external_address(address);
        }

        // 설정에는 /dns4/도메인 주소를 유지하되 QUIC dial 직전에 IPv4로 변환합니다.
        let bootstrap_addresses = config.bootstrap_peers.clone();
        for address in config.bootstrap_peers {
            add_bootstrap_address(&mut swarm, address, local_peer_id, loopback_only).await?;
        }
        let _ = swarm.behaviour_mut().kademlia.bootstrap();

        let (command_tx, mut command_rx) = mpsc::channel(128);
        let (event_tx, event_rx) = mpsc::channel(256);
        let mut guard = PeerGuard::new(config.ban_duration);
        let mut connected_at: HashMap<ConnectionId, Instant> = HashMap::new();
        let mut same_lan_peers = HashSet::new();
        let mut bootstrap_redial_tick = tokio::time::interval(Duration::from_secs(60));
        bootstrap_redial_tick.tick().await;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = bootstrap_redial_tick.tick() => {
                        if swarm.connected_peers().next().is_none() {
                            crate::log_info!(
                                "[P2P 자동 복구] 연결된 피어가 없어 원본 bootstrap DNS 주소로 재접속합니다."
                            );
                            for address in &bootstrap_addresses {
                                if let Err(error) = dial_address(&mut swarm, address.clone()).await {
                                    crate::logger::write_repeated_error(&error);
                                }
                            }
                        }
                    }
                    command = command_rx.recv() => {
                        match command {
                            Some(NetworkCommand::PublishBlock(block)) => {
                                publish(&mut swarm, BLOCK_TOPIC, &WireMessage::Block(block));
                            }
                            Some(NetworkCommand::PublishTransaction(transaction)) => {
                                publish(
                                    &mut swarm,
                                    BLOCK_TOPIC,
                                    &WireMessage::Transaction(transaction),
                                );
                            }
                            Some(NetworkCommand::PublishConsensus(message)) => {
                                publish(&mut swarm, CONSENSUS_TOPIC, &WireMessage::Consensus(message));
                            }
                            Some(NetworkCommand::PublishProposal(proposal)) => {
                                publish(&mut swarm, CONSENSUS_TOPIC, &WireMessage::Proposal(proposal));
                            }
                            Some(NetworkCommand::PublishEvidence(evidence)) => {
                                publish(&mut swarm, CONSENSUS_TOPIC, &WireMessage::Evidence(evidence));
                            }
                            Some(NetworkCommand::PublishValidatorRegistration(registration)) => {
                                publish(
                                    &mut swarm,
                                    CONSENSUS_TOPIC,
                                    &WireMessage::ValidatorRegistration(registration),
                                );
                            }
                            Some(NetworkCommand::PublishNodeRewardRegistration(registration)) => {
                                publish(
                                    &mut swarm,
                                    CONSENSUS_TOPIC,
                                    &WireMessage::NodeRewardRegistration(registration),
                                );
                            }
                            Some(NetworkCommand::PublishUpdateAvailable { version }) => {
                                publish(
                                    &mut swarm,
                                    CONSENSUS_TOPIC,
                                    &WireMessage::UpdateAvailable { version },
                                );
                            }
                            Some(NetworkCommand::RequestSync { from_height }) => {
                                publish(
                                    &mut swarm,
                                    SYNC_TOPIC,
                                    &WireMessage::SyncRequest {
                                        requester: local_peer_id.to_string(),
                                        from_height,
                                    },
                                );
                            }
                            Some(NetworkCommand::RespondSync { requester, tip, certificates }) => {
                                publish(
                                    &mut swarm,
                                    SYNC_TOPIC,
                                    &WireMessage::SyncResponse {
                                        requester,
                                        tip,
                                        certificates,
                                    },
                                );
                            }
                            Some(NetworkCommand::PublishSnapshotAttestation(attestation)) => {
                                publish(
                                    &mut swarm,
                                    CONSENSUS_TOPIC,
                                    &WireMessage::SnapshotAttestation(attestation),
                                );
                            }
                            Some(NetworkCommand::PenalizePeer { peer_id, points }) => {
                                if guard.penalize(&peer_id.to_string(), points)
                                    == PeerDecision::TemporarilyBlocked
                                {
                                    let _ = swarm.disconnect_peer_id(peer_id);
                                }
                            }
                            Some(NetworkCommand::Dial(address)) => {
                                if let Err(error) = dial_address(&mut swarm, address).await {
                                    crate::log_error!("{error}");
                                }
                            }
                            Some(NetworkCommand::SendCommunication(mut envelope)) => {
                                envelope.sender_peer_id = local_peer_id.to_string();
                                match envelope.target_peer_id.parse::<PeerId>() {
                                    Ok(target) => {
                                        swarm
                                            .behaviour_mut()
                                            .communication
                                            .send_request(&target, envelope);
                                    }
                                    Err(error) => crate::log_error!(
                                        "통신 대상 PeerId 형식 오류: {error}"
                                    ),
                                }
                            }
                            Some(NetworkCommand::Shutdown) | None => break,
                        }
                    }
                    event = swarm.select_next_some() => {
                        let context = SwarmEventContext {
                            event_tx: &event_tx,
                            local_network: &local_network,
                            max_message_bytes,
                            loopback_only,
                        };
                        if let Err(error) = handle_swarm_event(
                            &mut swarm,
                            event,
                            &mut guard,
                            &mut connected_at,
                            &mut same_lan_peers,
                            context,
                        ).await {
                            crate::log_error!("P2P 이벤트 처리 오류: {error}");
                        }
                    }
                }
            }
        });

        Ok((local_peer_id, command_tx, event_rx))
    }
}

async fn add_bootstrap_address(
    swarm: &mut libp2p::Swarm<IeumBehaviour>,
    address: Multiaddr,
    local_peer_id: PeerId,
    loopback_only: bool,
) -> Result<(), String> {
    let original_address = address.clone();
    let resolved_addresses = resolve_dns4_addresses(&address).await?;
    for mut resolved_address in resolved_addresses {
        let peer_id = match resolved_address.pop() {
            Some(Protocol::P2p(peer_id)) => peer_id,
            _ => return Err("부트스트랩 주소 끝에는 /p2p/PeerId가 필요합니다.".into()),
        };
        swarm
            .behaviour_mut()
            .kademlia
            .add_address(&peer_id, resolved_address.clone());
        if !loopback_only {
            swarm
                .behaviour_mut()
                .autonat
                .add_server(peer_id, Some(resolved_address.clone()));
        }
        resolved_address.push(Protocol::P2p(peer_id));
        crate::log_info!("[P2P 접속 시도] {original_address} -> {resolved_address}");
        swarm.dial(resolved_address.clone()).map_err(|error| {
            format!("[P2P 접속 시작 실패] 주소: {original_address}, 오류: {error}")
        })?;
        if !loopback_only
            && peer_id != local_peer_id
            && !resolved_address
                .iter()
                .any(|protocol| matches!(protocol, Protocol::P2pCircuit))
        {
            let relay_address = resolved_address.with(Protocol::P2pCircuit);
            match swarm.listen_on(relay_address.clone()) {
                Ok(_) => crate::log_info!(
                    "[NAT 릴레이 예약 시도] {relay_address} · 성공하면 포트 개방 없이 연결을 수신합니다."
                ),
                Err(error) => crate::log_error!(
                    "[NAT 릴레이 예약 시작 실패] 주소: {relay_address}, 오류: {error}"
                ),
            }
        }
    }
    Ok(())
}

async fn dial_address(
    swarm: &mut libp2p::Swarm<IeumBehaviour>,
    address: Multiaddr,
) -> Result<(), String> {
    let resolved_addresses = resolve_dns4_addresses(&address).await?;
    for resolved_address in resolved_addresses {
        crate::log_info!("[P2P 접속 시도] {address} -> {resolved_address}");
        swarm
            .dial(resolved_address)
            .map_err(|error| format!("[P2P 접속 시작 실패] 주소: {address}, 오류: {error}"))?;
    }
    Ok(())
}

/// QUIC transport가 직접 처리하지 못하는 `/dns4/...`를 `/ip4/...`로 변환합니다.
/// 설정에는 도메인을 그대로 두므로 노드를 다시 시작하거나 재접속할 때 최신 IP를 조회합니다.
async fn resolve_dns4_addresses(address: &Multiaddr) -> Result<Vec<Multiaddr>, String> {
    let domain = address.iter().find_map(|protocol| match protocol {
        Protocol::Dns4(domain) => Some(domain.into_owned()),
        _ => None,
    });
    let Some(domain) = domain else {
        return Ok(vec![address.clone()]);
    };

    let resolved = lookup_host((domain.as_str(), 0))
        .await
        .map_err(|error| format!("[P2P DNS 조회 실패] 도메인: {domain}, 오류: {error}"))?;
    let ipv4_addresses: HashSet<_> = resolved
        .filter_map(|socket_address| match socket_address.ip() {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        })
        .collect();
    if ipv4_addresses.is_empty() {
        return Err(format!(
            "[P2P DNS 조회 실패] 도메인 {domain}에서 IPv4 주소를 찾지 못했습니다."
        ));
    }

    let mut resolved_addresses = Vec::with_capacity(ipv4_addresses.len());
    for ip in ipv4_addresses {
        let mut resolved_address = Multiaddr::empty();
        for protocol in address.iter() {
            match protocol {
                Protocol::Dns4(_) => resolved_address.push(Protocol::Ip4(ip)),
                other => resolved_address.push(other),
            }
        }
        crate::log_info!("[P2P DNS 변환] {domain} -> {ip}");
        resolved_addresses.push(resolved_address);
    }
    resolved_addresses.sort();
    Ok(resolved_addresses)
}

fn publish(swarm: &mut libp2p::Swarm<IeumBehaviour>, topic: &str, message: &WireMessage) {
    publish_to_topic(swarm, gossipsub::IdentTopic::new(topic), message);
}

fn publish_to_topic(
    swarm: &mut libp2p::Swarm<IeumBehaviour>,
    topic: gossipsub::IdentTopic,
    message: &WireMessage,
) {
    let bytes = match encode_wire_message(message) {
        Ok(bytes) => bytes,
        Err(error) => {
            crate::log_error!("P2P 메시지 직렬화 실패: {error}");
            return;
        }
    };
    if let Err(error) = swarm.behaviour_mut().gossipsub.publish(topic, bytes) {
        let message = error.to_string();
        if message.to_ascii_lowercase().contains("no peers subscribed")
            || message.contains("NoPeersSubscribedToTopic")
        {
            crate::logger::write_repeated_info(
                "[P2P 전파 대기] 연결된 피어의 토픽 가입을 기다립니다.",
            );
        } else {
            crate::logger::write_repeated_error(&format!("P2P 메시지 전파 실패: {error}"));
        }
    }
}

/// 작은 메시지는 기존 JSON 그대로 유지하고, 큰 JSON만 zstd로 압축합니다.
/// 헤더에는 압축 해제 후 길이를 넣어 할당 전에 상한을 검사합니다.
fn encode_wire_message(message: &WireMessage) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if json.len() < COMPRESSION_THRESHOLD_BYTES || json.len() > u32::MAX as usize {
        return Ok(json);
    }
    let compressed = zstd::bulk::compress(&json, 3).map_err(|error| error.to_string())?;
    if WIRE_HEADER_BYTES + compressed.len() >= json.len() {
        return Ok(json);
    }
    let mut framed = Vec::with_capacity(WIRE_HEADER_BYTES + compressed.len());
    framed.extend_from_slice(COMPRESSED_WIRE_MAGIC);
    framed.extend_from_slice(&(json.len() as u32).to_be_bytes());
    framed.extend_from_slice(&compressed);
    Ok(framed)
}

fn decode_wire_message(bytes: &[u8], max_message_bytes: usize) -> Result<WireMessage, String> {
    let json = if bytes.starts_with(COMPRESSED_WIRE_MAGIC) {
        if bytes.len() < WIRE_HEADER_BYTES {
            return Err("압축 P2P 메시지 헤더가 잘렸습니다.".into());
        }
        let declared = u32::from_be_bytes(
            bytes[COMPRESSED_WIRE_MAGIC.len()..WIRE_HEADER_BYTES]
                .try_into()
                .map_err(|_| "압축 P2P 메시지 길이 헤더가 잘못되었습니다.")?,
        ) as usize;
        if declared > max_message_bytes {
            return Err(format!(
                "압축 해제 크기가 제한을 넘습니다: declared={declared}, max={max_message_bytes}"
            ));
        }
        let decoded = zstd::bulk::decompress(&bytes[WIRE_HEADER_BYTES..], declared)
            .map_err(|error| format!("zstd 압축 해제 실패: {error}"))?;
        if decoded.len() != declared {
            return Err(format!(
                "압축 해제 크기가 헤더와 다릅니다: declared={declared}, actual={}",
                decoded.len()
            ));
        }
        decoded
    } else {
        bytes.to_vec()
    };
    serde_json::from_slice(&json).map_err(|error| format!("json={error}"))
}

struct SwarmEventContext<'a> {
    event_tx: &'a mpsc::Sender<NetworkEvent>,
    local_network: &'a LocalNetworkView,
    max_message_bytes: usize,
    loopback_only: bool,
}

async fn handle_swarm_event(
    swarm: &mut libp2p::Swarm<IeumBehaviour>,
    event: SwarmEvent<IeumBehaviourEvent>,
    guard: &mut PeerGuard,
    connected_at: &mut HashMap<ConnectionId, Instant>,
    same_lan_peers: &mut HashSet<PeerId>,
    context: SwarmEventContext<'_>,
) -> Result<(), String> {
    let SwarmEventContext {
        event_tx,
        local_network,
        max_message_bytes,
        loopback_only,
    } = context;

    match event {
        SwarmEvent::Behaviour(IeumBehaviourEvent::RelayClient(event)) => match *event {
            relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } => crate::log_info!(
                "[NAT 릴레이 준비 완료] Relay PeerId: {relay_peer_id} · 수동 포트 개방 없이 연결 가능합니다."
            ),
            other => crate::logger::write_repeated_info(&format!("[NAT 릴레이 상태] {other:?}")),
        },
        SwarmEvent::Behaviour(IeumBehaviourEvent::RelayServer(event)) => {
            crate::logger::write_repeated_info(&format!("[NAT 릴레이 서버] {event:?}"));
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Dcutr(event)) => {
            crate::log_info!("[NAT 홀 펀칭] {event:?}");
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Autonat(event)) => match *event {
            autonat::Event::StatusChanged { old, new } => match new {
                autonat::NatStatus::Public(address) => crate::log_info!(
                    "[자동 역할 판정] 공개 네트워크 지원 가능 · 외부 직접 접근 주소: {address} · 이전 상태: {old:?}"
                ),
                autonat::NatStatus::Private => crate::log_info!(
                    "[자동 역할 판정] 일반 클라이언트 유지 · 외부 역접속 불가 · NAT/방화벽 내부이거나 판정 서버가 같은 공인 IP에 있습니다. 릴레이 또는 다른 공인 IP의 판정 서버가 필요합니다. · 이전 상태: {old:?}"
                ),
                autonat::NatStatus::Unknown => crate::log_info!(
                    "[NAT 접근성 판정] 아직 확정할 수 없습니다. 서로 다른 공인 IP의 AutoNAT 서버 응답을 기다립니다. · 이전 상태: {old:?}"
                ),
            },
            other => crate::logger::write_repeated_info(&format!("[NAT 접근성 확인 중] {other:?}")),
        },
        SwarmEvent::Behaviour(IeumBehaviourEvent::Ping(event)) => {
            if let Err(error) = event.result {
                crate::logger::write_repeated_error(&format!(
                    "[P2P 연결 상태 확인 실패] PeerId: {}, 오류: {error}",
                    event.peer
                ));
            }
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
            if loopback_only {
                return Ok(());
            }
            for (peer, address) in peers {
                // 다른 로컬 프로세스가 광고한 loopback·Docker bridge 주소는 그 프로세스
                // 바깥에서 같은 PeerId를 보장하지 않습니다. 이를 Kademlia에 넣으면
                // 주소의 실제 노드와 예상 PeerId가 달라지는 반복 dial 오류가 발생합니다.
                if !local_network.accepts_learned_address(&address, true) {
                    crate::logger::write_repeated_info(&format!(
                        "[P2P 주소 제외] PeerId: {peer} · 주소: {address} · 현재 노드와 같은 LAN이 아니거나 로컬/가상 인터페이스 주소입니다."
                    ));
                    swarm
                        .behaviour_mut()
                        .kademlia
                        .remove_address(&peer, &address);
                    continue;
                }
                same_lan_peers.insert(peer);
                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer);
                swarm.behaviour_mut().kademlia.add_address(&peer, address);
                let _ = event_tx.send(NetworkEvent::PeerDiscovered(peer)).await;
            }
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
            if loopback_only {
                return Ok(());
            }
            for (peer, _) in peers {
                swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer);
                same_lan_peers.remove(&peer);
            }
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Identify(event)) => {
            if let identify::Event::Received { peer_id, info, .. } = *event {
                for address in info.listen_addrs {
                    let Some(address) = normalize_learned_address(&address, peer_id) else {
                        crate::logger::write_repeated_info(&format!(
                            "[P2P 주소 제외] PeerId: {peer_id} · 주소: {address} · 중첩되거나 잘못된 릴레이 경로입니다."
                        ));
                        continue;
                    };
                    let resolved_addresses = match resolve_dns4_addresses(&address).await {
                        Ok(addresses) => addresses,
                        Err(error) => {
                            crate::logger::write_repeated_error(&format!(
                                "[P2P 학습 주소 DNS 변환 실패] PeerId: {peer_id} · 주소: {address} · {error}"
                            ));
                            continue;
                        }
                    };
                    for resolved_address in resolved_addresses {
                        if !local_network.accepts_learned_address(
                            &resolved_address,
                            same_lan_peers.contains(&peer_id),
                        ) {
                            crate::logger::write_repeated_info(&format!(
                                "[P2P 주소 제외] PeerId: {peer_id} · 주소: {resolved_address} · 현재 노드와 같은 LAN이 아니거나 로컬/가상 인터페이스 주소입니다."
                            ));
                            swarm
                                .behaviour_mut()
                                .kademlia
                                .remove_address(&peer_id, &resolved_address);
                            continue;
                        }
                        swarm
                            .behaviour_mut()
                            .kademlia
                            .add_address(&peer_id, resolved_address);
                    }
                }
            }
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            propagation_source,
            message,
            ..
        })) => {
            let peer_key = propagation_source.to_string();
            let message_source = message.source.unwrap_or(propagation_source);
            if guard.check(&peer_key) == PeerDecision::TemporarilyBlocked {
                return Ok(());
            }
            if message.data.len() > max_message_bytes {
                guard.penalize(&peer_key, 100);
                let _ = swarm.disconnect_peer_id(propagation_source);
                return Err("최대 크기를 넘는 메시지를 보낸 피어를 차단했습니다.".into());
            }
            let decoded: WireMessage = match decode_wire_message(&message.data, max_message_bytes) {
                Ok(value) => value,
                Err(error) => {
                    guard.penalize(&peer_key, 25);
                    return Err(format!(
                        "해석할 수 없는 메시지입니다: peer={message_source}, topic={}, bytes={}, detail={error}",
                        message.topic,
                        message.data.len()
                    ));
                }
            };
            // 여기서는 외부 포맷과 크기까지만 검사합니다.
            // 거래·블록·합의 서명의 의미 검증은 노드 코어에서 수행합니다.
            guard.reward(&peer_key);
            let network_event = match decoded {
                WireMessage::Block(block) => NetworkEvent::BlockReceived {
                    source: propagation_source,
                    block,
                },
                WireMessage::Transaction(transaction) => NetworkEvent::TransactionReceived {
                    source: propagation_source,
                    transaction,
                },
                WireMessage::Consensus(message) => NetworkEvent::ConsensusReceived {
                    source: propagation_source,
                    message,
                },
                WireMessage::Proposal(proposal) => NetworkEvent::ProposalReceived {
                    source: propagation_source,
                    proposal,
                },
                WireMessage::Evidence(evidence) => NetworkEvent::EvidenceReceived {
                    source: propagation_source,
                    evidence,
                },
                WireMessage::ValidatorRegistration(registration) => {
                    NetworkEvent::ValidatorRegistrationReceived {
                        source: propagation_source,
                        registration,
                    }
                }
                WireMessage::NodeRewardRegistration(registration) => {
                    NetworkEvent::NodeRewardRegistrationReceived {
                        source: message_source,
                        registration,
                    }
                }
                WireMessage::UpdateAvailable { version } => NetworkEvent::UpdateAvailableReceived {
                    source: propagation_source,
                    version,
                },
                WireMessage::SyncRequest {
                    requester,
                    from_height,
                } => NetworkEvent::SyncRequested {
                    source: propagation_source,
                    requester,
                    from_height,
                },
                WireMessage::SyncResponse {
                    requester,
                    tip,
                    certificates,
                } => {
                    if requester != swarm.local_peer_id().to_string() {
                        return Ok(());
                    }
                    NetworkEvent::SyncReceived {
                        // quorum은 메시지를 전달한 릴레이가 아니라 서명된 gossipsub
                        // 원 작성자 기준으로 독립 피어를 집계해야 합니다.
                        source: message_source,
                        tip,
                        certificates,
                    }
                }
                WireMessage::SnapshotAttestation(attestation) => {
                    NetworkEvent::SnapshotAttestationReceived {
                        source: propagation_source,
                        attestation,
                    }
                }
            };
            event_tx
                .send(network_event)
                .await
                .map_err(|e| e.to_string())?;
        }
        SwarmEvent::Behaviour(IeumBehaviourEvent::Communication(event)) => match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => {
                    let peer_key = peer.to_string();
                    if guard.check(&peer_key) == PeerDecision::TemporarilyBlocked {
                        return Ok(());
                    }
                    let accepted =
                        validate_direct_communication(swarm.local_peer_id(), &peer, &request)
                            .is_ok();
                    if accepted {
                        guard.reward(&peer_key);
                    } else {
                        guard.penalize(&peer_key, 50);
                    }
                    let ack = CommunicationAck {
                        message_id: request.id.clone(),
                        accepted,
                    };
                    swarm
                        .behaviour_mut()
                        .communication
                        .send_response(channel, ack)
                        .map_err(|_| "통신 신호 응답 전송에 실패했습니다.".to_string())?;
                    if accepted {
                        event_tx
                            .send(NetworkEvent::CommunicationReceived {
                                source: peer,
                                envelope: request,
                            })
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
                request_response::Message::Response { response, .. } => {
                    if !response.accepted {
                        crate::log_error!(
                            "대상 피어가 통신 신호를 거부했습니다: {}",
                            response.message_id
                        );
                    }
                }
            },
            request_response::Event::OutboundFailure { peer, error, .. } => {
                crate::log_error!("통신 신호 직접 전송 실패: PeerId {peer}, 오류: {error}");
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                guard.penalize(&peer.to_string(), 25);
                crate::log_error!("통신 신호 직접 수신 실패: PeerId {peer}, 오류: {error}");
            }
            request_response::Event::ResponseSent { .. } => {}
        },
        SwarmEvent::ConnectionEstablished {
            peer_id,
            connection_id,
            endpoint,
            num_established,
            ..
        } => {
            connected_at.insert(connection_id, Instant::now());
            let remote_address = endpoint.get_remote_address().clone();
            let unique_peers = swarm.connected_peers().count();
            let _ = event_tx
                .send(NetworkEvent::PeerConnected {
                    peer_id,
                    remote_ip: multiaddr_ip(&remote_address),
                    remote_address,
                    direction: connection_direction(&endpoint),
                    connection_id: format!("{connection_id:?}"),
                    unique_peers,
                    peer_connections: num_established.get() as usize,
                })
                .await;
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            connection_id,
            endpoint,
            num_established,
            cause,
            ..
        } => {
            let remote_address = endpoint.get_remote_address().clone();
            let unique_peers = swarm.connected_peers().count();
            let connected_for = connected_at
                .remove(&connection_id)
                .map(|started| started.elapsed());
            let _ = event_tx
                .send(NetworkEvent::PeerDisconnected {
                    peer_id,
                    remote_ip: multiaddr_ip(&remote_address),
                    remote_address,
                    direction: connection_direction(&endpoint),
                    connection_id: format!("{connection_id:?}"),
                    connected_for,
                    unique_peers,
                    peer_connections: num_established as usize,
                    cause: cause.map(|error| error.to_string()),
                })
                .await;
        }
        SwarmEvent::OutgoingConnectionError {
            connection_id,
            peer_id,
            error,
        } => {
            let _ = event_tx
                .send(NetworkEvent::OutgoingConnectionFailed {
                    peer_id,
                    connection_id: format!("{connection_id:?}"),
                    error: error.to_string(),
                })
                .await;
        }
        SwarmEvent::NewListenAddr { address, .. } => {
            crate::log_info!("QUIC P2P 대기: {address}/p2p/{}", swarm.local_peer_id());
        }
        // Kademlia 이벤트는 Behaviour 내부에서 이미 상태에 반영됩니다.
        // 값을 명시적으로 소비해 이벤트 필드가 사용되지 않는다는 경고를 피합니다.
        SwarmEvent::Behaviour(IeumBehaviourEvent::Kademlia(_event)) => {}
        _ => {}
    }
    Ok(())
}

/// Identify/Kademlia로 학습한 주소에서 목적지 PeerId 중복과 릴레이 재중첩을 제거합니다.
///
/// Kademlia에는 대상 PeerId를 별도 인자로 전달하므로 주소 끝의 `/p2p/<대상>`은
/// 제거합니다. `/p2p-circuit`은 한 번만 허용하여 이미 릴레이된 주소에 또 릴레이
/// 경로가 붙는 것을 막습니다.
fn normalize_learned_address(address: &Multiaddr, peer_id: PeerId) -> Option<Multiaddr> {
    let protocols = address.iter().collect::<Vec<_>>();
    if protocols
        .iter()
        .filter(|protocol| matches!(protocol, Protocol::P2pCircuit))
        .count()
        > 1
    {
        return None;
    }

    let mut normalized = Multiaddr::empty();
    let mut after_circuit = false;
    for (index, protocol) in protocols.iter().enumerate() {
        match protocol {
            Protocol::P2pCircuit => {
                after_circuit = true;
                normalized.push(protocol.clone());
            }
            Protocol::P2p(found) if *found == peer_id && index + 1 == protocols.len() => {
                // 대상 PeerId는 Kademlia가 별도로 보관하므로 주소에서는 제거합니다.
            }
            Protocol::P2p(_) if after_circuit => {
                // circuit 뒤에는 대상 PeerId 외의 경로가 오면 안 됩니다.
                return None;
            }
            _ => normalized.push(protocol.clone()),
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn is_lan_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 169 && b == 254)
        || (a == 100 && (64..=127).contains(&b))
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_broadcast()
        && !ip.is_multicast()
        && !is_lan_ipv4(ip)
        && a != 0
        && a < 224
        && !(a == 192 && b == 0)
        && !(a == 198 && (b == 18 || b == 19))
        && !(a == 198 && b == 51)
        && !(a == 203 && b == 0)
}

fn validate_direct_communication(
    local_peer_id: &PeerId,
    source: &PeerId,
    envelope: &CommunicationEnvelope,
) -> Result<(), String> {
    if envelope.target_peer_id != local_peer_id.to_string() {
        return Err("통신 메시지 대상 PeerId가 현재 노드와 다릅니다.".into());
    }
    if envelope.sender_peer_id != source.to_string() {
        return Err("통신 메시지 발신 PeerId가 실제 연결과 다릅니다.".into());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("시스템 시각 오류: {error}"))?
        .as_secs();
    envelope.validate(now)
}

fn connection_direction(endpoint: &ConnectedPoint) -> &'static str {
    match endpoint {
        ConnectedPoint::Dialer { .. } => "발신",
        ConnectedPoint::Listener { .. } => "수신",
    }
}

fn multiaddr_ip(address: &Multiaddr) -> Option<String> {
    address.iter().find_map(|protocol| match protocol {
        Protocol::Ip4(ip) => Some(ip.to_string()),
        Protocol::Ip6(ip) => Some(ip.to_string()),
        _ => None,
    })
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3_600;
    let minutes = total_seconds % 3_600 / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}시간 {minutes}분 {seconds}초")
    } else if minutes > 0 {
        format!("{minutes}분 {seconds}초")
    } else {
        format!("{seconds}초")
    }
}

#[cfg(test)]
mod connection_log_tests {
    use super::*;
    use crate::{
        Block, CommunicationKind, ScheduledEvent, ScheduledEventAction, SignedProposal, Wallet,
    };

    #[test]
    fn p2p_proposal_with_u128_transaction_round_trips() {
        let sender = Wallet::from_seed([31; 32]);
        let receiver = Wallet::from_seed([32; 32]);
        let proposer = Wallet::from_seed([33; 32]);
        let transaction = sender.sign_transfer(
            receiver.address(),
            100_000_000_000_000_000_u128,
            21_000_u128,
            0,
        );
        let block = Block::new(
            1,
            Block::genesis().hash,
            1_785_914_671,
            proposer.address(),
            vec![transaction],
        );
        let proposal = SignedProposal::new(1, 0, &proposer, block);
        let wire = WireMessage::Proposal(proposal.clone());

        let bytes = encode_wire_message(&wire).unwrap();
        let decoded = decode_wire_message(&bytes, 2 * 1024 * 1024).unwrap();

        match decoded {
            WireMessage::Proposal(decoded) => assert_eq!(decoded, proposal),
            _ => panic!("제안 WireMessage가 다른 종류로 역직렬화되었습니다."),
        }
    }

    #[test]
    fn p2p_proposal_with_u128_system_event_round_trips() {
        let proposer = Wallet::from_seed([34; 32]);
        let block = Block::new(
            1,
            Block::genesis().hash,
            1_785_914_671,
            proposer.address(),
            Vec::new(),
        )
        .with_system_events(vec![ScheduledEvent {
            id: "periodic-producer-reward-test".into(),
            execute_at: 1_785_914_671,
            action: ScheduledEventAction::PeriodicProducerReward {
                producer: proposer.address(),
                amount: 10 * 10u128.pow(18),
            },
        }]);
        let proposal = SignedProposal::new(1, 0, &proposer, block);
        let wire = WireMessage::Proposal(proposal.clone());

        let bytes = encode_wire_message(&wire).unwrap();
        let decoded = decode_wire_message(&bytes, 2 * 1024 * 1024).unwrap();

        match decoded {
            WireMessage::Proposal(decoded) => assert_eq!(decoded, proposal),
            _ => panic!("시스템 이벤트 Proposal이 다른 종류로 역직렬화되었습니다."),
        }
    }

    #[test]
    fn compressed_wire_magic_has_binary_version_byte() {
        assert_eq!(COMPRESSED_WIRE_MAGIC, &[b'I', b'E', b'U', b'M', b'Z', 0x01]);
        assert_eq!(WIRE_HEADER_BYTES, 10);
    }

    #[test]
    fn large_compressible_wire_message_uses_zstd_frame() {
        // Proposal의 실제 길이는 키/서명 표현이 바뀌면 압축 임계값의 양쪽으로
        // 이동할 수 있습니다. 압축 정책은 크기가 확실하고 반복 가능한 payload로
        // 별도 검증해 u128 왕복 테스트와 결합하지 않습니다.
        let wire = WireMessage::UpdateAvailable {
            version: "a".repeat(COMPRESSION_THRESHOLD_BYTES * 2),
        };

        let bytes = encode_wire_message(&wire).unwrap();
        assert!(bytes.starts_with(COMPRESSED_WIRE_MAGIC));

        let decoded = decode_wire_message(&bytes, 2 * 1024 * 1024).unwrap();
        match decoded {
            WireMessage::UpdateAvailable { version } => {
                assert_eq!(version, "a".repeat(COMPRESSION_THRESHOLD_BYTES * 2));
            }
            _ => panic!("압축 WireMessage가 다른 종류로 역직렬화되었습니다."),
        }
    }

    #[test]
    fn transaction_u128_is_a_decimal_string_and_legacy_number_is_accepted() {
        let sender = Wallet::from_seed([41; 32]);
        let receiver = Wallet::from_seed([42; 32]);
        let transaction = sender.sign_transfer(receiver.address(), u128::MAX, 21_000, 0);
        let json = serde_json::to_string(&transaction).unwrap();
        assert!(json.contains(&format!("\"amount\":\"{}\"", u128::MAX)));

        let legacy = json.replace("\"fee\":\"21000\"", "\"fee\":21000");
        let decoded: Transaction = serde_json::from_str(&legacy).unwrap();
        assert_eq!(decoded.amount, u128::MAX);
        assert_eq!(decoded.fee, 21_000);
    }

    #[test]
    fn compressed_message_declared_size_is_bounded_before_decompression() {
        let mut malicious = COMPRESSED_WIRE_MAGIC.to_vec();
        malicious.extend_from_slice(&10_000_u32.to_be_bytes());
        malicious.extend_from_slice(&[0; 8]);
        let error = decode_wire_message(&malicious, 1_000).unwrap_err();
        assert!(error.contains("제한을 넘습니다"));
    }

    #[test]
    fn extracts_ipv4_from_multiaddr() {
        let address: Multiaddr = "/ip4/192.168.1.193/udp/7001/quic-v1".parse().unwrap();
        assert_eq!(multiaddr_ip(&address).as_deref(), Some("192.168.1.193"));
    }

    #[test]
    fn only_accepts_private_addresses_on_the_same_physical_lan() {
        let local_network = LocalNetworkView {
            public_ipv4: Vec::new(),
        };
        let loopback: Multiaddr = "/ip4/127.0.0.1/udp/7001/quic-v1".parse().unwrap();
        let docker: Multiaddr = "/ip4/172.18.0.1/udp/7001/quic-v1".parse().unwrap();
        let lan: Multiaddr = "/ip4/192.168.1.20/udp/7001/quic-v1".parse().unwrap();
        let other_lan: Multiaddr = "/ip4/192.168.153.129/udp/7001/quic-v1".parse().unwrap();
        let public: Multiaddr = "/ip4/122.35.243.20/udp/7001/quic-v1".parse().unwrap();
        assert!(!local_network.accepts_learned_address(&loopback, true));
        assert!(!local_network.accepts_learned_address(&docker, false));
        assert!(local_network.accepts_learned_address(&docker, true));
        assert!(local_network.accepts_learned_address(&lan, true));
        assert!(!local_network.accepts_learned_address(&other_lan, false));
        assert!(local_network.accepts_learned_address(&public, false));
    }

    #[test]
    fn removes_destination_peer_from_learned_relay_address() {
        let relay = PeerId::random();
        let destination = PeerId::random();
        let address: Multiaddr = format!(
            "/ip4/122.35.243.20/udp/7001/quic-v1/p2p/{relay}/p2p-circuit/p2p/{destination}"
        )
        .parse()
        .unwrap();
        let normalized = normalize_learned_address(&address, destination).unwrap();
        assert_eq!(
            normalized.to_string(),
            format!("/ip4/122.35.243.20/udp/7001/quic-v1/p2p/{relay}/p2p-circuit")
        );
    }

    #[test]
    fn rejects_nested_relay_address() {
        let relay_a = PeerId::random();
        let relay_b = PeerId::random();
        let destination = PeerId::random();
        let address: Multiaddr = format!(
            "/ip4/122.35.243.20/udp/7001/quic-v1/p2p/{relay_a}/p2p-circuit/p2p/{relay_b}/p2p-circuit/p2p/{destination}"
        )
        .parse()
        .unwrap();
        assert!(normalize_learned_address(&address, destination).is_none());
    }

    #[test]
    fn classifies_public_private_and_cgnat_ipv4() {
        assert!(is_public_ipv4(Ipv4Addr::new(122, 35, 243, 20)));
        assert!(!is_public_ipv4(Ipv4Addr::new(192, 168, 1, 10)));
        assert!(!is_public_ipv4(Ipv4Addr::new(100, 64, 1, 10)));
        assert!(!is_public_ipv4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn formats_connection_duration() {
        assert_eq!(format_duration(Duration::from_secs(3_725)), "1시간 2분 5초");
    }

    #[test]
    fn block_received_log_is_concise_and_does_not_expose_signature() {
        let block = Block::new(
            2,
            "00".repeat(32),
            123,
            "validator".into(),
            vec![Transaction {
                from: "0x1111111111111111111111111111111111111111".into(),
                to: "0x2222222222222222222222222222222222222222".into(),
                amount: 1,
                fee: 21_000,
                nonce: 0,
                signature: "ethraw:secret-signature".into(),
            }],
        );
        let line = NetworkEvent::BlockReceived {
            source: PeerId::random(),
            block: block.clone(),
        }
        .to_string();

        assert!(line.contains("높이: 2"));
        assert!(line.contains(&format!("해시: {}", block.hash)));
        assert!(line.contains("거래: 1개"));
        assert!(!line.contains("ethraw:"));
        assert!(!line.contains("signature"));
    }

    #[tokio::test]
    async fn keeps_ipv4_multiaddr_unchanged() {
        let address: Multiaddr =
            "/ip4/122.35.243.20/udp/7001/quic-v1/p2p/12D3KooWAVRZjnbP8nXp8vD6irYFAXdLJVyczEFWdKLFzKnKDATx"
                .parse()
                .unwrap();
        assert_eq!(
            resolve_dns4_addresses(&address).await.unwrap(),
            vec![address]
        );
    }

    #[test]
    fn direct_communication_rejects_spoofed_sender_and_wrong_target() {
        let source = PeerId::random();
        let target = PeerId::random();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut envelope = CommunicationEnvelope {
            id: "call_0123456789abcdef".into(),
            sender_peer_id: source.to_string(),
            target_peer_id: target.to_string(),
            kind: CommunicationKind::CallInvite,
            created_at: now,
            expires_at: now + 60,
            encrypted_payload_hex: "aabbcc".into(),
        };
        assert!(validate_direct_communication(&target, &source, &envelope).is_ok());

        envelope.sender_peer_id = PeerId::random().to_string();
        assert!(validate_direct_communication(&target, &source, &envelope).is_err());
        envelope.sender_peer_id = source.to_string();
        assert!(validate_direct_communication(&PeerId::random(), &source, &envelope).is_err());
    }
}
