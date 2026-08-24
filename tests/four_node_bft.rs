use ieum_chain::consensus::{ConsensusPhase, Validator};
use ieum_chain::model::Block;
use ieum_chain::wallet::Wallet;
use ieum_chain::{Blockchain, ConsensusRuntime};
use std::time::Duration;

fn setup() -> (Vec<ConsensusRuntime>, Vec<Wallet>) {
    let wallets: Vec<_> = (1_u8..=4).map(|n| Wallet::from_seed([n; 32])).collect();
    let validators = wallets
        .iter()
        .map(|w| Validator::new(w.address(), 100))
        .collect::<Vec<_>>();
    // Wallet은 개인키를 보유하므로 실수로 복제되지 않게 Clone을 구현하지 않습니다.
    // 테스트 노드용 지갑은 동일한 테스트 seed에서 별도로 재생성합니다.
    let nodes = (1_u8..=4)
        .map(|n| {
            ConsensusRuntime::new(
                Blockchain::new(vec![]),
                validators.clone(),
                Wallet::from_seed([n; 32]),
                Duration::from_millis(50),
            )
            .unwrap()
        })
        .collect();
    (nodes, wallets)
}

#[test]
fn four_nodes_finalize_then_store_and_new_node_syncs() {
    let (mut nodes, _) = setup();
    let proposer_index = (0..nodes.len())
        .find(|&i| {
            nodes[i]
                .make_proposal(Block::new(
                    1,
                    nodes[i].chain.blocks[0].hash.clone(),
                    1,
                    "validator".into(),
                    vec![],
                ))
                .is_ok()
        })
        .unwrap();
    let block = Block::new(
        1,
        nodes[proposer_index].chain.blocks[0].hash.clone(),
        1,
        "validator".into(),
        vec![],
    );
    let proposal = nodes[proposer_index].make_proposal(block).unwrap();
    let prevotes = nodes
        .iter_mut()
        .map(|node| node.receive_proposal(proposal.clone()).unwrap())
        .collect::<Vec<_>>();
    assert!(nodes.iter().all(|node| node.chain.blocks.len() == 1));

    let mut precommits = Vec::new();
    for vote in prevotes {
        for node in &mut nodes {
            if let Some(precommit) = node.receive_vote(vote.clone()).unwrap() {
                precommits.push(precommit);
            }
        }
    }
    precommits.sort_by(|a, b| a.validator_id.cmp(&b.validator_id));
    precommits.dedup_by(|a, b| a.validator_id == b.validator_id);
    for vote in precommits.into_iter().take(3) {
        for node in &mut nodes {
            node.receive_vote(vote.clone()).unwrap();
        }
    }
    assert!(
        nodes
            .iter()
            .all(|node| node.phase() == ConsensusPhase::Finalized)
    );
    assert!(nodes.iter().all(|node| node.chain.blocks.len() == 2));
    assert!(
        nodes
            .windows(2)
            .all(|pair| pair[0].chain.blocks == pair[1].chain.blocks)
    );

    let (mut fresh, _) = setup();
    let certificates = nodes[0].certificates_from(1);
    assert_eq!(certificates.len(), 1);
    assert_eq!(fresh[0].apply_sync_certificates(certificates).unwrap(), 1);
    assert_eq!(fresh[0].chain.blocks, nodes[0].chain.blocks);
}

#[test]
fn new_node_rejects_block_without_three_precommits() {
    let (mut nodes, wallets) = setup();
    let block = Block::new(
        1,
        nodes[0].chain.blocks[0].hash.clone(),
        1,
        "validator".into(),
        vec![],
    );
    let certificate = ieum_chain::FinalityCertificate {
        round: 0,
        precommits: wallets
            .iter()
            .take(2)
            .map(|wallet| ieum_chain::ConsensusMessage::precommit(1, 0, wallet, block.hash.clone()))
            .collect(),
        block,
    };
    assert!(nodes[0].apply_sync_certificates(vec![certificate]).is_err());
    assert_eq!(nodes[0].chain.tip_height(), 0);
}

#[test]
fn certified_snapshot_recovers_a_multi_block_gap() {
    use ieum_chain::{SnapshotAttestation, SnapshotCertificate, StateSnapshot, ValidatorSigner};
    use std::collections::{HashMap, HashSet};

    let (mut nodes, _) = setup();
    let checkpoint_chain = Blockchain::from_snapshot_with_staking(
        nodes[0].chain.chain_id,
        nodes[0].chain.genesis_commitment.clone(),
        20,
        "11".repeat(32),
        HashMap::new(),
        HashMap::new(),
        HashSet::new(),
        Default::default(),
    )
    .unwrap();
    let snapshot = StateSnapshot::from_chain(&checkpoint_chain);
    let attestations = (1_u8..=3)
        .map(|seed| {
            let signer = ValidatorSigner::from(Wallet::from_seed([seed; 32]));
            SnapshotAttestation::sign(&snapshot, &signer).unwrap()
        })
        .collect();
    let certificate = SnapshotCertificate::from_attestations(attestations).unwrap();

    nodes[0]
        .install_certified_snapshot(snapshot.clone(), &certificate)
        .unwrap();

    assert_eq!(nodes[0].chain.tip_height(), 20);
    assert_eq!(nodes[0].chain.tip_hash(), snapshot.block_hash);
    assert_eq!(nodes[0].chain.state_hash(), snapshot.state_hash);
}

#[test]
fn timeout_changes_round_without_storing_candidate() {
    let (mut nodes, _) = setup();
    assert_eq!(nodes[0].force_timeout_for_test().unwrap(), 1);
    assert_eq!(nodes[0].chain.blocks.len(), 1);
}

#[test]
fn proposer_does_not_create_a_second_proposal_after_prevote_started() {
    let (mut nodes, _) = setup();
    let proposer_index = (0..nodes.len())
        .find(|&index| nodes[index].can_make_proposal())
        .unwrap();
    let first = Block::new(
        1,
        nodes[proposer_index].chain.blocks[0].hash.clone(),
        1,
        "validator".into(),
        vec![],
    );
    let proposal = nodes[proposer_index].make_proposal(first).unwrap();
    nodes[proposer_index].receive_proposal(proposal).unwrap();

    assert!(!nodes[proposer_index].can_make_proposal());
    let duplicate = Block::new(
        1,
        nodes[proposer_index].chain.blocks[0].hash.clone(),
        2,
        "validator".into(),
        vec![],
    );
    assert!(
        nodes[proposer_index]
            .make_proposal(duplicate)
            .unwrap_err()
            .contains("현재 단계")
    );
}
