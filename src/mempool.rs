use crate::model::Transaction;
use std::collections::{HashMap, HashSet};

const DEFAULT_MAX_TRANSACTIONS: usize = 10_000;
const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct Mempool {
    transactions: Vec<Transaction>,
    ids: HashSet<String>,
    sender_nonce: HashMap<(String, u64), String>,
    total_bytes: usize,
    max_transactions: usize,
    max_bytes: usize,
}

impl Mempool {
    pub fn with_limits(max_transactions: usize, max_bytes: usize) -> Self {
        Self {
            transactions: Vec::new(),
            ids: HashSet::new(),
            sender_nonce: HashMap::new(),
            total_bytes: 0,
            max_transactions: max_transactions.max(1),
            max_bytes: max_bytes.max(1),
        }
    }

    /// 거래 ID가 같은 거래를 두 번 넣지 않습니다.
    /// 잔액과 nonce의 최종 검증은 블록 실행 시 다시 수행합니다.
    pub fn add(&mut self, tx: Transaction) -> Result<(), String> {
        let id = tx.id();
        if self.ids.contains(&id) {
            return Err("이미 mempool에 있는 거래입니다.".into());
        }
        let size = serde_json::to_vec(&tx)
            .map_err(|error| error.to_string())?
            .len();
        if size > self.max_bytes {
            return Err("거래 하나가 mempool 최대 바이트보다 큽니다.".into());
        }
        let key = (tx.from.clone(), tx.nonce);
        if let Some(previous_id) = self.sender_nonce.get(&key).cloned() {
            let position = self
                .transactions
                .iter()
                .position(|existing| existing.id() == previous_id)
                .ok_or("mempool nonce 인덱스가 손상되었습니다.")?;
            let previous = &self.transactions[position];
            let minimum_fee = previous.fee.saturating_add((previous.fee / 10).max(1));
            if tx.fee < minimum_fee {
                return Err(
                    "같은 nonce 거래 교체에는 기존보다 최소 10% 높은 수수료가 필요합니다.".into(),
                );
            }
            self.total_bytes = self.total_bytes.saturating_sub(
                serde_json::to_vec(previous)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0),
            );
            self.ids.remove(&previous_id);
            self.transactions.remove(position);
        }
        while self.transactions.len() >= self.max_transactions
            || self.total_bytes.saturating_add(size) > self.max_bytes
        {
            let Some((position, _)) = self
                .transactions
                .iter()
                .enumerate()
                .min_by_key(|(_, transaction)| transaction.fee)
            else {
                break;
            };
            self.remove_at(position);
        }
        self.ids.insert(id.clone());
        self.sender_nonce.insert(key, id);
        self.total_bytes = self.total_bytes.saturating_add(size);
        self.transactions.push(tx);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// P2P 전파용 읽기 전용 복사본입니다. 큐의 소유권이나 순서를 바꾸지 않습니다.
    pub fn snapshot(&self, max_count: usize) -> Vec<Transaction> {
        self.transactions.iter().take(max_count).cloned().collect()
    }

    /// 표준 JSON-RPC가 확정 전 거래도 조회할 수 있도록 mempool 거래를 찾습니다.
    pub fn transaction_by_hash(&self, hash: &str) -> Option<&Transaction> {
        let wanted = hash.strip_prefix("0x").unwrap_or(hash);
        self.transactions
            .iter()
            .find(|transaction| transaction.id().eq_ignore_ascii_case(wanted))
    }

    /// 확정 nonce 뒤에 연속으로 대기 중인 거래까지 포함한 다음 nonce입니다.
    pub fn next_nonce(&self, address: &str, finalized_nonce: u64) -> u64 {
        let mut next = finalized_nonce;
        while self.transactions.iter().any(|transaction| {
            transaction.from.eq_ignore_ascii_case(address) && transaction.nonce == next
        }) {
            let Some(incremented) = next.checked_add(1) else {
                break;
            };
            next = incremented;
        }
        next
    }

    /// 블록 최대 거래 수만큼 앞에서 꺼냅니다.
    /// 운영 버전에서는 수수료와 공정성을 함께 고려한 선택 정책이 필요합니다.
    pub fn drain(&mut self, max_count: usize) -> Vec<Transaction> {
        self.transactions
            .sort_by(|left, right| right.fee.cmp(&left.fee).then(left.nonce.cmp(&right.nonce)));
        let mut selected = Vec::new();
        while selected.len() < max_count && !self.transactions.is_empty() {
            selected.push(self.remove_at(0));
        }
        selected
    }

    pub fn retain_valid<F>(&mut self, mut valid: F)
    where
        F: FnMut(&Transaction) -> bool,
    {
        let retained: Vec<_> = self.transactions.drain(..).filter(|tx| valid(tx)).collect();
        *self = Self::with_limits(self.max_transactions, self.max_bytes);
        for transaction in retained {
            let _ = self.add(transaction);
        }
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    fn remove_at(&mut self, position: usize) -> Transaction {
        let transaction = self.transactions.remove(position);
        let id = transaction.id();
        self.ids.remove(&id);
        self.sender_nonce
            .remove(&(transaction.from.clone(), transaction.nonce));
        self.total_bytes = self.total_bytes.saturating_sub(
            serde_json::to_vec(&transaction)
                .map(|bytes| bytes.len())
                .unwrap_or(0),
        );
        transaction
    }
}

impl Default for Mempool {
    fn default() -> Self {
        Self::with_limits(DEFAULT_MAX_TRANSACTIONS, DEFAULT_MAX_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TransactionAction;

    fn transaction() -> Transaction {
        Transaction {
            from: "0x1111111111111111111111111111111111111111".into(),
            to: "0x2222222222222222222222222222222222222222".into(),
            amount: 1,
            fee: 1,
            nonce: 0,
            signature: "test-signature".into(),
            action: TransactionAction::Transfer,
        }
    }

    #[test]
    fn pending_transaction_can_be_found_with_prefixed_hash() {
        let mut pool = Mempool::default();
        let transaction = transaction();
        let hash = format!("0x{}", transaction.id());
        pool.add(transaction.clone()).unwrap();
        assert_eq!(pool.transaction_by_hash(&hash), Some(&transaction));
    }

    #[test]
    fn pending_nonce_advances_only_over_contiguous_sender_transactions() {
        let mut pool = Mempool::default();
        let mut first = transaction();
        first.nonce = 3;
        let mut second = transaction();
        second.nonce = 4;
        pool.add(first).unwrap();
        pool.add(second).unwrap();
        assert_eq!(
            pool.next_nonce("0x1111111111111111111111111111111111111111", 3),
            5
        );
        assert_eq!(
            pool.next_nonce("0x2222222222222222222222222222222222222222", 3),
            3
        );
    }
}
