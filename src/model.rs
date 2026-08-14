use crate::scheduled_event::ScheduledEvent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Address = String;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransactionAction {
    #[default]
    Transfer,
    Delegate {
        validator: Address,
    },
    Undelegate {
        validator: Address,
    },
    ClaimUnbonded,
}

/// 사용자가 서명해 네트워크에 제출하는 가장 기본적인 코인 송금 거래입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transaction {
    pub from: Address,
    pub to: Address,
    /// 1 IEUM = 10^18 최소 단위이므로 전체 발행량을 안전하게 담도록 u128을 사용합니다.
    #[serde(with = "decimal_u128")]
    pub amount: u128,
    #[serde(with = "decimal_u128")]
    pub fee: u128,
    pub nonce: u64,
    /// v0.23.0 이전 거래에는 필드가 없으며 일반 송금으로 해석합니다.
    #[serde(default, skip_serializing_if = "TransactionAction::is_transfer")]
    pub action: TransactionAction,
    pub signature: String,
}

impl TransactionAction {
    pub fn is_transfer(&self) -> bool {
        matches!(self, Self::Transfer)
    }
    fn consensus_bytes(&self) -> Vec<u8> {
        match self {
            Self::Transfer => vec![0],
            Self::Delegate { validator } => {
                let mut out = vec![1];
                push_text(&mut out, validator);
                out
            }
            Self::Undelegate { validator } => {
                let mut out = vec![2];
                push_text(&mut out, validator);
                out
            }
            Self::ClaimUnbonded => vec![3],
        }
    }
}

/// JSON 구현의 128비트 숫자 지원 여부와 무관한 고정 표현입니다.
/// 새 데이터는 십진 문자열로 쓰고, 기존 원장/P2P JSON의 숫자도 계속 읽습니다.
pub(crate) mod decimal_u128 {
    use serde::de::{self, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DecimalU128Visitor;

        impl Visitor<'_> for DecimalU128Visitor {
            type Value = u128;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("u128 범위의 십진 문자열 또는 음수가 아닌 정수")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(u128::from(value))
            }

            fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E> {
                Ok(value)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value.parse::<u128>().map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(DecimalU128Visitor)
    }
}

impl Transaction {
    /// 직렬화 구현이 바뀌어도 같은 서명 결과가 나오도록 필드를 고정 순서로 조합합니다.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_text(&mut bytes, &self.from);
        push_text(&mut bytes, &self.to);
        bytes.extend_from_slice(&self.amount.to_be_bytes());
        bytes.extend_from_slice(&self.fee.to_be_bytes());
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        if !self.action.is_transfer() {
            bytes.extend_from_slice(b"IEUM-TX-ACTION-V1");
            bytes.extend_from_slice(&self.action.consensus_bytes());
        }
        bytes
    }

    /// mempool 중복 검사와 블록 해시에 사용할 거래 식별자입니다.
    pub fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_bytes());
        hasher.update(self.signature.as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// 확정된 거래 묶음입니다. 빈 거래 블록은 만들지 않는 것이 이 체인의 기본 정책입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub height: u64,
    pub previous_hash: String,
    pub timestamp: u64,
    pub producer: Address,
    pub transactions: Vec<Transaction>,
    /// 합의된 시각에 실행하는 시스템 이벤트입니다. 기존 블록 호환을 위해 기본값은 빈 목록입니다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_events: Vec<ScheduledEvent>,
    pub hash: String,
}

impl Block {
    /// 모든 노드가 동일하게 시작하는 0번 블록입니다.
    pub fn genesis() -> Self {
        Self::genesis_at(0)
    }

    /// 제네시스 설정에 기록된 합의 시각으로 0번 블록을 만듭니다.
    pub fn genesis_at(timestamp: u64) -> Self {
        let mut block = Self {
            height: 0,
            previous_hash: "0".repeat(64),
            timestamp,
            producer: "genesis".into(),
            transactions: vec![],
            system_events: vec![],
            hash: String::new(),
        };
        block.hash = block.calculate_hash();
        block
    }

    pub fn new(
        height: u64,
        previous_hash: String,
        timestamp: u64,
        producer: Address,
        transactions: Vec<Transaction>,
    ) -> Self {
        let mut block = Self {
            height,
            previous_hash,
            timestamp,
            producer,
            transactions,
            system_events: vec![],
            hash: String::new(),
        };
        block.hash = block.calculate_hash();
        block
    }

    /// 블록의 모든 합의 대상 필드를 고정 순서로 해시합니다.
    pub fn calculate_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.height.to_be_bytes());
        hasher.update(self.previous_hash.as_bytes());
        hasher.update(self.timestamp.to_be_bytes());
        hasher.update(self.producer.as_bytes());
        for tx in &self.transactions {
            hasher.update(tx.id().as_bytes());
        }
        // v0.16.x까지 생성된 블록은 이 필드가 없으므로 빈 목록은 해시에 넣지 않습니다.
        for event in &self.system_events {
            hasher.update(event.id.as_bytes());
            hasher.update(event.consensus_bytes());
        }
        hex::encode(hasher.finalize())
    }

    pub fn with_system_events(mut self, events: Vec<ScheduledEvent>) -> Self {
        self.system_events = events;
        self.hash = self.calculate_hash();
        self
    }
}

fn push_text(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}
