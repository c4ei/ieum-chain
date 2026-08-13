use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use ieum_chain::{
    AccountWallet, ConsensusRuntime, ConsensusTimeouts, EventSchedule, EvidenceStore,
    ExternalSigner, FinalityStore, GenesisConfig, Keystore, NetworkCommand, NetworkConfig,
    NetworkEvent, NodeRewardRegistration, NodeWalletKeystore, P2pNode, RpcConfig, RpcServer,
    ScheduledEvent, ScheduledEventAction, SnapshotAttestation, SnapshotCertificate, SyncTip,
    TipQuorum, Transaction, UpgradeSchedule, Validator, ValidatorRegistration, ValidatorSigner,
    Wallet, log_error, log_info, logger::init_server_log, node_key::load_or_create_node_key,
};
use libp2p::{Multiaddr, multiaddr::Protocol};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_BOOTSTRAP_CONFIG: &str = "config/bootstrap.json";
const DEFAULT_NETWORK_CONFIG: &str = "config/network.json";
const DEFAULT_ACCOUNT_PASSWORD_FILE: &str = "secure/ieum-account.password";
const DEFAULT_BOOTSTRAP_PEERS: [&str; 4] = [
    "/dns4/node.ieum.aah.name/udp/7001/quic-v1/p2p/12D3KooWGABnBEucGacnREpBieFwspL5q7Aa6RRuj1MtxEYwrPo2",
    "/dns4/node.ieum.aah.name/udp/7002/quic-v1/p2p/12D3KooWLqqVdBzWGGc3bjaarVpsu2WA6DTYuzKGoYCznbgrugnX",
    "/dns4/node.ieum.aah.name/udp/7003/quic-v1/p2p/12D3KooWE18Cv12b4R5bjrZg1RDGiPXbMDQZqz1t9rfrNaLpwDRB",
    "/dns4/node.ieum.aah.name/udp/7004/quic-v1/p2p/12D3KooWCByiTkyDHySsS3GRFVHue1ewPGSdgSWEvzMrtwggM3Wg",
];
const SERVER_INSTANCE_PORT: u16 = 49_889;
const CLIENT_INSTANCE_PORT: u16 = 49_890;
const SUPPORTED_PROTOCOL_VERSION: u32 = 2;

mod installation;

#[derive(Debug, Parser)]
#[command(name = "ieum-chain", version, about = "가벼운 IEUM 테스트넷 노드")]
struct Args {
    /// 실행 역할. 생략하면 기존 검증자 자격을 안전하게 감지하고 나머지는 일반 노드로
    /// 시작합니다.
    #[arg(long, value_enum, default_value_t = RunMode::Auto, global = true)]
    mode: RunMode,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum RunMode {
    /// 기존 검증자 키와 승인 목록이 일치하면 검증자, 아니면 일반 노드로 실행합니다.
    #[default]
    Auto,
    /// 송금·조회·동기화를 수행하는 일반 사용자 노드입니다.
    Client,
    /// 외부 접속을 지원하려고 시도하며 AutoNAT 결과로 실제 도달 가능성을 확인합니다.
    Public,
    /// 승인된 검증자 키로 합의에 참여합니다.
    Validator,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 외부 노드가 접속하는 운영 부트스트랩 서버로 실행합니다.
    Server(NodeArgs),
    /// 일반 PC 노드로 실행하고 운영 서버에 연결합니다.
    Client(ClientArgs),
    /// 서명 manifest를 검사하고 실행파일만 비대화형으로 교체합니다.
    /// 서버는 systemd가 중지한 상태에서 호출해야 합니다.
    Update {
        /// 서명된 업데이트 manifest URL
        #[arg(long)]
        manifest_url: String,
        /// manifest를 검증할 IEUM 릴리스 Ed25519 공개키(32바이트 hex)
        #[arg(long)]
        release_public_key: String,
    },
    /// 운영 검증자 Ed25519 키와 공개키 설정을 관리합니다.
    ValidatorKey {
        #[command(subcommand)]
        command: ValidatorKeyCommand,
    },
    /// 신규 노드 초기화와 기존 노드 상태 검증을 수행합니다.
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    /// 이 노드의 최초 참여 보상 지갑을 조회하거나 송금합니다.
    Reward {
        #[command(subcommand)]
        command: RewardCommand,
    },
    /// geth personal 계정과 같은 0x 지갑을 로컬 암호화 keystore로 관리합니다.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// 부트스트랩과 이 서버가 외부에 광고할 공개 주소를 관리합니다.
    Network {
        #[command(subcommand)]
        command: NetworkCommandConfig,
    },
    /// 체인 사고 복구 원칙과 안전한 복구 방식을 안내합니다.
    Recovery {
        #[command(subcommand)]
        command: RecoveryCommand,
    },
}

#[derive(Debug, Subcommand)]
enum RecoveryCommand {
    /// 거래 단위 복구와 체크포인트 롤백의 사용 기준을 표시합니다.
    Policy,
}

#[derive(Debug, Subcommand)]
enum NetworkCommandConfig {
    /// 현재 적용되는 네트워크 설정을 표시합니다.
    Show,
    /// 지정한 항목만 저장합니다. 생략한 항목은 기존 값을 유지합니다.
    Set {
        /// 시작 시 접속할 공개 노드 주소. 여러 개면 옵션을 반복합니다.
        #[arg(long = "bootstrap")]
        bootstrap_peers: Vec<Multiaddr>,
        /// 이 서버의 포트포워딩된 공개 광고 주소.
        #[arg(long)]
        advertise_address: Option<Multiaddr>,
    },
    /// 사용자 설정을 지우고 내장된 자동 기본값으로 되돌립니다.
    Reset,
}

#[derive(Debug, Subcommand)]
enum RewardCommand {
    /// 이 노드의 영구 보상 주소를 출력합니다.
    Address {
        #[arg(long, default_value = "data/keys/node_wallet.keystore")]
        keystore: PathBuf,
        #[arg(long, default_value = "data/keys/node_wallet.password")]
        password_file: PathBuf,
    },
    /// 보상 잔액을 다른 IEUM 지갑으로 서명 전송합니다.
    Send {
        #[arg(long)]
        to: String,
        /// 사람이 읽는 IEUM 단위(예: 1 또는 0.25)
        #[arg(long)]
        amount: String,
        /// 사람이 읽는 IEUM 단위. 기본 0.000001 IEUM
        #[arg(long, default_value = "0.000001")]
        fee: String,
        #[arg(long, default_value = "data/keys/node_wallet.keystore")]
        keystore: PathBuf,
        #[arg(long, default_value = "data/keys/node_wallet.password")]
        password_file: PathBuf,
        #[arg(long, default_value_t = 8989)]
        rpc_port: u16,
    },
    /// 노드 지갑 주소를 유지한 채 사용자가 준비한 새 암호로 원자적으로 재암호화합니다.
    ChangePassword {
        #[arg(long, default_value = "data/keys/node_wallet.keystore")]
        keystore: PathBuf,
        #[arg(long, default_value = "data/keys/node_wallet.password")]
        password_file: PathBuf,
        /// 새 암호만 담긴 소유자 전용(0600) 파일. 명령행에 암호를 직접 노출하지 않습니다.
        #[arg(long)]
        new_password_file: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum AccountCommand {
    /// 새 secp256k1 계정을 만들고 0x 주소를 출력합니다.
    New {
        #[arg(long, default_value = "data/keystore")]
        keystore_dir: PathBuf,
        /// 10자 이상 암호 한 줄을 담은 0600 파일. 없으면 이 인스턴스에 자동 생성합니다.
        #[arg(long, default_value = DEFAULT_ACCOUNT_PASSWORD_FILE)]
        password_file: PathBuf,
    },
    /// 32바이트 secp256k1 개인키 파일을 암호화 keystore로 가져옵니다.
    Import {
        /// 0x 접두사가 선택적인 64자리 hex 개인키 파일
        key_file: PathBuf,
        #[arg(long, default_value = "data/keystore")]
        keystore_dir: PathBuf,
        #[arg(long, default_value = DEFAULT_ACCOUNT_PASSWORD_FILE)]
        password_file: PathBuf,
    },
    /// 로컬 keystore에 저장된 0x 주소를 나열합니다.
    List {
        #[arg(long, default_value = "data/keystore")]
        keystore_dir: PathBuf,
    },
    /// 로컬 keystore 계정으로 서명해 실행 중인 노드 RPC에 송금합니다.
    Send {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: String,
        #[arg(long, default_value = "0.000001")]
        fee: String,
        #[arg(long, default_value = "data/keystore")]
        keystore_dir: PathBuf,
        #[arg(long, default_value = DEFAULT_ACCOUNT_PASSWORD_FILE)]
        password_file: PathBuf,
        #[arg(long, default_value_t = 8989)]
        rpc_port: u16,
    },
    /// 계정의 확정 잔액을 IEUM과 최소 단위로 조회합니다.
    Balance {
        address: String,
        #[arg(long, default_value_t = 8989)]
        rpc_port: u16,
    },
    /// 거래 해시로 트랜잭션을 조회합니다.
    Transaction {
        hash: String,
        #[arg(long, default_value_t = 8989)]
        rpc_port: u16,
    },
    /// 거래 해시로 영수증과 확정 여부를 조회합니다.
    Receipt {
        hash: String,
        #[arg(long, default_value_t = 8989)]
        rpc_port: u16,
    },
}

#[derive(Debug, Subcommand)]
enum NodeCommand {
    /// 기존 상태를 자동 백업하고 완전한 신규 서버 노드를 초기화합니다.
    Init {
        /// 기존 노드 복구가 아닌 신규 노드 생성을 명시적으로 확인합니다.
        #[arg(long, required = true)]
        new: bool,
    },
    /// 기존 서버 노드의 원장, validator.key, server.node.key 일치를 검사합니다.
    Verify,
    /// 키·원장·네트워크 설정을 점검하고 안전하게 자동 복구합니다.
    Doctor,
    /// 영구 노드 키를 읽거나 생성하고 대응하는 libp2p PeerId를 출력합니다.
    PeerId {
        /// 읽거나 처음 한 번 생성할 영구 노드 키
        #[arg(long, default_value = "data/keys/p2p_identity.key")]
        key: PathBuf,
    },
    /// 서버 신원 키는 보존하고 원장만 백업한 뒤 비워 재동기화합니다.
    Clean {
        /// 원장 백업 및 초기화를 명시적으로 확인합니다.
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ValidatorKeyCommand {
    /// 운영체제 난수로 검증자 개인 seed 파일을 안전하게 생성합니다.
    Generate {
        /// 생성할 개인키 파일
        #[arg(long, default_value = "data/keys/consensus_signing.key")]
        output: PathBuf,
    },
    /// 개인 seed 파일에서 validators.json에 넣을 공개키를 출력합니다.
    Public {
        /// 읽을 개인키 파일
        #[arg(long, default_value = "data/keys/consensus_signing.key")]
        key: PathBuf,
    },
    /// 이 검증자 키가 소유한 IEUM을 지정 지갑으로 안전하게 서명 전송합니다.
    Transfer {
        #[arg(long)]
        to: String,
        /// IEUM 금액 또는 잔액 전체에서 수수료를 뺀 `all`
        #[arg(long, default_value = "all")]
        amount: String,
        #[arg(long, default_value = "0.000001")]
        fee: String,
        #[arg(long, default_value = "data/keys/consensus_signing.key")]
        key: PathBuf,
        #[arg(long, default_value_t = 8989)]
        rpc_port: u16,
    },
    /// 공개키 4개 이상으로 모든 노드가 공유할 운영 설정을 생성합니다.
    CreateConfig {
        /// 검증자 Ed25519 공개키 32바이트 hex. 검증자 순서대로 반복합니다.
        #[arg(long = "public-key", required = true, num_args = 4..)]
        public_keys: Vec<String>,

        /// 각 검증자의 투표권
        #[arg(long, default_value_t = 100)]
        voting_power: u64,

        /// 생성할 운영 검증자 설정
        #[arg(long, default_value = "config/validators.json")]
        output: PathBuf,
    },
}

#[derive(Debug, ClapArgs)]
struct NodeArgs {
    /// GitHub Actions용 격리 실행. 루프백 P2P, 고정 개발 키와 짧은 합의 제한 시간을 사용합니다.
    #[arg(long = "git_action_test", default_value_t = false)]
    git_action_test: bool,

    /// QUIC UDP 리스닝 포트
    #[arg(long, default_value_t = 7001)]
    port: u16,

    /// 단일 P2P 메시지 최대 크기
    #[arg(long, default_value_t = 2_097_152)]
    max_message_bytes: usize,

    /// 확정 블록 사이의 최소 목표 간격(ms). 100ms~15000ms, 기본 5초입니다.
    #[arg(long, default_value_t = 5_000, value_parser = parse_block_time_ms)]
    block_time_ms: u64,

    /// 지갑/geth 호환 JSON-RPC TCP 포트
    #[arg(long, default_value_t = 8989)]
    rpc_port: u16,

    /// JSON-RPC 리스닝 IP. 기본값은 외부에 노출되지 않는 localhost입니다.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    rpc_host: IpAddr,

    /// RPC 원장·체크포인트 저장 경로
    #[arg(long, default_value = "data/ledger")]
    rpc_data_dir: PathBuf,

    /// 재시작 후에도 PeerId를 유지할 영구 노드 키 파일
    #[arg(long, default_value = "data/node.key")]
    node_key: PathBuf,

    /// 4노드 테스트넷 검증자 번호(1~4). server에서만 사용합니다.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=4))]
    validator_index: u8,

    /// 운영 검증자 Ed25519 seed 32바이트 hex 파일
    #[arg(long, default_value = "data/keys/consensus_signing.key")]
    validator_key: PathBuf,

    /// 검증자 공개키와 투표권 설정
    #[arg(long, default_value = "config/validators.json")]
    validators_config: PathBuf,

    /// 검증자 전원이 동일하게 배포하는 승인된 시간 기반 이벤트 설정
    #[arg(long, default_value = "config/events.json")]
    events_config: PathBuf,

    /// 서명된 업데이트 manifest URL. 지정한 경우 시작할 때 새 버전을 확인합니다.
    #[arg(long, requires = "release_public_key")]
    update_manifest_url: Option<String>,

    /// manifest를 검증할 IEUM 릴리스 Ed25519 공개키(32바이트 hex)
    #[arg(long, requires = "update_manifest_url")]
    release_public_key: Option<String>,

    /// 폐쇄형 개발망에서만 고정 검증자 키를 허용합니다.
    #[arg(long, default_value_t = false)]
    allow_insecure_test_keys: bool,

    /// 공개 개발키와 테스트 네트워크 이름이 없는 production genesis만 허용합니다.
    #[arg(long, default_value_t = false)]
    mainnet_strict: bool,

    /// 개인키를 노드 밖에 두는 signer 실행 파일(HSM/Vault adapter)
    #[arg(long, requires = "validator_public_key")]
    validator_signer_command: Option<PathBuf>,

    /// 외부 signer의 Ed25519 공개키 32바이트 hex
    #[arg(long, requires = "validator_signer_command")]
    validator_public_key: Option<String>,

    /// proposal 단계 제한 시간(ms)
    #[arg(long, default_value_t = 3_000)]
    propose_timeout_ms: u64,

    /// prevote 단계 제한 시간(ms)
    #[arg(long, default_value_t = 2_000)]
    prevote_timeout_ms: u64,

    /// precommit 단계 제한 시간(ms)
    #[arg(long, default_value_t = 2_000)]
    precommit_timeout_ms: u64,

    /// 동일 tip/state root 확인에 필요한 독립 피어 수(2~3)
    #[arg(long, default_value_t = 2, value_parser = parse_sync_quorum_peers)]
    sync_quorum_peers: usize,

    /// 서버/클라이언트가 시작할 때 접속할 추가 P2P 주소
    #[arg(long, help = "/dns4/node.ieum.aah.name/udp/7001/quic-v1/p2p/PeerId")]
    peer: Vec<Multiaddr>,

    /// CI·폐쇄형 개발망에서 내장 운영 bootstrap 접속을 비활성화합니다.
    #[arg(long, default_value_t = false)]
    no_default_bootstrap: bool,
}

#[derive(Debug, ClapArgs)]
struct ClientArgs {
    #[command(flatten)]
    node: NodeArgs,

    /// 자동으로 읽을 부트스트랩 설정 파일
    #[arg(long, default_value = DEFAULT_BOOTSTRAP_CONFIG)]
    bootstrap_config: PathBuf,
}

fn default_node_args() -> NodeArgs {
    NodeArgs {
        git_action_test: false,
        port: 7001,
        max_message_bytes: 2_097_152,
        block_time_ms: 5_000,
        rpc_port: 8989,
        rpc_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        rpc_data_dir: PathBuf::from("data/ledger"),
        node_key: PathBuf::from("data/node.key"),
        validator_index: 1,
        validator_key: PathBuf::from("data/keys/consensus_signing.key"),
        validators_config: PathBuf::from("config/validators.json"),
        events_config: PathBuf::from("config/events.json"),
        update_manifest_url: None,
        release_public_key: None,
        allow_insecure_test_keys: false,
        mainnet_strict: false,
        validator_signer_command: None,
        validator_public_key: None,
        propose_timeout_ms: 3_000,
        prevote_timeout_ms: 2_000,
        precommit_timeout_ms: 2_000,
        sync_quorum_peers: 2,
        peer: Vec::new(),
        no_default_bootstrap: false,
    }
}

fn automatic_or_selected_mode(
    requested: RunMode,
) -> Result<(&'static str, NodeArgs, Vec<Multiaddr>, bool), String> {
    let selected = if requested == RunMode::Auto {
        if approved_local_validator(
            Path::new("data/keys/consensus_signing.key"),
            Path::new("config/validators.json"),
        ) {
            log_info!(
                "[자동 역할 선택] 승인된 검증자 키를 확인했습니다. 검증자 모드로 시작합니다."
            );
            RunMode::Validator
        } else {
            log_info!(
                "[자동 역할 선택] 일반 노드로 시작합니다. AutoNAT 역접속에 성공하면 공개 지원 노드로 자동 판정됩니다."
            );
            RunMode::Client
        }
    } else {
        requested
    };

    let mut node = default_node_args();
    let is_client = selected != RunMode::Validator;
    let label = match selected {
        RunMode::Validator => {
            node.node_key = PathBuf::from("data/keys/p2p_identity.key");
            "검증자"
        }
        RunMode::Public => {
            node.node_key = PathBuf::from("data/public.node.key");
            node.rpc_data_dir = PathBuf::from("data/public-ledger");
            "공개 지원 후보"
        }
        RunMode::Client | RunMode::Auto => {
            node.node_key = PathBuf::from("data/client.node.key");
            node.rpc_data_dir = PathBuf::from("data/client-ledger");
            "일반"
        }
    };
    let peers = load_bootstrap_peers(Path::new(DEFAULT_BOOTSTRAP_CONFIG), Vec::new())?;
    Ok((label, node, peers, is_client))
}

fn approved_local_validator(key: &Path, validators_config: &Path) -> bool {
    if !key.exists() || !validators_config.exists() {
        return false;
    }
    let result = ieum_chain::validator_key::public_key_from_file(key).and_then(|public_key| {
        Ok(load_validators(validators_config)?
            .iter()
            .any(|validator| validator.id.eq_ignore_ascii_case(&public_key)))
    });
    match result {
        Ok(approved) => approved,
        Err(error) => {
            log_error!(
                "[자동 역할 판정 경고] 기존 검증자 설정을 확인하지 못해 일반 노드로 시작합니다: {error}"
            );
            false
        }
    }
}

fn parse_sync_quorum_peers(value: &str) -> Result<usize, String> {
    let peers = value
        .parse::<usize>()
        .map_err(|_| "sync quorum peers는 2 또는 3이어야 합니다".to_string())?;

    if (2..=3).contains(&peers) {
        Ok(peers)
    } else {
        Err("sync quorum peers는 2 또는 3이어야 합니다".to_string())
    }
}

fn parse_block_time_ms(value: &str) -> Result<u64, String> {
    let milliseconds = value
        .parse::<u64>()
        .map_err(|_| "블록 생성 간격은 밀리초 정수여야 합니다.".to_string())?;
    if !(100..=15_000).contains(&milliseconds) {
        return Err("블록 생성 간격은 100ms 이상 15000ms 이하여야 합니다.".into());
    }
    Ok(milliseconds)
}

#[tokio::main]
async fn main() -> Result<(), String> {
    use_binary_directory()?;
    migrate_legacy_key_files()?;
    let cli = Args::parse();
    if cli.command.is_some() && cli.mode != RunMode::Auto {
        return Err(
            "--mode는 하위 명령 없이 사용하세요. 기존 호환 명령은 `server` 또는 `client` 중 하나만 사용합니다."
                .into(),
        );
    }
    let (mode, mut args, bootstrap_peers, is_client) = match cli.command {
        Some(Command::ValidatorKey { command }) => return run_validator_key_command(command),
        Some(Command::Reward { command }) => return run_reward_command(command),
        Some(Command::Account { command }) => return run_account_command(command),
        Some(Command::Node { command }) => return run_node_command(command),
        Some(Command::Network { command }) => return run_network_command(command),
        Some(Command::Recovery { command }) => return run_recovery_command(command),
        Some(Command::Update {
            manifest_url,
            release_public_key,
        }) => {
            return match ieum_chain::updater::install_non_interactive(
                &manifest_url,
                &release_public_key,
            )? {
                ieum_chain::updater::UpdateResult::Current => {
                    println!("현재 버전이 최신입니다.");
                    Ok(())
                }
                ieum_chain::updater::UpdateResult::Installed => {
                    println!("서명된 업데이트를 설치했습니다.");
                    Ok(())
                }
            };
        }
        Some(Command::Server(mut args)) => {
            if args.git_action_test {
                log_info!(
                    "[테스트 모드] GitHub Actions용 격리 네트워크입니다. 고정 개발 키를 사용하며 실제 자산을 넣지 마세요."
                );
                args.rpc_host = IpAddr::V4(Ipv4Addr::LOCALHOST);
                args.allow_insecure_test_keys = true;
                args.no_default_bootstrap = true;
                args.propose_timeout_ms = 1_500;
                args.prevote_timeout_ms = 1_500;
                args.precommit_timeout_ms = 1_500;
                args.block_time_ms = 100;
            }
            if args.node_key == Path::new("data/node.key") {
                args.node_key = PathBuf::from("data/keys/p2p_identity.key");
            }
            let mut peers = std::mem::take(&mut args.peer);
            if peers.is_empty() && !args.no_default_bootstrap {
                peers = configured_bootstrap_peers()?;
            }
            ("서버", args, peers, false)
        }
        Some(Command::Client(mut client)) => {
            if client.node.git_action_test {
                return Err("--git_action_test는 server 명령에서만 사용할 수 있습니다.".into());
            }
            if client.node.node_key == Path::new("data/node.key") {
                client.node.node_key = PathBuf::from("data/client.node.key");
            }
            if client.node.rpc_data_dir == Path::new("data/ledger") {
                client.node.rpc_data_dir = PathBuf::from("data/client-ledger");
            }
            let peers = load_bootstrap_peers(
                &client.bootstrap_config,
                std::mem::take(&mut client.node.peer),
            )?;
            ("일반 PC", client.node, peers, true)
        }
        None => automatic_or_selected_mode(cli.mode)?,
    };
    // 서버는 P2P/RPC 포트 자체가 인스턴스 경계이므로 같은 장비에서 여러 검증자를
    // 실행할 수 있습니다. 일반 PC 클라이언트만 중복 실행을 차단합니다.
    let _instance_guard = if is_client {
        Some(acquire_instance_guard(true)?)
    } else {
        None
    };
    if !is_client {
        init_server_log("data/logs/ieum-chain.log")?;
        installation::prepare_server_files(
            &args.validator_key,
            &args.node_key,
            &args.rpc_data_dir,
            &args.validators_config,
            &args.events_config,
            Path::new("config/upgrades.json"),
            args.allow_insecure_test_keys,
        )?;
    }
    prepare_ports(&mut args, is_client)?;
    if let (Some(url), Some(public_key)) = (
        args.update_manifest_url.as_deref(),
        args.release_public_key.as_deref(),
    ) && let Err(error) = ieum_chain::updater::check_and_prompt(url, public_key, is_client)
    {
        log_error!("[업데이트 확인 실패] {error}. 현재 버전으로 계속 실행합니다.");
    }

    let identity_key = load_or_create_node_key(&args.node_key)?;
    let local_peer_id = libp2p::PeerId::from(identity_key.public());
    let reward_node_key = identity_key.clone();
    let external_addresses = repair_local_advertise_address(local_peer_id)?
        .advertise_address
        .into_iter()
        .map(|address| {
            let parsed = address
                .parse::<Multiaddr>()
                .map_err(|error| format!("공개 광고 주소 형식 오류({address}): {error}"))?;
            ensure_peer_id(parsed, local_peer_id)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bootstrap_peers = bootstrap_peers
        .into_iter()
        .filter(|address| multiaddr_peer_id(address).as_ref() != Some(&local_peer_id))
        .collect();
    let config = NetworkConfig {
        listen_port: args.port,
        loopback_only: args.git_action_test,
        bootstrap_peers,
        external_addresses,
        identity_key: Some(identity_key),
        max_message_bytes: args.max_message_bytes,
        // 릴레이 예약과 NAT 뒤 노드의 연결이 유휴 구간에도 유지되도록 한다.
        idle_timeout: Duration::from_secs(120),
        ban_duration: Duration::from_secs(10 * 60),
    };
    let startup_peers = config.bootstrap_peers.clone();
    let (peer_id, commands, mut events) = P2pNode::new(config).run().await?;

    let mut genesis: GenesisConfig =
        serde_json::from_str(include_str!("../config/genesis.json"))
            .map_err(|error| format!("운영망 제네시스 설정 오류: {error}"))?;
    if args.git_action_test {
        genesis.initial_balances.extend([
            (
                "0xB0E5863D0DDf7e105e409Fee0eCC0123a362e14B".into(),
                1_000_000_000_000_000_000,
            ),
            (
                "0x3252b7b65e50B54508974dB8d634134B0bd6be90".into(),
                1_000_000_000_000_000_000,
            ),
            (
                "0xf0DCB0Ea878057Ff5C78C4737023f900ECe09e7B".into(),
                1_000_000_000_000_000_000,
            ),
            (
                "0xD5ac7674AC15E3Df0B7D737CF8Cb8f2Ea713F329".into(),
                1_000_000_000_000_000_000,
            ),
        ]);
    }
    genesis.validate()?;
    if args.mainnet_strict {
        genesis.validate_production_safety()?;
        if args.allow_insecure_test_keys || args.git_action_test {
            return Err("--mainnet-strict는 테스트 키 옵션과 함께 사용할 수 없습니다.".into());
        }
    }
    let mut validators = load_validators(&args.validators_config)?;
    let rpc_config = RpcConfig {
        listen_ip: args.rpc_host,
        port: args.rpc_port,
        data_dir: args.rpc_data_dir.clone(),
        genesis: Some(genesis.clone()),
        validators: validators.clone(),
        locked_addresses: genesis.locked_addresses.clone(),
        block_time_ms: args.block_time_ms,
        ..RpcConfig::default()
    };
    let rpc_server = RpcServer::new(rpc_config);
    let rpc = rpc_server.node_handle();
    let mut rpc_task = tokio::spawn(rpc_server.run());
    if !is_client && validators.len() < 4 {
        log_info!(
            "[부트스트랩 합의] 현재 검증자 {}명입니다. 4명 이상 등록되기 전에는 \
             장애 허용 BFT가 아니라 개발·초기 구성 모드로 동작합니다.",
            validators.len()
        );
    }
    let local_validator: ValidatorSigner = if is_client {
        Wallet::new().into()
    } else if let (Some(command), Some(public_key)) = (
        args.validator_signer_command.as_ref(),
        args.validator_public_key.as_ref(),
    ) {
        ExternalSigner::new(command, public_key.clone())?.into()
    } else {
        load_validator_wallet(
            &args.validator_key,
            args.validator_index,
            args.allow_insecure_test_keys,
        )?
        .into()
    };
    let local_validator_address = local_validator.address();
    let mut local_is_validator = !is_client
        && validators
            .iter()
            .any(|validator| validator.id == local_validator_address);
    if !is_client {
        if local_is_validator {
            log_info!(
                "[합의 참여] 로컬 검증자 {}가 현재 검증자 집합에 등록되어 있습니다.",
                local_validator_address
            );
        } else {
            log_info!(
                "[일반 노드 시작] 로컬 검증자 {}는 아직 등록되지 않았습니다. \
                 P2P 동기화와 서명 후보 등록은 계속하며 합의 투표에는 참여하지 않습니다.",
                local_validator_address
            );
        }
    }
    let local_registration = if is_client {
        None
    } else {
        Some(ValidatorRegistration {
            validator_id: local_validator_address.clone(),
            peer_id: peer_id.to_string(),
            signature_hex: local_validator.sign_bytes(&ValidatorRegistration::bytes_to_sign(
                &local_validator_address,
                &peer_id.to_string(),
            ))?,
        })
    };
    let mut registrations = BTreeMap::new();
    if let Some(registration) = &local_registration {
        registrations.insert(registration.validator_id.clone(), registration.clone());
    }
    let reward_registration_wallet =
        load_or_create_reward_wallet(Path::new("data/keys/node_reward_signing.key"))?;
    let reward_wallet = NodeWalletKeystore::load_or_create_default(
        Path::new("data/keys/node_wallet.keystore"),
        Path::new("data/keys/node_wallet.password"),
    )?;
    let reward_registration_bytes =
        NodeRewardRegistration::bytes_to_sign(&reward_wallet.address(), &peer_id.to_string());
    let local_reward_registration = NodeRewardRegistration {
        reward_address: reward_wallet.address(),
        peer_id: peer_id.to_string(),
        signature_hex: reward_registration_wallet.sign_bytes(&reward_registration_bytes),
        registration_signer: reward_registration_wallet.address(),
        node_public_key_hex: hex::encode(reward_node_key.public().encode_protobuf()),
        node_signature_hex: hex::encode(
            reward_node_key
                .sign(&reward_registration_bytes)
                .map_err(|error| format!("노드 보상 등록 서명 실패: {error}"))?,
        ),
    };
    let mut node_reward_registrations = BTreeMap::new();
    node_reward_registrations.insert(
        local_reward_registration.peer_id.clone(),
        local_reward_registration.clone(),
    );
    let upgrades = UpgradeSchedule::load("config/upgrades.json")?;
    upgrades.ensure_supported(
        rpc.chain()?.tip_height().saturating_add(1),
        SUPPORTED_PROTOCOL_VERSION,
    )?;
    let mut consensus = ConsensusRuntime::with_signer(
        rpc.chain()?,
        validators.clone(),
        local_validator,
        ConsensusTimeouts {
            propose: Duration::from_millis(args.propose_timeout_ms),
            prevote: Duration::from_millis(args.prevote_timeout_ms),
            precommit: Duration::from_millis(args.precommit_timeout_ms),
        },
    )?;
    let event_schedule = EventSchedule::load(&args.events_config)?;
    consensus.set_event_schedule(event_schedule.clone())?;
    let finality_store = FinalityStore::new(&args.rpc_data_dir)?;
    let evidence_store = EvidenceStore::new(&args.rpc_data_dir);
    let evidence_count = evidence_store.load()?.len();
    if evidence_count > 0 {
        log_info!("[BFT 이중투표 증거 복원] {evidence_count}개");
    }
    let certificate_history = finality_store.load(&validators)?;
    for certificate in &certificate_history {
        rpc.record_finality(certificate.clone())?;
    }
    let imported = consensus.import_certificate_history(certificate_history)?;
    if imported > 0 {
        log_info!("[BFT 인증서 복원] {imported}개");
    }
    let mut consensus_tick = tokio::time::interval(Duration::from_millis(100));
    let mut snapshot_tick = tokio::time::interval(Duration::from_secs(30));
    let block_interval = Duration::from_millis(args.block_time_ms);
    let mut observed_tip_height = consensus.chain.tip_height();
    let mut next_block_at = std::time::Instant::now() + block_interval;
    // 비제안자 RPC로 들어온 거래는 제안자가 받을 때까지 주기적으로 재전파하되,
    // 매 consensus tick마다 같은 payload를 쏟아내지는 않습니다.
    let mut announced_transactions = std::collections::HashMap::<String, std::time::Instant>::new();
    // 같은 블록은 여러 피어에서 도착할 수 있습니다. 운영 로그에는 최초 수신만 남겨
    // 로그 폭증을 막되, 블록 검증과 합의 처리는 기존대로 모두 수행합니다.
    let mut logged_block_hashes = HashSet::<String>::new();
    let mut logged_block_order = VecDeque::<String>::new();
    // 거래가 없는 동안에는 빈 블록을 만들지 않는다. 첫 거래나 제안이 도착했을 때
    // 노드 시작 시점의 만료된 deadline을 쓰지 않도록 활성 전환을 추적한다.
    let mut consensus_was_active = false;
    // 연결 직후에도 별도로 전송하므로 주기 공지는 복구용 저빈도 heartbeat만 사용합니다.
    let mut registration_tick = tokio::time::interval(Duration::from_secs(60));
    let auto_update = ieum_chain::updater::AutoUpdateConfig::discover()?;
    if let Some((path, config)) = &auto_update {
        log_info!(
            "[자동 업데이트 활성] 설정: {} · 확인 주기: {}초 · 시작 즉시 확인",
            path.display(),
            config.check_interval_secs
        );
    } else {
        log_info!("[자동 업데이트 비활성] config/update.json을 찾지 못했거나 enabled=false입니다.");
    }
    let update_interval = auto_update
        .as_ref()
        .map(|(_, config)| config.check_interval_secs)
        .unwrap_or(24 * 60 * 60);
    let mut update_tick = tokio::time::interval(Duration::from_secs(update_interval));
    let mut sync_quorum = TipQuorum::new(args.sync_quorum_peers)?;
    let mut snapshot_votes = HashMap::<(u64, String, String), Vec<SnapshotAttestation>>::new();

    log_info!("IEUM {mode} 노드 시작: {peer_id}");
    log_info!("영구 노드 키: {}", args.node_key.display());
    log_info!("P2P 포트: {}/UDP", args.port);
    log_info!("RPC 주소: {}:{}", args.rpc_host, args.rpc_port);
    log_info!("원장 경로: {}", args.rpc_data_dir.display());
    log_info!("목표 블록 생성 간격: {}ms", args.block_time_ms);
    log_info!("노드 보상 주소: {}", reward_wallet.address());
    if is_client {
        log_info!("운영 서버 자동 연결 대상:");
        for peer in &startup_peers {
            log_info!("  - {peer}");
        }
    }
    log_info!("같은 LAN의 노드는 mDNS로 자동 검색합니다. 종료: Ctrl+C");

    loop {
        tokio::select! {
            _ = update_tick.tick(), if auto_update.is_some() => {
                let (_, config) = auto_update.as_ref().expect("guarded by is_some");
                log_info!("[자동 업데이트 확인] 서명된 최신 manifest를 확인합니다.");
                match ieum_chain::updater::install_if_newer(
                    &config.manifest_url,
                    &config.release_public_key,
                ) {
                    Ok(ieum_chain::updater::UpdateResult::Current) => {}
                    Ok(ieum_chain::updater::UpdateResult::Installed) => {
                        log_info!("[자동 업데이트 완료] 새 실행 파일을 설치했습니다. 서비스를 재시작합니다.");
                        return Ok(());
                    }
                    Err(error) => log_error!("[자동 업데이트 실패] {error}"),
                }
                commands.send(NetworkCommand::PublishUpdateAvailable {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                }).await.map_err(|error| error.to_string())?;
            }
            _ = registration_tick.tick() => {
                if !consensus
                    .chain
                    .executed_events()
                    .contains("ieum-bootstrap-validator-reward-v1")
                    && let Some(registration) = &local_registration
                {
                    commands
                        .send(NetworkCommand::PublishValidatorRegistration(
                            registration.clone(),
                        ))
                        .await
                        .map_err(|error| error.to_string())?;
                }
                commands
                    .send(NetworkCommand::PublishNodeRewardRegistration(
                        local_reward_registration.clone(),
                    ))
                    .await
                    .map_err(|error| error.to_string())?;
            }
            _ = snapshot_tick.tick() => {
                if local_is_validator
                    && let Some(snapshot) = rpc.pending_snapshot_certification()?
                {
                    let attestation = consensus.sign_snapshot_attestation(&snapshot)?;
                    let votes = snapshot_votes
                        .entry((attestation.height, attestation.block_hash.clone(), attestation.state_root.clone()))
                        .or_default();
                    if !votes.iter().any(|known| known.validator_id == attestation.validator_id) {
                        votes.push(attestation.clone());
                    }
                    commands
                        .send(NetworkCommand::PublishSnapshotAttestation(attestation))
                        .await
                        .map_err(|error| error.to_string())?;
                    log_info!("[snapshot 인증] 높이 {} 로컬 검증자 서명 전파", snapshot.height);
                }
            }
            _ = consensus_tick.tick() => {
                if consensus.chain.tip_height() != observed_tip_height {
                    observed_tip_height = consensus.chain.tip_height();
                    next_block_at = std::time::Instant::now() + block_interval;
                }
                for envelope in rpc.drain_outbound_communication()? {
                    commands
                        .send(NetworkCommand::SendCommunication(envelope))
                        .await
                        .map_err(|error| error.to_string())?;
                }
                let timed_out_transactions = consensus.pending_transactions();
                let consensus_is_active = local_is_validator
                    && (consensus.phase() != ieum_chain::ConsensusPhase::Propose
                        || rpc.has_pending_transactions()?);
                let timeout_now = std::time::Instant::now();
                if consensus_is_active && !consensus_was_active {
                    consensus.restart_phase_timeout(timeout_now);
                }
                if consensus_is_active
                    && consensus.timeout_if_due(timeout_now)?
                {
                    rpc.restore_transactions(timed_out_transactions)?;
                    print!(
                        "\r\x1b[2K[BFT 라운드 변경] 단계별 제한 시간 초과, 새 라운드 {}",
                        consensus.round()
                    );
                    io::stdout().flush().map_err(|error| error.to_string())?;
                }
                consensus_was_active = consensus_is_active;
                if !is_client && local_is_validator {
                    upgrades.ensure_supported(
                        consensus.chain.tip_height().saturating_add(1),
                        SUPPORTED_PROTOCOL_VERSION,
                    )?;
                    // RPC가 어느 노드로 들어와도 현재 제안자가 받을 수 있게 읽기 전용으로 전파합니다.
                    // 비제안자는 drain_transactions를 호출하지 않으므로 원본 거래는 그대로 보존됩니다.
                    if !consensus.can_make_proposal() {
                        let now = std::time::Instant::now();
                        for transaction in rpc.pending_transactions_snapshot(1_000)? {
                            let transaction_id = transaction.id();
                            let should_publish = announced_transactions
                                .get(&transaction_id)
                                .is_none_or(|last| now.duration_since(*last) >= Duration::from_secs(2));
                            if !should_publish {
                                continue;
                            }
                            commands
                                .send(NetworkCommand::PublishTransaction(transaction))
                                .await
                                .map_err(|error| error.to_string())?;
                            announced_transactions.insert(transaction_id, now);
                        }
                    }
                    let timestamp = unix_timestamp();
                    let mut due_events =
                        event_schedule.due(timestamp, consensus.chain.executed_events());
                    if !consensus
                        .chain
                        .executed_events()
                        .contains("ieum-bootstrap-validator-reward-v1")
                    {
                        let validator_proofs: Vec<_> = validators
                            .iter()
                            .filter_map(|validator| registrations.get(&validator.id).cloned())
                            .collect();
                        if validator_proofs.len() == 4 {
                            due_events.push(ScheduledEvent {
                                id: "ieum-bootstrap-validator-reward-v1".into(),
                                execute_at: timestamp,
                                action: ScheduledEventAction::BootstrapValidatorReward {
                                    registrations: validator_proofs,
                                    amount: 10 * 10u128.pow(18),
                                },
                            });
                        }
                    }
                    if !consensus
                        .chain
                        .executed_events()
                        .contains("ieum-node-100-reward-v1")
                        && node_reward_registrations.len() >= 100
                    {
                        due_events.push(ScheduledEvent {
                            id: "ieum-node-100-reward-v1".into(),
                            execute_at: timestamp,
                            action: ScheduledEventAction::NodeMilestoneReward {
                                registrations: node_reward_registrations
                                    .values()
                                    .take(100)
                                    .cloned()
                                    .collect(),
                                amount: 10u128.pow(18),
                            },
                        });
                    }
                    if consensus.can_make_proposal()
                        && std::time::Instant::now() >= next_block_at
                        && (rpc.has_pending_transactions()? || !due_events.is_empty())
                    {
                        // 제안자만 거래 큐를 비웁니다. 비제안자 RPC에 들어온 거래를 보존합니다.
                        let pending = rpc.drain_transactions(1_000)?;
                        let previous = consensus.chain.blocks.last().unwrap();
                        let block = ieum_chain::Block::new(
                            previous.height + 1,
                            previous.hash.clone(),
                            timestamp,
                            local_validator_address.clone(),
                            pending.clone(),
                        )
                        .with_system_events(due_events);
                        match consensus.make_proposal(block) {
                            Ok(proposal) => {
                                next_block_at = std::time::Instant::now() + block_interval;
                                let prevote = match consensus.receive_proposal(proposal.clone()) {
                                    Ok(prevote) => prevote,
                                    Err(error) => {
                                        log_error!(
                                            "[BFT 로컬 제안 보류] 합의 단계가 변경되어 다음 tick에서 재시도합니다: {error}"
                                        );
                                        rpc.restore_transactions(pending)?;
                                        continue;
                                    }
                                };
                                commands.send(NetworkCommand::PublishProposal(proposal)).await.map_err(|e| e.to_string())?;
                                commands.send(NetworkCommand::PublishConsensus(prevote.clone())).await.map_err(|e| e.to_string())?;
                                if let Some(precommit) = consensus.receive_vote(prevote)? {
                                    commands.send(NetworkCommand::PublishConsensus(precommit.clone())).await.map_err(|e| e.to_string())?;
                                    consensus.receive_vote(precommit)?;
                                }
                                finalize_if_ready(&mut consensus, &rpc, &commands, &finality_store).await?;
                            }
                            Err(_) => {
                                for transaction in &pending {
                                    commands.send(NetworkCommand::PublishTransaction(transaction.clone())).await.map_err(|e| e.to_string())?;
                                }
                                rpc.restore_transactions(pending)?;
                            }
                        }
                    }
                }
            }
            result = &mut rpc_task => {
                return match result {
                    Ok(Ok(())) => Err("JSON-RPC 서버가 예기치 않게 종료되었습니다.".into()),
                    Ok(Err(message)) => Err(message),
                    Err(error) => Err(format!("JSON-RPC 작업 실행 실패: {error}")),
                };
            }
            event = events.recv() => {
                match event {
                    Some(NetworkEvent::PeerConnected { peer_id: connected, remote_address, remote_ip, direction, connection_id, unique_peers, peer_connections }) => {
                        rpc.peer_connected(
                            &connected.to_string(),
                            &remote_address.to_string(),
                            remote_ip.as_deref(),
                            direction,
                            peer_connections,
                        )?;
                        log_info!("{}", NetworkEvent::PeerConnected { peer_id: connected, remote_address, remote_ip, direction, connection_id, unique_peers, peer_connections });
                        commands.send(NetworkCommand::RequestSync {
                            from_height: consensus.chain.tip_height() + 1,
                        }).await.map_err(|e| e.to_string())?;
                        if let Some(registration) = &local_registration {
                            commands.send(NetworkCommand::PublishValidatorRegistration(registration.clone()))
                                .await.map_err(|e| e.to_string())?;
                        }
                        commands.send(NetworkCommand::PublishNodeRewardRegistration(
                            local_reward_registration.clone(),
                        )).await.map_err(|e| e.to_string())?;
                        if auto_update.is_some() {
                            commands.send(NetworkCommand::PublishUpdateAvailable {
                                version: env!("CARGO_PKG_VERSION").to_string(),
                            }).await.map_err(|error| error.to_string())?;
                        }
                    }
                    Some(NetworkEvent::ValidatorRegistrationReceived { source, registration }) if !is_client => {
                        ieum_chain::logger::write_repeated_info(&format!(
                            "[검증자 등록 수신] PeerId: {source}, 검증자: {}",
                            registration.validator_id
                        ));
                        if let Err(error) = verify_validator_registration(&registration) {
                            log_error!("[검증자 자동 등록 거부] {error}");
                            continue;
                        }
                        let registration_id = registration.validator_id.clone();
                        let is_new = registrations
                            .insert(registration_id.clone(), registration)
                            .is_none();
                        if is_new {
                            log_info!(
                                "[검증자 자동 등록] 확인 {}/4명",
                                registrations.len().min(4)
                            );
                        }
                        if registrations.len() >= 4 && consensus.chain.tip_height() == 0 {
                            let selected: Vec<_> = registrations
                                .keys()
                                .take(4)
                                .map(|id| Validator::new(id.clone(), 100))
                                .collect();
                            let mut current_ids: Vec<_> =
                                validators.iter().map(|validator| validator.id.clone()).collect();
                            current_ids.sort();
                            let selected_ids: Vec<_> =
                                selected.iter().map(|validator| validator.id.clone()).collect();
                            if current_ids != selected_ids {
                                consensus.replace_bootstrap_validators(selected.clone())?;
                                save_validators(&args.validators_config, &selected)?;
                                validators = selected;
                                local_is_validator = validators
                                    .iter()
                                    .any(|validator| validator.id == local_validator_address);
                                println!();
                                log_info!(
                                    "[BFT 합의 시작] 검증자 4명 자동 등록 완료. 공통 검증자 집합으로 전환했습니다."
                                );
                            }
                        } else if !validators
                            .iter()
                            .any(|validator| validator.id == registration_id)
                        {
                            log_info!(
                                "[검증자 후보 대기] {} · P2P 접속과 키 소유권 확인 완료. \
                                 현재 검증자 승인 및 다음 epoch 적용 전까지 합의권을 부여하지 않습니다.",
                                registration_id
                            );
                        }
                    }
                    Some(NetworkEvent::NodeRewardRegistrationReceived { source, registration }) => {
                        if source.to_string() != registration.peer_id {
                            log_error!("[노드 보상 등록 거부] 전파 PeerId와 등록 PeerId가 다릅니다.");
                            continue;
                        }
                        if let Err(error) = ieum_chain::wallet::verify_signature(
                            registration.registration_signer(),
                            &NodeRewardRegistration::bytes_to_sign(
                                &registration.reward_address,
                                &registration.peer_id,
                            ),
                            &registration.signature_hex,
                        ) {
                            log_error!("[노드 보상 등록 거부] {error}");
                            continue;
                        }
                        if let Err(error) = registration.verify_node_identity() {
                            log_error!("[노드 보상 등록 거부] {error}");
                            continue;
                        }
                        let is_new = node_reward_registrations
                            .insert(registration.peer_id.clone(), registration)
                            .is_none();
                        if is_new {
                            log_info!(
                                "[노드 보상 등록] 서로 다른 노드 {}/100개 확인",
                                node_reward_registrations.len().min(100)
                            );
                        }
                    }
                    Some(NetworkEvent::UpdateAvailableReceived { source, version }) => {
                        let Some((_, config)) = auto_update.as_ref() else {
                            continue;
                        };
                        if !ieum_chain::updater::is_newer(env!("CARGO_PKG_VERSION"), &version)
                            .unwrap_or(false)
                        {
                            continue;
                        }
                        log_info!(
                            "[P2P 업데이트 알림] PeerId: {source}, 버전: {version}. \
                             로컬 릴리스 공개키로 재검증합니다."
                        );
                        match ieum_chain::updater::install_if_newer(
                            &config.manifest_url,
                            &config.release_public_key,
                        ) {
                            Ok(ieum_chain::updater::UpdateResult::Current) => {}
                            Ok(ieum_chain::updater::UpdateResult::Installed) => {
                                log_info!(
                                    "[P2P 자동 업데이트 완료] 새 실행 파일을 설치했습니다. 서비스를 재시작합니다."
                                );
                                return Ok(());
                            }
                            Err(error) => log_error!("[P2P 자동 업데이트 거부] {error}"),
                        }
                    }
                    Some(NetworkEvent::TransactionReceived { transaction, .. }) => {
                        rpc.restore_transactions(vec![transaction])?;
                    }
                    Some(NetworkEvent::BlockReceived { source, block }) => {
                        if logged_block_hashes.insert(block.hash.clone()) {
                            let block_hash = block.hash.clone();
                            log_info!(
                                "{}",
                                NetworkEvent::BlockReceived {
                                    source,
                                    block,
                                }
                            );
                            logged_block_order.push_back(block_hash);
                            if logged_block_order.len() > 4_096
                                && let Some(expired) = logged_block_order.pop_front()
                            {
                                logged_block_hashes.remove(&expired);
                            }
                        }
                    }
                    Some(NetworkEvent::CommunicationReceived { envelope, .. }) => {
                        rpc.receive_communication(envelope, unix_timestamp())?;
                    }
                    Some(NetworkEvent::ProposalReceived { proposal, .. }) if !is_client && local_is_validator => {
                        match consensus.receive_proposal(proposal) {
                            Ok(prevote) => {
                                commands.send(NetworkCommand::PublishConsensus(prevote.clone())).await.map_err(|e| e.to_string())?;
                                if let Some(precommit) = consensus.receive_vote(prevote)? {
                                    commands.send(NetworkCommand::PublishConsensus(precommit.clone())).await.map_err(|e| e.to_string())?;
                                    consensus.receive_vote(precommit)?;
                                }
                                publish_replayed_votes(&mut consensus, &commands).await?;
                                finalize_if_ready(&mut consensus, &rpc, &commands, &finality_store).await?;
                            }
                            Err(error) => log_error!("[BFT 제안 거부] {error}"),
                        }
                    }
                    Some(NetworkEvent::ConsensusReceived { message, .. }) if !is_client && local_is_validator => {
                        match consensus.receive_vote(message.clone()) {
                            Ok(Some(precommit)) => {
                                commands.send(NetworkCommand::PublishConsensus(precommit.clone())).await.map_err(|e| e.to_string())?;
                                consensus.receive_vote(precommit)?;
                            }
                            Ok(None) => {}
                            Err(error) if ieum_chain::consensus_runtime::is_deferable_vote_error(&error) => {
                                consensus.defer_vote(message);
                                ieum_chain::logger::write_repeated_info(
                                    "[BFT 투표 보류] 제안 또는 이전 합의 단계를 기다립니다."
                                );
                            }
                            Err(error) => log_error!("[BFT 투표 거부] {error}"),
                        }
                        publish_replayed_votes(&mut consensus, &commands).await?;
                        persist_and_publish_evidence(
                            &mut consensus,
                            &evidence_store,
                            &commands,
                        ).await?;
                        finalize_if_ready(&mut consensus, &rpc, &commands, &finality_store).await?;
                    }
                    Some(NetworkEvent::EvidenceReceived { evidence, .. }) => {
                        let registered = validators
                            .iter()
                            .any(|validator| validator.id == evidence.first.validator_id);
                        if !registered {
                            log_error!("[BFT 이중투표 증거 거부] 등록되지 않은 검증자");
                        } else if evidence_store.append(&evidence)? {
                            log_error!("[BFT 이중투표 증거 저장] {}", evidence.id());
                        }
                    }
                    Some(NetworkEvent::SyncRequested { requester, from_height, .. }) if !is_client => {
                        commands.send(NetworkCommand::RespondSync {
                            requester,
                            tip: SyncTip {
                                height: consensus.chain.tip_height(),
                                block_hash: consensus.chain.tip_hash().to_string(),
                                state_root: consensus.chain.state_hash(),
                            },
                            certificates: consensus.certificates_from(from_height),
                        }).await.map_err(|e| e.to_string())?;
                    }
                    Some(NetworkEvent::SyncReceived { source, tip, certificates }) => {
                        let Some(agreed_tip) = sync_quorum.observe(source.to_string(), tip) else {
                            log_info!("[동기화 교차검증] 두 번째 독립 피어 응답을 기다립니다.");
                            continue;
                        };
                        rpc.begin_sync(agreed_tip.height)?;
                        let mut applied = 0;
                        for certificate in certificates {
                            let chain_before = consensus.chain.clone();
                            let block = certificate.block.clone();
                            if consensus.apply_sync_certificates(vec![certificate.clone()])? == 1 {
                                rpc.install_finalized(
                                    &chain_before,
                                    consensus.chain.clone(),
                                    &block,
                                )?;
                                finality_store.append(&certificate)?;
                                applied += 1;
                            }
                        }
                        if applied > 0 {
                            if consensus.chain.tip_height() == agreed_tip.height
                                && (consensus.chain.tip_hash() != agreed_tip.block_hash
                                    || consensus.chain.state_hash() != agreed_tip.state_root)
                            {
                                return Err("동기화 완료 상태가 피어 quorum의 tip/state root와 다릅니다.".into());
                            }
                            log_info!("[동기화 완료] 확정 블록 {applied}개 적용, 높이 {}", consensus.chain.tip_height());
                            commands.send(NetworkCommand::RequestSync {
                                from_height: consensus.chain.tip_height() + 1,
                            }).await.map_err(|e| e.to_string())?;
                        } else if consensus.chain.tip_height() < agreed_tip.height {
                            commands.send(NetworkCommand::RequestSync {
                                from_height: consensus.chain.tip_height() + 1,
                            }).await.map_err(|e| e.to_string())?;
                        }
                    }
                    Some(NetworkEvent::SnapshotAttestationReceived { source, attestation }) => {
                        if let Err(error) = attestation.verify() {
                            log_error!("[snapshot 투표 거부] PeerId: {source} · {error}");
                            commands.send(NetworkCommand::PenalizePeer { peer_id: source, points: 100 })
                                .await.map_err(|error| error.to_string())?;
                            continue;
                        }
                        let key = (attestation.height, attestation.block_hash.clone(), attestation.state_root.clone());
                        let votes = snapshot_votes.entry(key).or_default();
                        if !votes.iter().any(|known| known.validator_id == attestation.validator_id) {
                            votes.push(attestation);
                        }
                        let certificate = SnapshotCertificate::from_attestations(votes.clone())?;
                        match certificate.verify(&validators) {
                            Ok(()) => {
                                let height = certificate.height;
                                rpc.certify_snapshot(certificate)?;
                                snapshot_votes.retain(|(candidate, _, _), _| *candidate > height);
                                log_info!("[snapshot 인증 완료] 높이 {height} · 검증자 2/3 초과 서명 저장");
                            }
                            Err(error) if error.contains("2/3 초과") => {}
                            Err(error) => {
                                log_error!("[snapshot 인증서 거부] PeerId: {source} · {error}");
                                commands.send(NetworkCommand::PenalizePeer { peer_id: source, points: 100 })
                                    .await.map_err(|error| error.to_string())?;
                            }
                        }
                    }
                    Some(NetworkEvent::PeerDisconnected { peer_id, remote_address, remote_ip, direction, connection_id, connected_for, unique_peers, peer_connections, cause }) => {
                        rpc.peer_disconnected(&peer_id.to_string(), peer_connections)?;
                        log_info!("{}", NetworkEvent::PeerDisconnected {
                            peer_id,
                            remote_address,
                            remote_ip,
                            direction,
                            connection_id,
                            connected_for,
                            unique_peers,
                            peer_connections,
                            cause,
                        });
                    }
                    Some(NetworkEvent::OutgoingConnectionFailed { peer_id, error, .. }) => {
                        let peer = peer_id
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "확인 불가".into());
                        let guidance = if error.contains("Unexpected peer ID") {
                            " · 이 주소는 기대한 PeerId가 아닌 다른 노드입니다. 학습된 로컬/사설 주소는 제거되며, bootstrap 주소에서도 반복되면 bootstrap.json과 운영 server.node.key를 확인하세요."
                        } else {
                            ""
                        };
                        ieum_chain::logger::write_repeated_error(
                            &format!("[P2P 접속 실패] PeerId: {peer} · 오류: {error}{guidance}"),
                        );
                    }
                    Some(event) => log_info!("{event}"),
                    None => {
                        rpc_task.abort();
                        return Err("P2P 네트워크 이벤트 채널이 종료되었습니다.".into());
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!();
                log_info!("노드를 안전하게 종료합니다.");
                rpc_task.abort();
                break;
            }
        }
    }
    Ok(())
}

fn run_recovery_command(command: RecoveryCommand) -> Result<(), String> {
    match command {
        RecoveryCommand::Policy => {
            println!("[IEUM Chain 사고 복구 원칙]");
            println!("거래 단위 복구: 해킹·오발행처럼 영향 거래가 명확할 때 사용");
            println!("체크포인트 롤백: 원장 전체가 손상되거나 합의 버그가 발생했을 때만 사용");
            println!("승인 조건: 등록 검증자 수 3/4 이상 또는 전체 투표권 3/4 이상");
            println!();
            println!("확정된 거래나 블록을 직접 삭제하지 마세요.");
            println!("거래 단위 복구는 원본 이력을 보존한 승인된 보상 기록으로 처리합니다.");
            println!("체크포인트 롤백은 서비스 중지, 전체 백업, 검증자 합의 후에만 수행합니다.");
        }
    }
    Ok(())
}

fn run_validator_key_command(command: ValidatorKeyCommand) -> Result<(), String> {
    match command {
        ValidatorKeyCommand::Generate { output } => {
            let public_key = ieum_chain::validator_key::generate_key_file(&output)?;
            println!("검증자 개인키 생성 완료: {}", output.display());
            println!("공개키: {public_key}");
            println!("주의: 개인키 파일을 Git에 커밋하거나 다른 서버와 공유하지 마세요.");
        }
        ValidatorKeyCommand::Public { key } => {
            println!("{}", ieum_chain::validator_key::public_key_from_file(&key)?);
        }
        ValidatorKeyCommand::Transfer {
            to,
            amount,
            fee,
            key,
            rpc_port,
        } => {
            let wallet = ieum_chain::validator_key::wallet_from_file(&key)?;
            send_wallet_balance(&wallet, to, amount, fee, rpc_port)?;
        }
        ValidatorKeyCommand::CreateConfig {
            public_keys,
            voting_power,
            output,
        } => {
            ieum_chain::validator_key::create_validators_config(
                &output,
                &public_keys,
                voting_power,
            )?;
            println!("운영 검증자 설정 생성 완료: {}", output.display());
            println!("검증자 수: {}", public_keys.len());
        }
    }
    Ok(())
}

fn run_node_command(command: NodeCommand) -> Result<(), String> {
    let validator_key = Path::new("data/keys/consensus_signing.key");
    let node_key = Path::new("data/keys/p2p_identity.key");
    let ledger_dir = Path::new("data/ledger");
    match command {
        NodeCommand::Init { new: true } => {
            let backup = installation::initialize_new_server_node(
                validator_key,
                node_key,
                ledger_dir,
                Path::new("config/validators.json"),
                Path::new("config/events.json"),
                Path::new("config/upgrades.json"),
            )?;
            if let Some(path) = backup {
                println!("[기존 노드 자동 백업] {}", path.display());
            }
            println!("신규 노드 초기화가 완료되었습니다. 다음 명령으로 실행하세요:");
            println!("  ieum-chain server");
            Ok(())
        }
        NodeCommand::Init { new: false } => unreachable!("--new는 필수 옵션입니다."),
        NodeCommand::Verify => {
            let (validator_public_key, peer_id) =
                installation::verify_server_node(validator_key, node_key, ledger_dir)?;
            println!("[노드 검증 완료] validator 공개키: {validator_public_key}");
            println!("[노드 검증 완료] PeerId: {peer_id}");
            println!("원장과 서버 키가 최초 초기화 기록과 일치합니다.");
            Ok(())
        }
        NodeCommand::Doctor => {
            installation::prepare_server_files(
                validator_key,
                node_key,
                ledger_dir,
                Path::new("config/validators.json"),
                Path::new("config/events.json"),
                Path::new("config/upgrades.json"),
                false,
            )?;
            let identity = load_or_create_node_key(node_key)?;
            let peer_id = libp2p::PeerId::from(identity.public());
            repair_local_advertise_address(peer_id)?;
            let validator_public_key =
                ieum_chain::validator_key::public_key_from_file(validator_key)?;
            println!("[자동 복구 완료] 검증자 공개키: {validator_public_key}");
            println!("[자동 복구 완료] PeerId: {peer_id}");
            println!("이제 `ieum-chain server` 또는 systemd 서비스를 다시 시작하세요.");
            Ok(())
        }
        NodeCommand::PeerId { key } => {
            let identity = load_or_create_node_key(&key)?;
            println!("{}", libp2p::PeerId::from(identity.public()));
            Ok(())
        }
        NodeCommand::Clean { yes: true } => {
            let backup = installation::clean_ledger_preserving_identity(ledger_dir)?;
            println!("[원장 안전 백업] {}", backup.display());
            println!("합의 서명 키와 P2P 식별 키는 보존했습니다.");
            println!("다음 실행 시 네트워크에서 원장을 자동으로 다시 동기화합니다.");
            Ok(())
        }
        NodeCommand::Clean { yes: false } => unreachable!("--yes는 필수 옵션입니다."),
    }
}

#[cfg(test)]
mod cli_tests {
    use super::{parse_block_time_ms, parse_sync_quorum_peers};

    #[test]
    fn block_time_accepts_100ms_through_15s() {
        assert_eq!(parse_block_time_ms("100"), Ok(100));
        assert_eq!(parse_block_time_ms("5000"), Ok(5000));
        assert_eq!(parse_block_time_ms("15000"), Ok(15000));
        assert!(parse_block_time_ms("99").is_err());
        assert!(parse_block_time_ms("15001").is_err());
    }

    #[test]
    fn sync_quorum_peers_accepts_two_or_three() {
        assert_eq!(parse_sync_quorum_peers("2"), Ok(2));
        assert_eq!(parse_sync_quorum_peers("3"), Ok(3));
    }

    #[test]
    fn sync_quorum_peers_rejects_values_outside_range() {
        assert!(parse_sync_quorum_peers("1").is_err());
        assert!(parse_sync_quorum_peers("4").is_err());
        assert!(parse_sync_quorum_peers("invalid").is_err());
    }
}

async fn persist_and_publish_evidence(
    consensus: &mut ConsensusRuntime,
    evidence_store: &EvidenceStore,
    commands: &tokio::sync::mpsc::Sender<NetworkCommand>,
) -> Result<(), String> {
    for evidence in consensus.take_evidence() {
        if evidence_store.append(&evidence)? {
            log_error!("[BFT 이중투표 증거 생성] {}", evidence.id());
            commands
                .send(NetworkCommand::PublishEvidence(evidence))
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn publish_replayed_votes(
    consensus: &mut ConsensusRuntime,
    commands: &tokio::sync::mpsc::Sender<NetworkCommand>,
) -> Result<(), String> {
    for vote in consensus.replay_deferred_votes()? {
        commands
            .send(NetworkCommand::PublishConsensus(vote))
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn finalize_if_ready(
    consensus: &mut ConsensusRuntime,
    rpc: &ieum_chain::rpc::RpcNodeHandle,
    commands: &tokio::sync::mpsc::Sender<NetworkCommand>,
    finality_store: &FinalityStore,
) -> Result<(), String> {
    let certificates = consensus.take_finalized();
    if certificates.is_empty() {
        return Ok(());
    }
    for certificate in certificates {
        let chain_before = rpc.chain()?;
        let chain_after = consensus.chain.clone();
        rpc.install_finalized(&chain_before, chain_after, &certificate.block)?;
        finality_store.append(&certificate)?;
        rpc.record_finality(certificate.clone())?;
        commands
            .send(NetworkCommand::PublishBlock(certificate.block.clone()))
            .await
            .map_err(|error| error.to_string())?;
        log_info!(
            "[BFT 확정] 높이 {}, 해시 {}, precommit {}개",
            certificate.block.height,
            certificate.block.hash,
            certificate.precommits.len()
        );
    }
    consensus.advance_after_finalization()
}

fn testnet_validator_seed(index: u8) -> [u8; 32] {
    [index; 32]
}

/// 기존 설치의 키 내용을 바꾸지 않고 역할이 드러나는 새 경로로 한 번만 이전합니다.
/// 키를 재생성하면 PeerId나 검증자 신원이 달라지므로 반드시 파일 자체를 이동합니다.
fn migrate_legacy_key_files() -> Result<(), String> {
    for (legacy, current, label) in [
        (
            Path::new("data/server.node.key"),
            Path::new("data/keys/p2p_identity.key"),
            "P2P 식별 키",
        ),
        (
            Path::new("config/validator.key"),
            Path::new("data/keys/consensus_signing.key"),
            "합의 서명 키",
        ),
        (
            Path::new("data/reward.key"),
            Path::new("data/keys/node_reward_signing.key"),
            "노드 보상 서명 키",
        ),
    ] {
        if current.exists() || !legacy.exists() {
            continue;
        }
        if let Some(parent) = current.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("{label} 디렉터리 생성 실패: {error}"))?;
        }
        fs::rename(legacy, current).map_err(|error| {
            format!(
                "{label} 자동 이전 실패({} -> {}): {error}",
                legacy.display(),
                current.display()
            )
        })?;
        set_private_file_permissions(current)?;
        log_info!(
            "[키 경로 자동 이전] {label}: {} -> {}",
            legacy.display(),
            current.display()
        );
    }
    Ok(())
}

fn load_or_create_reward_wallet(path: &Path) -> Result<Wallet, String> {
    if path.exists() {
        let value = fs::read_to_string(path)
            .map_err(|error| format!("노드 보상 키를 읽지 못했습니다: {error}"))?;
        let seed: [u8; 32] = hex::decode(value.trim().trim_start_matches("0x"))
            .map_err(|_| "노드 보상 키가 hex 문자열이 아닙니다.".to_string())?
            .try_into()
            .map_err(|_| "노드 보상 키는 정확히 32바이트여야 합니다.".to_string())?;
        return Ok(Wallet::from_seed(seed));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    fs::write(path, hex::encode(seed))
        .map_err(|error| format!("노드 보상 키를 저장하지 못했습니다: {error}"))?;
    set_private_file_permissions(path)?;
    Ok(Wallet::from_seed(seed))
}

fn run_reward_command(command: RewardCommand) -> Result<(), String> {
    match command {
        RewardCommand::Address {
            keystore,
            password_file,
        } => {
            println!(
                "{}",
                NodeWalletKeystore::load_or_create_default(&keystore, &password_file)?.address()
            );
            Ok(())
        }
        RewardCommand::Send {
            to,
            amount,
            fee,
            keystore,
            password_file,
            rpc_port,
        } => {
            let wallet = NodeWalletKeystore::load_or_create_default(&keystore, &password_file)?;
            send_wallet_balance(&wallet, to, amount, fee, rpc_port)
        }
        RewardCommand::ChangePassword {
            keystore,
            password_file,
            new_password_file,
        } => {
            let old_password = fs::read_to_string(&password_file)
                .map_err(|error| format!("현재 노드 지갑 암호 읽기 실패: {error}"))?;
            let new_password = fs::read_to_string(&new_password_file)
                .map_err(|error| format!("새 노드 지갑 암호 읽기 실패: {error}"))?;
            let old_password = old_password.trim();
            let new_password = new_password.trim();
            NodeWalletKeystore::change_password(&keystore, old_password, new_password)?;
            if let Err(error) = atomic_private_write(&password_file, new_password.as_bytes()) {
                let _ = NodeWalletKeystore::change_password(&keystore, new_password, old_password);
                return Err(format!(
                    "암호 파일 교체 실패로 keystore를 기존 암호로 복원했습니다: {error}"
                ));
            }
            println!("노드 지갑 주소를 유지한 채 암호를 변경했습니다.");
            Ok(())
        }
    }
}

fn run_account_command(command: AccountCommand) -> Result<(), String> {
    match command {
        AccountCommand::New {
            keystore_dir,
            password_file,
        } => {
            let password = load_or_create_account_password(&password_file)?;
            let keystore = Keystore::new(keystore_dir)?;
            let wallet = AccountWallet::new();
            println!("{}", keystore.store(&wallet, password.trim())?);
            Ok(())
        }
        AccountCommand::List { keystore_dir } => {
            for address in Keystore::new(keystore_dir)?.addresses()? {
                println!("{address}");
            }
            Ok(())
        }
        AccountCommand::Import {
            key_file,
            keystore_dir,
            password_file,
        } => {
            let private_key = fs::read_to_string(&key_file)
                .map_err(|error| format!("개인키 파일 읽기 실패: {error}"))?;
            let password = load_or_create_account_password(&password_file)?;
            let wallet = AccountWallet::from_private_key_hex(private_key.trim())?;
            println!(
                "{}",
                Keystore::new(keystore_dir)?.store(&wallet, password.trim())?
            );
            Ok(())
        }
        AccountCommand::Send {
            from,
            to,
            amount,
            fee,
            keystore_dir,
            password_file,
            rpc_port,
        } => {
            let password = fs::read_to_string(&password_file).map_err(|error| {
                format!(
                    "계정 암호 파일 읽기 실패({}): {error}. 계정을 만든 인스턴스의 암호 파일인지 확인하세요.",
                    password_file.display()
                )
            })?;
            let wallet = Keystore::new(keystore_dir)?.load(&from, password.trim())?;
            send_wallet_balance(&wallet, to, amount, fee, rpc_port)
        }
        AccountCommand::Balance { address, rpc_port } => {
            validate_account_address(&address)?;
            let response = rpc_call(
                rpc_port,
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "eth_getBalance",
                    "params": [address, "latest"]
                }),
            )?;
            let value = parse_rpc_quantity(&response, "잔액")?;
            println!("{} IEUM ({value} wei)", format_ieum_amount(value));
            Ok(())
        }
        AccountCommand::Transaction { hash, rpc_port } => {
            print_rpc_lookup(rpc_port, "eth_getTransactionByHash", &hash)
        }
        AccountCommand::Receipt { hash, rpc_port } => {
            print_rpc_lookup(rpc_port, "eth_getTransactionReceipt", &hash)
        }
    }
}

/// 모든 상대 경로를 호출한 셸의 cwd가 아니라 현재 바이너리 인스턴스 기준으로
/// 해석합니다. 한 서버의 /opt/ieum-node1, node2, node3가 서로의 키와 설정을
/// 공유하거나 다른 바이너리를 업데이트하지 않게 하는 인스턴스 경계입니다.
fn use_binary_directory() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("현재 실행 바이너리 경로 확인 실패: {error}"))?;
    let directory = executable
        .parent()
        .ok_or("현재 실행 바이너리의 상위 폴더를 확인할 수 없습니다.")?
        .to_path_buf();
    std::env::set_current_dir(&directory).map_err(|error| {
        format!(
            "바이너리 기준 폴더로 이동할 수 없습니다({}): {error}",
            directory.display()
        )
    })?;
    Ok(directory)
}

fn load_or_create_account_password(path: &Path) -> Result<String, String> {
    if path.exists() {
        return fs::read_to_string(path)
            .map(|value| value.trim().to_string())
            .map_err(|error| format!("계정 암호 파일 읽기 실패({}): {error}", path.display()));
    }
    let mut secret = [0_u8; 32];
    OsRng.fill_bytes(&mut secret);
    let password = hex::encode(secret);
    atomic_private_write(path, password.as_bytes()).map_err(|error| {
        format!(
            "이 인스턴스의 계정 암호 파일 생성 실패({}): {error}",
            path.display()
        )
    })?;
    eprintln!(
        "이 인스턴스의 계정 암호 파일을 생성했습니다: {}",
        path.display()
    );
    Ok(password)
}

fn print_rpc_lookup(rpc_port: u16, method: &str, hash: &str) -> Result<(), String> {
    if !hash.starts_with("0x")
        || hash.len() != 66
        || !hash[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("거래 해시는 0x로 시작하는 64자리 hex여야 합니다.".into());
    }
    let response = rpc_call(
        rpc_port,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": [hash]}),
    )?;
    if let Some(error) = response.get("error") {
        return Err(format!("RPC 조회 실패: {error}"));
    }
    let result = response
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn validate_account_address(address: &str) -> Result<(), String> {
    if address.starts_with("0x")
        && address.len() == 42
        && address[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        Ok(())
    } else {
        Err("주소는 0x로 시작하는 40자리 IEUM 계정 주소여야 합니다.".into())
    }
}

fn format_ieum_amount(value: u128) -> String {
    let whole = value / 1_000_000_000_000_000_000;
    let fraction = value % 1_000_000_000_000_000_000;
    if fraction == 0 {
        return whole.to_string();
    }
    format!("{whole}.{:018}", fraction)
        .trim_end_matches('0')
        .to_string()
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

trait CliTransferWallet {
    fn cli_address(&self) -> String;
    fn cli_sign_transfer(&self, to: String, amount: u128, fee: u128, nonce: u64) -> Transaction;
}

impl CliTransferWallet for AccountWallet {
    fn cli_address(&self) -> String {
        self.address()
    }
    fn cli_sign_transfer(&self, to: String, amount: u128, fee: u128, nonce: u64) -> Transaction {
        self.sign_transfer(to, amount, fee, nonce)
    }
}

impl CliTransferWallet for Wallet {
    fn cli_address(&self) -> String {
        self.address()
    }
    fn cli_sign_transfer(&self, to: String, amount: u128, fee: u128, nonce: u64) -> Transaction {
        self.sign_transfer(to, amount, fee, nonce)
    }
}

fn send_wallet_balance<W: CliTransferWallet>(
    wallet: &W,
    to: String,
    amount: String,
    fee: String,
    rpc_port: u16,
) -> Result<(), String> {
    if !to.starts_with("0x")
        || to.len() != 42
        || !to[2..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("받는 주소는 0x로 시작하는 40자리 IEUM 계정 주소여야 합니다.".into());
    }
    let fee = parse_ieum_amount(&fee)?;
    let balance_response = rpc_call(
        rpc_port,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getBalance",
            "params": [wallet.cli_address(), "latest"]
        }),
    )?;
    let balance = parse_rpc_quantity(&balance_response, "잔액")?;
    let amount = if amount.eq_ignore_ascii_case("all") {
        balance
            .checked_sub(fee)
            .ok_or("잔액이 수수료보다 작아 전체 송금을 할 수 없습니다.")?
    } else {
        parse_ieum_amount(&amount)?
    };
    if amount.checked_add(fee).is_none_or(|total| total > balance) {
        return Err(format!(
            "잔액이 부족합니다. 보유: {balance} wei, 송금+수수료: {} wei",
            amount.saturating_add(fee)
        ));
    }
    let nonce_response = rpc_call(
        rpc_port,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "eth_getTransactionCount",
            "params": [wallet.cli_address(), "pending"]
        }),
    )?;
    let nonce = u64::try_from(parse_rpc_quantity(&nonce_response, "nonce")?)
        .map_err(|_| "RPC nonce가 u64 범위를 벗어났습니다.".to_string())?;
    let from = wallet.cli_address();
    let transaction = wallet.cli_sign_transfer(to.clone(), amount, fee, nonce);
    let response = rpc_call(
        rpc_port,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "ieum_sendSignedTransaction",
            "params": [transaction]
        }),
    )?;
    if let Some(error) = response.get("error") {
        return Err(format!("송금 제출 실패: {error}"));
    }
    println!("[보내는 계정] {from}");
    println!("[송금 대상] {to}");
    println!(
        "송금 제출 완료: {}",
        response
            .get("result")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("거래 해시 없음")
    );
    Ok(())
}

fn parse_rpc_quantity(response: &serde_json::Value, name: &str) -> Result<u128, String> {
    let value = response
        .get("result")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{name} 조회 실패: {response}"))?;
    u128::from_str_radix(value.trim_start_matches("0x"), 16)
        .map_err(|_| format!("RPC {name} 형식이 올바르지 않습니다."))
}

fn parse_ieum_amount(value: &str) -> Result<u128, String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || fraction.len() > 18
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("IEUM 금액은 소수점 이하 최대 18자리 숫자여야 합니다.".into());
    }
    let whole: u128 = whole
        .parse()
        .map_err(|_| "IEUM 금액이 너무 큽니다.".to_string())?;
    let fraction = format!("{fraction:0<18}")
        .parse::<u128>()
        .map_err(|_| "IEUM 소수 금액이 올바르지 않습니다.".to_string())?;
    whole
        .checked_mul(10u128.pow(18))
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| "IEUM 금액이 너무 큽니다.".into())
}

fn rpc_call(port: u16, request: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .map_err(|error| format!("로컬 RPC 연결 실패(127.0.0.1:{port}): {error}"))?;
    write!(
        stream,
        "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .map_err(|error| error.to_string())?;
    stream.write_all(&body).map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut response).map_err(|error| error.to_string())?;
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("RPC HTTP 응답 형식이 올바르지 않습니다.")?;
    serde_json::from_slice(&response[split + 4..])
        .map_err(|error| format!("RPC JSON 응답 오류: {error}"))
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn load_validator_wallet(
    path: &Path,
    index: u8,
    allow_insecure_test_keys: bool,
) -> Result<Wallet, String> {
    if allow_insecure_test_keys {
        log_info!(
            "[경고] 고정 개발 검증자 키 {}번을 사용합니다. 실제 자산을 넣지 마세요.",
            index
        );
        return Ok(Wallet::from_seed(testnet_validator_seed(index)));
    }
    let value = fs::read_to_string(path).map_err(|error| {
        format!(
            "검증자 키 파일을 읽지 못했습니다({}): {error}. 개발망이면 --allow-insecure-test-keys를 명시하세요.",
            path.display()
        )
    })?;
    let seed: [u8; 32] = hex::decode(value.trim().trim_start_matches("0x"))
        .map_err(|_| "검증자 키 파일은 32바이트 hex여야 합니다.")?
        .try_into()
        .map_err(|_| "검증자 키 파일은 정확히 32바이트여야 합니다.")?;
    Ok(Wallet::from_seed(seed))
}

#[derive(Debug, Deserialize, Serialize)]
struct ValidatorConfig {
    chain_id: String,
    validators: Vec<Validator>,
}

fn verify_validator_registration(registration: &ValidatorRegistration) -> Result<(), String> {
    registration
        .peer_id
        .parse::<libp2p::PeerId>()
        .map_err(|_| "등록 PeerId 형식이 올바르지 않습니다.".to_string())?;
    ieum_chain::wallet::verify_signature(
        &registration.validator_id,
        &ValidatorRegistration::bytes_to_sign(&registration.validator_id, &registration.peer_id),
        &registration.signature_hex,
    )
    .map_err(|error| format!("검증자 소유권 서명 오류: {error}"))
}

fn save_validators(path: &Path, validators: &[Validator]) -> Result<(), String> {
    let config = ValidatorConfig {
        chain_id: "21004".into(),
        validators: validators.to_vec(),
    };
    let mut contents = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("검증자 설정 직렬화 실패: {error}"))?;
    contents.push('\n');
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("validators.json");
    let temporary = path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        unix_timestamp_nanos()
    ));
    let mut temporary_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("검증자 임시 설정 생성 실패: {error}"))?;
    temporary_file
        .write_all(contents.as_bytes())
        .and_then(|_| temporary_file.sync_all())
        .map_err(|error| format!("검증자 임시 설정 저장 실패: {error}"))?;
    fs::rename(&temporary, path)
        .map_err(|error| format!("검증자 설정 교체 실패({}): {error}", path.display()))
}

fn unix_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn multiaddr_peer_id(address: &Multiaddr) -> Option<libp2p::PeerId> {
    address.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer_id) => Some(peer_id),
        _ => None,
    })
}

fn load_validators(path: &Path) -> Result<Vec<Validator>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("검증자 설정 읽기 실패({}): {error}", path.display()))?;
    let config: ValidatorConfig = serde_json::from_str(&text)
        .map_err(|error| format!("검증자 설정 형식 오류({}): {error}", path.display()))?;
    if config.chain_id != "21004" {
        return Err("검증자 설정 chain_id는 21004여야 합니다.".into());
    }
    if config.validators.is_empty() {
        return Err("검증자는 최소 1개가 필요합니다.".into());
    }
    Ok(config.validators)
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

struct InstanceGuard {
    _listener: TcpListener,
}

fn acquire_instance_guard(is_client: bool) -> Result<InstanceGuard, String> {
    let (port, mode) = if is_client {
        (CLIENT_INSTANCE_PORT, "클라이언트")
    } else {
        (SERVER_INSTANCE_PORT, "서버")
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|_| {
        format!(
            "이미 IEUM {mode} 인스턴스가 실행 중입니다. 기존 프로세스를 종료한 뒤 다시 실행하세요."
        )
    })?;
    Ok(InstanceGuard {
        _listener: listener,
    })
}

fn prepare_ports(args: &mut NodeArgs, is_client: bool) -> Result<(), String> {
    if is_client {
        let original_p2p = args.port;
        args.port = first_available_udp_port(args.port, 100)?;
        if args.port != original_p2p {
            log_info!(
                "UDP {original_p2p} 포트가 사용 중이므로 클라이언트 P2P 포트를 {}로 자동 변경합니다.",
                args.port
            );
        }

        let original_rpc = args.rpc_port;
        args.rpc_port = first_available_tcp_port(args.rpc_host, args.rpc_port, 100)?;
        if args.rpc_port != original_rpc {
            log_info!(
                "TCP {original_rpc} 포트가 사용 중이므로 클라이언트 RPC 포트를 {}로 자동 변경합니다.",
                args.rpc_port
            );
        }
    } else {
        ensure_udp_port_available(args.port)?;
        ensure_tcp_port_available(args.rpc_host, args.rpc_port)?;
    }
    Ok(())
}

fn ensure_udp_port_available(port: u16) -> Result<(), String> {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))
        .map(|_| ())
        .map_err(|error| {
            format!(
                "P2P UDP {port} 포트를 사용할 수 없습니다: {error}. 이미 IEUM 노드나 다른 프로그램이 실행 중인지 확인하세요."
            )
        })
}

fn ensure_tcp_port_available(host: IpAddr, port: u16) -> Result<(), String> {
    TcpListener::bind((host, port)).map(|_| ()).map_err(|error| {
        format!(
            "JSON-RPC TCP {host}:{port} 포트를 사용할 수 없습니다: {error}. 이미 IEUM 노드나 다른 프로그램이 실행 중인지 확인하세요."
        )
    })
}

fn first_available_udp_port(start: u16, attempts: u16) -> Result<u16, String> {
    (start..=start.saturating_add(attempts))
        .find(|port| UdpSocket::bind((Ipv4Addr::UNSPECIFIED, *port)).is_ok())
        .ok_or_else(|| {
            format!(
                "사용 가능한 클라이언트 P2P UDP 포트를 찾지 못했습니다({start}~{}).",
                start.saturating_add(attempts)
            )
        })
}

fn first_available_tcp_port(host: IpAddr, start: u16, attempts: u16) -> Result<u16, String> {
    (start..=start.saturating_add(attempts))
        .find(|port| TcpListener::bind((host, *port)).is_ok())
        .ok_or_else(|| {
            format!(
                "사용 가능한 클라이언트 RPC TCP 포트를 찾지 못했습니다({start}~{}).",
                start.saturating_add(attempts)
            )
        })
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum BootstrapConfig {
    Addresses(Vec<String>),
    Object { peers: Vec<String> },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct NodeNetworkSettings {
    #[serde(default)]
    bootstrap_peers: Vec<String>,
    #[serde(default)]
    advertise_address: Option<String>,
}

fn default_bootstrap_peers() -> Result<Vec<Multiaddr>, String> {
    DEFAULT_BOOTSTRAP_PEERS
        .iter()
        .map(|address| {
            address
                .parse()
                .map_err(|error| format!("내장 부트스트랩 주소 오류({address}): {error}"))
        })
        .collect()
}

fn load_network_settings() -> Result<NodeNetworkSettings, String> {
    let path = Path::new(DEFAULT_NETWORK_CONFIG);
    if !path.exists() {
        return Ok(NodeNetworkSettings::default());
    }
    let text = fs::read_to_string(path)
        .map_err(|error| format!("네트워크 설정 읽기 실패({}): {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("네트워크 설정 형식 오류({}): {error}", path.display()))
}

fn configured_bootstrap_peers() -> Result<Vec<Multiaddr>, String> {
    let settings = load_network_settings()?;
    if settings.bootstrap_peers.is_empty() {
        return default_bootstrap_peers();
    }
    settings
        .bootstrap_peers
        .into_iter()
        .map(|address| {
            address
                .parse()
                .map_err(|error| format!("부트스트랩 주소 형식 오류({address}): {error}"))
        })
        .collect()
}

fn ensure_peer_id(
    mut address: Multiaddr,
    local_peer_id: libp2p::PeerId,
) -> Result<Multiaddr, String> {
    match multiaddr_peer_id(&address) {
        Some(peer_id) if peer_id != local_peer_id => Err(format!(
            "공개 광고 주소의 PeerId가 현재 server.node.key와 다릅니다: {peer_id} != {local_peer_id}"
        )),
        // Swarm은 자신의 PeerId를 Identify 정보에 붙이므로 외부 주소에는 전송 주소만
        // 등록합니다. 사용자는 검증을 위해 완전한 /p2p/ 주소를 입력할 수 있습니다.
        Some(_) => {
            let _ = address.pop();
            Ok(address)
        }
        None => Ok(address),
    }
}

fn repair_local_advertise_address(
    local_peer_id: libp2p::PeerId,
) -> Result<NodeNetworkSettings, String> {
    let mut settings = load_network_settings()?;
    let Some(value) = settings.advertise_address.as_deref() else {
        return Ok(settings);
    };
    let mut address: Multiaddr = value
        .parse()
        .map_err(|error| format!("공개 광고 주소 형식 오류({value}): {error}"))?;
    if multiaddr_peer_id(&address).is_some_and(|peer_id| peer_id != local_peer_id) {
        let _ = address.pop();
        address.push(Protocol::P2p(local_peer_id));
        let repaired = address.to_string();
        log_info!(
            "[네트워크 자동 복구] server.node.key 변경을 감지해 공개 광고 주소 PeerId를 \
             현재 값으로 교체했습니다: {repaired}"
        );
        settings.advertise_address = Some(repaired);
        save_network_settings(&settings)?;
    }
    Ok(settings)
}

fn save_network_settings(settings: &NodeNetworkSettings) -> Result<(), String> {
    let path = Path::new(DEFAULT_NETWORK_CONFIG);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("설정 폴더 생성 실패({}): {error}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, format!("{json}\n"))
        .map_err(|error| format!("네트워크 설정 저장 실패({}): {error}", path.display()))
}

fn run_network_command(command: NetworkCommandConfig) -> Result<(), String> {
    match command {
        NetworkCommandConfig::Show => {
            let settings = load_network_settings()?;
            println!("bootstrap:");
            let peers = if settings.bootstrap_peers.is_empty() {
                DEFAULT_BOOTSTRAP_PEERS
                    .iter()
                    .map(|address| (*address).to_string())
                    .collect()
            } else {
                settings.bootstrap_peers
            };
            for peer in peers {
                println!("  - {peer}");
            }
            println!(
                "advertise_address: {}",
                settings
                    .advertise_address
                    .as_deref()
                    .unwrap_or("(자동/NAT)")
            );
            Ok(())
        }
        NetworkCommandConfig::Set {
            bootstrap_peers,
            advertise_address,
        } => {
            if bootstrap_peers.is_empty() && advertise_address.is_none() {
                return Err("--bootstrap 또는 --advertise-address 중 하나는 필요합니다.".into());
            }
            let mut settings = load_network_settings()?;
            if !bootstrap_peers.is_empty() {
                settings.bootstrap_peers = bootstrap_peers
                    .into_iter()
                    .map(|address| address.to_string())
                    .collect();
            }
            if let Some(address) = advertise_address {
                settings.advertise_address = Some(address.to_string());
            }
            save_network_settings(&settings)?;
            println!("네트워크 설정을 저장했습니다: {DEFAULT_NETWORK_CONFIG}");
            Ok(())
        }
        NetworkCommandConfig::Reset => {
            let path = Path::new(DEFAULT_NETWORK_CONFIG);
            if path.exists() {
                fs::remove_file(path).map_err(|error| {
                    format!("네트워크 설정 삭제 실패({}): {error}", path.display())
                })?;
            }
            println!("내장 네트워크 기본값으로 되돌렸습니다.");
            Ok(())
        }
    }
}

fn load_bootstrap_peers(
    path: &Path,
    mut command_line_peers: Vec<Multiaddr>,
) -> Result<Vec<Multiaddr>, String> {
    // 기본 실행은 과거 배포본에 남은 bootstrap.json의 오래된 PeerId에 영향을 받지
    // 않습니다. config/network.json으로 명시 설정했거나 내장 4개 노드를 사용합니다.
    let mut peers = if path == Path::new(DEFAULT_BOOTSTRAP_CONFIG) {
        configured_bootstrap_peers()?
    } else if path.exists() {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("부트스트랩 설정 읽기 실패({}): {error}", path.display()))?;
        let configured: BootstrapConfig = serde_json::from_str(&text)
            .map_err(|error| format!("부트스트랩 설정 형식 오류({}): {error}", path.display()))?;
        let addresses = match configured {
            BootstrapConfig::Addresses(peers) | BootstrapConfig::Object { peers } => peers,
        };
        addresses
            .into_iter()
            .map(|address| {
                address
                    .parse()
                    .map_err(|error| format!("부트스트랩 주소 형식 오류({address}): {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?
    } else if command_line_peers.is_empty() {
        return Err(format!(
            "부트스트랩 설정 파일이 없습니다: {}",
            path.display()
        ));
    } else {
        Vec::new()
    };
    peers.append(&mut command_line_peers);
    peers.sort();
    peers.dedup();
    if peers.is_empty() {
        return Err(format!(
            "부트스트랩 피어가 없습니다. {}에 운영 서버 주소를 등록하세요.",
            path.display()
        ));
    }
    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_action_test_flag_requires_server_subcommand() {
        let args = Args::try_parse_from([
            "ieum-chain",
            "server",
            "--git_action_test",
            "--validator-index",
            "3",
        ])
        .unwrap();
        let Some(Command::Server(node)) = args.command else {
            panic!("server 명령이어야 합니다.");
        };
        assert!(node.git_action_test);
        assert_eq!(node.validator_index, 3);

        assert!(Args::try_parse_from(["ieum-chain", "--git_action_test"]).is_err());
    }

    #[test]
    fn bootstrap_config_accepts_address_array() {
        let json = format!(r#"["{}"]"#, DEFAULT_BOOTSTRAP_PEERS[0]);
        let config: BootstrapConfig = serde_json::from_str(&json).unwrap();
        match config {
            BootstrapConfig::Addresses(peers) => {
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0], DEFAULT_BOOTSTRAP_PEERS[0]);
            }
            BootstrapConfig::Object { .. } => panic!("배열 형식이어야 합니다."),
        }
    }

    #[test]
    fn validator_registration_requires_key_ownership_signature() {
        let signer: ValidatorSigner = Wallet::from_seed([91; 32]).into();
        let validator_id = signer.address();
        let peer_id = libp2p::PeerId::random().to_string();
        let registration = ValidatorRegistration {
            validator_id: validator_id.clone(),
            peer_id: peer_id.clone(),
            signature_hex: signer
                .sign_bytes(&ValidatorRegistration::bytes_to_sign(
                    &validator_id,
                    &peer_id,
                ))
                .unwrap(),
        };
        assert!(verify_validator_registration(&registration).is_ok());

        let mut forged = registration;
        forged.peer_id = libp2p::PeerId::random().to_string();
        assert!(verify_validator_registration(&forged).is_err());
    }

    #[test]
    fn bootstrap_node_does_not_dial_itself() {
        let address: Multiaddr = DEFAULT_BOOTSTRAP_PEERS[0].parse().unwrap();
        let peer_id = multiaddr_peer_id(&address).unwrap();
        assert_eq!(
            peer_id.to_string(),
            DEFAULT_BOOTSTRAP_PEERS[0].rsplit('/').next().unwrap()
        );
    }

    #[test]
    fn default_bootstrap_contains_four_public_nodes() {
        let peers = default_bootstrap_peers().unwrap();
        assert_eq!(peers.len(), 4);
        assert!(
            peers
                .iter()
                .any(|peer| peer.to_string().contains("/udp/7002/"))
        );
        assert!(
            peers
                .iter()
                .any(|peer| peer.to_string().contains("/udp/7003/"))
        );
        assert!(
            peers
                .iter()
                .any(|peer| peer.to_string().contains("/udp/7004/"))
        );
    }
}
