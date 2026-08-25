pub mod account;
pub mod account_security;
pub mod archive;
pub mod chain;
pub mod checkpoint;
pub mod communication;
pub mod consensus;
pub mod consensus_era;
pub mod consensus_runtime;
pub mod consensus_wal;
pub mod embedded_db;
pub mod evidence_store;
pub mod finality_store;
pub mod genesis;
pub mod holder_rewards;
pub mod keystore;
pub mod logger;
pub mod mempool;
pub mod metrics;
pub mod model;
pub mod modules;
pub mod network;
pub mod node_emission;
pub mod node_key;
pub mod node_wallet_keystore;
pub mod operations;
pub mod peer_guard;
pub mod raw_transaction;
pub mod recovery;
pub mod rpc;
pub mod scheduled_event;
pub mod signer;
pub mod snapshot_scheduler;
pub mod snapshot_sync;
pub mod staking;
pub mod state_store;
pub mod storage;
pub mod traffic_rewards;
pub mod updater;
pub mod upgrade;

/// Cargo 패키지 버전의 하이픈을 점으로 변환한 단일 사용자 표시 버전입니다.
/// `build.rs`가 빌드 시 생성하므로 CLI·RPC·자동 업데이트 버전이 서로 어긋나지 않습니다.
pub const IEUM_DISPLAY_VERSION: &str = env!("IEUM_DISPLAY_VERSION");
pub mod validator_interest;
pub mod validator_key;
pub mod validator_policy;
pub mod wallet;

pub use account::AccountWallet;
pub use account_security::{AccountKey, AccountPolicy, Authorization, Permission, RecoveryRequest};
pub use archive::{ArchiveStore, StateSnapshot};
pub use chain::{Blockchain, FEE_BPS_DENOMINATOR, FOUNDATION_FEE_ADDRESS, FOUNDATION_FEE_BPS};
pub use checkpoint::Checkpoint;
pub use communication::{
    CommunicationAck, CommunicationEnvelope, CommunicationInbox, CommunicationKind,
    MAX_ENCRYPTED_SIGNAL_BYTES, MAX_PENDING_SIGNALS, MAX_SIGNAL_TTL_SECONDS,
};
pub use consensus::{
    BftConsensus, ConsensusMessage, ConsensusPhase, DoubleVoteEvidence, FinalityCertificate,
    SignedProposal, Validator,
};
pub use consensus_era::{
    EraConfig, EraManager, NilVote, RoundChangeCertificate, SignedRoundChange, ValidatorSetUpdate,
};
pub use consensus_runtime::{ConsensusRuntime, ConsensusTimeouts};
pub use consensus_wal::ConsensusWal;
pub use embedded_db::EmbeddedDb;
pub use evidence_store::EvidenceStore;
pub use finality_store::FinalityStore;
pub use genesis::GenesisConfig;
pub use holder_rewards::HolderRewardPolicy;
pub use keystore::Keystore;
pub use mempool::Mempool;
pub use metrics::{NodeMetrics, prometheus_router};
pub use model::{Block, Transaction, TransactionAction};
pub use modules::{AppModule, ModuleContext, ModuleRouter, StateMigration};
pub use network::{
    NetworkCommand, NetworkConfig, NetworkEvent, NodeRewardRegistration, P2pNode,
    ValidatorRegistration,
};
pub use node_emission::{
    MAIN_NODE_PEER_IDS, MAX_SUPPLY, MAX_SUPPLY_IEUM, NodeServiceAttestation,
    REWARD_ACTIVATION_HEIGHT, REWARD_ACTIVATION_UNIX, TOTAL_NODE_EMISSION, annual_budget,
    daily_budget, is_reward_active, settle_daily_rewards,
};
pub use node_wallet_keystore::NodeWalletKeystore;
pub use operations::{NodeStorageMode, PruningPolicy, StorageManifest};
pub use peer_guard::{PeerDecision, PeerGuard};
pub use recovery::{
    RecoveryApprovalBasis, RecoveryApprovalResult, evaluate_checkpoint_recovery_approvals,
    evaluate_recovery_approvals,
};
pub use rpc::{RpcConfig, RpcNodeHandle, RpcServer};
pub use scheduled_event::{
    EventPayment, EventSchedule, MAX_CLOCK_DRIFT_SECONDS, ScheduledEvent, ScheduledEventAction,
};
pub use signer::{ExternalSigner, ValidatorSigner};
pub use snapshot_scheduler::{ChunkAssignment, SnapshotScheduler};
pub use snapshot_sync::{
    SnapshotAttestation, SnapshotCertificate, SnapshotChunk, SnapshotDownload, SnapshotManifest,
    SyncTip, TipQuorum,
};
pub use staking::{DelegationPosition, StakingState, UnbondingEntry};
pub use state_store::{CanonicalState, StateStore};
pub use traffic_rewards::{
    ContributionLedger, EligibleNode, LotteryPayment, PeerCandidate, RelayReceipt, RewardPolicy,
    TrafficPolicy, draw_lottery, select_balanced_peers,
};
pub use upgrade::{ProtocolUpgrade, UpgradeSchedule};
pub use validator_interest::{
    ValidatorInterestPolicy, calculate_payments as calculate_validator_interest_payments,
};
pub use wallet::Wallet;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mempool_fee_save_and_load() {
        let alice = Wallet::from_seed([1; 32]);
        let bob = Wallet::from_seed([2; 32]);
        let producer = Wallet::from_seed([3; 32]);
        let mut chain = Blockchain::new(vec![(alice.address(), 1_000)]);
        let mut pool = Mempool::default();
        pool.add(alice.sign_transfer(bob.address(), 250, 3, 0))
            .unwrap();
        chain
            .add_block(pool.drain(100), producer.address())
            .unwrap();

        assert_eq!(chain.balance_of(&alice.address()), 747);
        assert_eq!(chain.balance_of(&bob.address()), 250);
        assert_eq!(chain.balance_of(&producer.address()), 3);
        assert!(chain.verify_and_rebuild().is_ok());
    }

    #[test]
    fn block_fee_is_split_between_producer_and_foundation() {
        let alice = Wallet::from_seed([1; 32]);
        let bob = Wallet::from_seed([2; 32]);
        let producer = Wallet::from_seed([3; 32]);
        let mut chain = Blockchain::new(vec![(alice.address(), 10_000)]);

        chain
            .add_block(
                vec![alice.sign_transfer(bob.address(), 1_000, 100, 0)],
                producer.address(),
            )
            .unwrap();

        assert_eq!(chain.balance_of(&alice.address()), 8_900);
        assert_eq!(chain.balance_of(&bob.address()), 1_000);
        assert_eq!(chain.balance_of(&producer.address()), 80);
        assert_eq!(chain.balance_of(FOUNDATION_FEE_ADDRESS), 20);
        assert!(chain.verify_and_rebuild().is_ok());
    }

    #[test]
    fn fee_rounding_remainder_is_paid_to_producer() {
        let alice = Wallet::from_seed([4; 32]);
        let bob = Wallet::from_seed([5; 32]);
        let producer = Wallet::from_seed([6; 32]);
        let mut chain = Blockchain::new(vec![(alice.address(), 100)]);

        chain
            .add_block(
                vec![alice.sign_transfer(bob.address(), 10, 3, 0)],
                producer.address(),
            )
            .unwrap();

        assert_eq!(chain.balance_of(&producer.address()), 3);
        assert_eq!(chain.balance_of(FOUNDATION_FEE_ADDRESS), 0);
    }

    #[test]
    fn scheduled_distribution_executes_exactly_once() {
        let receiver = "0x1111111111111111111111111111111111111111".to_string();
        let mut chain = Blockchain::new(vec![(FOUNDATION_FEE_ADDRESS.into(), 1_000)]);
        let event = ScheduledEvent {
            id: "distribution-2027".into(),
            execute_at: 100,
            action: ScheduledEventAction::TreasuryDistribution {
                recipients: vec![EventPayment {
                    address: receiver.clone(),
                    amount: 250,
                }],
            },
        };
        let first = Block::new(
            1,
            chain.tip_hash().to_string(),
            100,
            "producer".into(),
            vec![],
        )
        .with_system_events(vec![event.clone()]);
        chain.apply_block(first).unwrap();
        assert_eq!(chain.balance_of(FOUNDATION_FEE_ADDRESS), 750);
        assert_eq!(chain.balance_of(&receiver), 250);

        let duplicate = Block::new(
            2,
            chain.tip_hash().to_string(),
            101,
            "producer".into(),
            vec![],
        )
        .with_system_events(vec![event]);
        assert!(chain.apply_block(duplicate).is_err());
    }

    #[test]
    fn duplicate_transaction_is_rejected() {
        let alice = Wallet::from_seed([1; 32]);
        let bob = Wallet::from_seed([2; 32]);
        let tx = alice.sign_transfer(bob.address(), 10, 1, 0);
        let mut pool = Mempool::default();
        pool.add(tx.clone()).unwrap();
        assert!(pool.add(tx).is_err());
    }

    #[test]
    fn received_block_keeps_original_hash() {
        let alice = Wallet::from_seed([1; 32]);
        let bob = Wallet::from_seed([2; 32]);
        let validator = Wallet::from_seed([3; 32]);
        let initial = vec![(alice.address(), 1_000)];
        let mut sender = Blockchain::new(initial.clone());
        let tx = alice.sign_transfer(bob.address(), 100, 1, 0);
        let block = sender
            .add_block(vec![tx], validator.address())
            .unwrap()
            .clone();

        let mut receiver = Blockchain::new(initial);
        receiver.apply_block(block.clone()).unwrap();
        assert_eq!(receiver.blocks.last().unwrap().hash, block.hash);
        assert_eq!(receiver.balance_of(&bob.address()), 100);
    }

    #[test]
    fn bft_finalizes_after_two_thirds_precommit() {
        let keys: Vec<_> = (1..=4).map(|n| Wallet::from_seed([n; 32])).collect();
        let validators = keys
            .iter()
            .map(|key| Validator::new(key.address(), 100))
            .collect();
        let mut bft = BftConsensus::new(validators).unwrap();
        bft.start_round(5, 0).unwrap();
        let proposer = bft.expected_proposer().to_string();
        bft.propose(&proposer, "block-hash").unwrap();

        for key in keys.iter().take(3) {
            bft.handle(ConsensusMessage::prevote(5, 0, key, "block-hash"))
                .unwrap();
        }
        for key in keys.iter().take(3) {
            bft.handle(ConsensusMessage::precommit(5, 0, key, "block-hash"))
                .unwrap();
        }

        assert_eq!(bft.finalized_hash(), Some("block-hash"));
    }

    #[test]
    fn forged_consensus_vote_is_rejected() {
        let validator = Wallet::from_seed([1; 32]);
        let attacker = Wallet::from_seed([9; 32]);
        let mut message = ConsensusMessage::prevote(1, 0, &validator, "block-a");
        // b"..." 바이트 문자열은 ASCII만 허용한다.
        // 한글처럼 UTF-8 문자는 일반 문자열을 바이트 슬라이스로 변환해 전달한다.
        message.signature = attacker.sign_bytes("위조 서명".as_bytes());
        assert!(message.verify().is_err());
    }

    #[test]
    fn validator_double_vote_is_rejected() {
        let keys: Vec<_> = (1..=4).map(|n| Wallet::from_seed([n; 32])).collect();
        let validators = keys
            .iter()
            .map(|key| Validator::new(key.address(), 100))
            .collect();
        let mut bft = BftConsensus::new(validators).unwrap();
        bft.start_round(1, 0).unwrap();
        let proposer = bft.expected_proposer().to_string();
        bft.propose(&proposer, "block-a").unwrap();

        bft.handle(ConsensusMessage::prevote(1, 0, &keys[0], "block-a"))
            .unwrap();
        assert!(
            bft.handle(ConsensusMessage::prevote(1, 0, &keys[0], "block-b"))
                .is_err()
        );
    }
}
