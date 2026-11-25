//! 4노드를 하나씩 재시작해도 라운드가 다시 맞춰져 블록이 계속 확정되는지 확인하는
//! 메모리 네트워크 시뮬레이션입니다. main.rs의 메시지 처리 순서를 그대로 따릅니다.
use ieum_chain::consensus::{ConsensusMessage, SignedProposal, Validator};
use ieum_chain::consensus_era::SignedRoundChange;
use ieum_chain::consensus_runtime::is_deferable_vote_error;
use ieum_chain::model::Block;
use ieum_chain::wallet::Wallet;
use ieum_chain::{Blockchain, ConsensusRuntime, ConsensusTimeouts};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
enum Msg {
    Proposal(SignedProposal),
    Vote(ConsensusMessage),
    RoundChange(SignedRoundChange),
}

struct Net {
    queue: Vec<(Instant, usize, Msg)>,
    down_until: [Option<Instant>; 4],
    seed: u64,
}

impl Net {
    fn jitter(&mut self) -> Duration {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        Duration::from_millis(1 + (self.seed >> 33) % 25)
    }

    fn broadcast(&mut self, from: usize, msg: Msg) {
        let now = Instant::now();
        for to in 0..4 {
            if to != from {
                let at = now + self.jitter();
                self.queue.push((at, to, msg.clone()));
            }
        }
    }
}

// 실제 노드처럼 노드마다 제한 시간이 조금씩 달라 혼자 돌면 라운드가 벌어집니다.
fn timeouts(index: usize) -> ConsensusTimeouts {
    ConsensusTimeouts {
        propose: Duration::from_millis(100 + 17 * index as u64),
        prevote: Duration::from_millis(80),
        precommit: Duration::from_millis(80),
    }
}

fn validators() -> Vec<Validator> {
    (1_u8..=4)
        .map(|n| Validator::new(Wallet::from_seed([n; 32]).address(), 100))
        .collect()
}

fn node(index: usize, chain: Blockchain) -> ConsensusRuntime {
    ConsensusRuntime::with_timeouts(
        chain,
        validators(),
        Wallet::from_seed([index as u8 + 1; 32]),
        timeouts(index),
    )
    .unwrap()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn after_vote(nodes: &mut [ConsensusRuntime], net: &mut Net, i: usize) {
    for vote in nodes[i].replay_deferred_votes().unwrap() {
        net.broadcast(i, Msg::Vote(vote));
    }
    if !nodes[i].take_finalized().is_empty() {
        nodes[i].advance_after_finalization().unwrap();
    }
}

fn on_proposal(nodes: &mut [ConsensusRuntime], net: &mut Net, i: usize, proposal: SignedProposal) {
    // 다른 라운드·잠긴 값 등으로 거부된 제안은 main.rs처럼 버립니다.
    if let Ok(prevote) = nodes[i].receive_proposal(proposal) {
        net.broadcast(i, Msg::Vote(prevote.clone()));
        if let Some(precommit) = nodes[i].receive_vote(prevote).unwrap() {
            net.broadcast(i, Msg::Vote(precommit.clone()));
            nodes[i].receive_vote(precommit).unwrap();
        }
        after_vote(nodes, net, i);
    }
}

fn on_round_moved(nodes: &mut [ConsensusRuntime], net: &mut Net, i: usize) {
    after_vote(nodes, net, i);
}

fn deliver(nodes: &mut [ConsensusRuntime], net: &mut Net, i: usize, msg: Msg) {
    match msg {
        Msg::Proposal(proposal) => on_proposal(nodes, net, i, proposal),
        Msg::Vote(vote) => {
            match nodes[i].receive_vote(vote.clone()) {
                Ok(Some(precommit)) => {
                    net.broadcast(i, Msg::Vote(precommit.clone()));
                    nodes[i].receive_vote(precommit).unwrap();
                }
                Ok(None) => {}
                Err(error) if is_deferable_vote_error(&error) => nodes[i].defer_vote(vote),
                Err(_) => {}
            }
            after_vote(nodes, net, i);
        }
        Msg::RoundChange(change) => {
            if let Ok(true) = nodes[i].receive_round_change(change) {
                on_round_moved(nodes, net, i);
            }
        }
    }
}

fn tick(nodes: &mut [ConsensusRuntime], net: &mut Net, i: usize, now: Instant, rebroadcast: bool) {
    // 뒤처진 노드는 가장 앞선 노드의 확정 인증서로 동기화합니다(main의 sync 요청 대체).
    let best = (0..4).max_by_key(|&j| nodes[j].chain.tip_height()).unwrap();
    if nodes[best].chain.tip_height() > nodes[i].chain.tip_height() {
        let certificates = nodes[best].certificates_from(nodes[i].chain.tip_height() + 1);
        let _ = nodes[i].apply_sync_certificates(certificates);
    }
    let (timed_out, change) = nodes[i].timeout_if_due_with_round_change(now).unwrap();
    if timed_out {
        if let Some(change) = change {
            net.broadcast(i, Msg::RoundChange(change));
        }
        on_round_moved(nodes, net, i);
    }
    if rebroadcast {
        for change in nodes[i].round_change_snapshot() {
            net.broadcast(i, Msg::RoundChange(change));
        }
    }
    if nodes[i].can_make_proposal() {
        let previous = nodes[i].chain.blocks.last().unwrap().clone();
        let block = Block::new(
            previous.height + 1,
            previous.hash.clone(),
            unix_now().max(previous.timestamp),
            "validator".into(),
            vec![],
        );
        if let Ok(proposal) = nodes[i].make_proposal(block) {
            net.broadcast(i, Msg::Proposal(proposal.clone()));
            on_proposal(nodes, net, i, proposal);
        }
    }
}

/// 시뮬레이션을 `duration` 동안 돌리고, `restarts`의 (시작 시점, 노드, 중단 시간)마다
/// 그 노드를 내렸다가 메모리 상태 없이 체인만 가진 채 다시 띄웁니다.
fn simulate(
    duration: Duration,
    checkpoint: Duration,
    restarts: &[(Duration, usize, Duration)],
    seed: u64,
) -> (Vec<ConsensusRuntime>, u64) {
    let mut nodes: Vec<_> = (0..4).map(|i| node(i, Blockchain::new(vec![]))).collect();
    let mut net = Net {
        queue: Vec::new(),
        down_until: [None; 4],
        seed,
    };
    let start = Instant::now();
    let mut pending_restarts: Vec<_> = restarts.to_vec();
    let mut last_rebroadcast = start;
    let mut checkpoint_height = None;
    while start.elapsed() < duration {
        let now = Instant::now();
        if checkpoint_height.is_none() && start + checkpoint <= now {
            checkpoint_height = Some(max_height(&nodes));
        }
        pending_restarts.retain(|&(at, i, down)| {
            if start + at <= now {
                net.down_until[i] = Some(now + down);
                false
            } else {
                true
            }
        });
        for (i, slot) in nodes.iter_mut().enumerate() {
            if let Some(until) = net.down_until[i]
                && until <= now
            {
                net.down_until[i] = None;
                let chain = slot.chain.clone();
                *slot = node(i, chain);
            }
        }
        let (ready, later): (Vec<_>, Vec<_>) =
            net.queue.drain(..).partition(|(at, _, _)| *at <= now);
        net.queue = later;
        for (_, to, msg) in ready {
            if net.down_until[to].is_none() {
                deliver(&mut nodes, &mut net, to, msg);
            }
        }
        let rebroadcast = now.duration_since(last_rebroadcast) >= Duration::from_millis(200);
        if rebroadcast {
            last_rebroadcast = now;
        }
        for i in 0..4 {
            if net.down_until[i].is_none() {
                tick(&mut nodes, &mut net, i, now, rebroadcast);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    (nodes, checkpoint_height.unwrap())
}

fn max_height(nodes: &[ConsensusRuntime]) -> u64 {
    nodes.iter().map(|n| n.chain.tip_height()).max().unwrap()
}

fn assert_recovers(name: &str, restarts: &[(Duration, usize, Duration)], recovered_at: Duration) {
    for seed in 1..=3_u64 {
        let (nodes, height_at_recovery) = simulate(
            recovered_at + Duration::from_millis(2_000),
            recovered_at,
            restarts,
            seed,
        );
        let final_height = max_height(&nodes);
        assert!(
            final_height >= height_at_recovery + 3,
            "{name} seed {seed}: 복구 뒤 2초 동안 확정이 거의 없음 ({height_at_recovery} → {final_height}), 라운드 {:?}",
            nodes.iter().map(|n| n.round()).collect::<Vec<_>>()
        );
        let common = nodes.iter().map(|n| n.chain.blocks.len()).min().unwrap();
        for n in &nodes {
            assert_eq!(
                n.chain.blocks[..common],
                nodes[0].chain.blocks[..common],
                "{name}: 체인 분기"
            );
        }
    }
}

/// 시간 기반 시뮬레이션이라 디버그 빌드에서는 서명 검증이 느려 의미가 없습니다.
/// `cargo test --release --test bft_restart_recovery -- --ignored`로 실행합니다.
#[test]
#[ignore]
fn sequential_restarts_keep_finalizing() {
    // update-four-nodes.sh처럼 하나씩 재시작합니다.
    let restarts: Vec<_> = (0..4)
        .map(|i| {
            (
                Duration::from_millis(400 + i as u64 * 450),
                i,
                Duration::from_millis(300),
            )
        })
        .collect();
    assert_recovers("순차 재시작", &restarts, Duration::from_millis(2_400));
}

#[test]
#[ignore]
fn two_nodes_down_then_back_realign_rounds() {
    // 두 노드가 동시에 내려가 있는 동안 남은 두 노드가 각자 라운드를 돌던 높이 108 정지 상황입니다.
    let restarts = [
        (Duration::from_millis(400), 2, Duration::from_millis(1_500)),
        (Duration::from_millis(450), 3, Duration::from_millis(1_500)),
    ];
    assert_recovers("2노드 동시 중단", &restarts, Duration::from_millis(2_200));
}
