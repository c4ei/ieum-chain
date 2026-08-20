use crate::consensus::Validator;
use crate::model::Address;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const IEUM_BASE_UNIT: u128 = 1_000_000_000_000_000_000;
pub const IEUM_MAX_SUPPLY: u128 = 210_000_000 * IEUM_BASE_UNIT;
pub const IEUM_FOUNDATION_ALLOCATION: u128 = 21_000_000 * IEUM_BASE_UNIT;
pub const IEUM_FOUNDATION_ADDRESS: &str = "0x356456ff1216b57a6f8891b195b42d296789b67d";
/// v0.23.9 재단 배분 Genesis: 2026-08-21 00:00:00 KST.
pub const IEUM_MAINNET_GENESIS_TIME: u64 = 1_787_238_000;

/// 모든 노드가 동일하게 보관해야 하는 체인 시작 설정입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisConfig {
    pub chain_id: u64,
    pub network_name: String,
    pub genesis_time: u64,
    /// 합의 규칙이 허용하는 IEUM 최대 발행량입니다.
    pub max_supply: u128,
    /// 최소 단위(1 IEUM = 10^18)입니다. 운영 배분량이 u64를 넘을 수 있어 u128을 씁니다.
    pub initial_balances: Vec<(Address, u128)>,
    /// 유통량에서 제외할 재단 락업·베스팅 주소입니다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked_addresses: Vec<Address>,
    pub validators: Vec<Validator>,
    pub max_block_bytes: u64,
    /// 모든 활성 블록 파일을 합한 최대 크기입니다. 과거 이름도 읽을 수 있습니다.
    #[serde(alias = "max_segment_bytes")]
    pub max_active_block_bytes: u64,
}

impl GenesisConfig {
    pub fn validate_production_safety(&self) -> Result<(), String> {
        const KNOWN_DEVELOPMENT_ADDRESSES: [&str; 4] = [
            "0xB0E5863D0DDf7e105e409Fee0eCC0123a362e14B",
            "0x3252b7b65e50B54508974dB8d634134B0bd6be90",
            "0xf0DCB0Ea878057Ff5C78C4737023f900ECe09e7B",
            "0xD5ac7674AC15E3Df0B7D737CF8Cb8f2Ea713F329",
        ];
        self.validate()?;
        if self.network_name.to_ascii_lowercase().contains("test") {
            return Err("mainnet strict 모드에서 test network_name을 사용할 수 없습니다.".into());
        }
        if self.initial_balances.iter().any(|(address, balance)| {
            *balance > 0
                && KNOWN_DEVELOPMENT_ADDRESSES
                    .iter()
                    .any(|known| address.eq_ignore_ascii_case(known))
        }) {
            return Err(
                "공개된 개발 개인키 주소에 genesis 잔액이 있어 mainnet strict 시작을 거부합니다."
                    .into(),
            );
        }
        if self.max_supply != IEUM_MAX_SUPPLY {
            return Err("IEUM mainnet 최대 발행량은 210,000,000 IEUM이어야 합니다.".into());
        }
        let foundation = self
            .initial_balances
            .iter()
            .find(|(address, _)| address.eq_ignore_ascii_case(IEUM_FOUNDATION_ADDRESS))
            .map(|(_, balance)| *balance)
            .unwrap_or(0);
        if foundation != IEUM_FOUNDATION_ALLOCATION {
            return Err("재단 최초 배분량은 21,000,000 IEUM이어야 합니다.".into());
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.chain_id == 0 {
            return Err("chain_id는 0일 수 없습니다.".into());
        }
        if self.max_supply == 0 {
            return Err("max_supply는 0일 수 없습니다.".into());
        }
        let mut balance_addresses = HashSet::new();
        let mut initial_supply = 0u128;
        for (address, balance) in &self.initial_balances {
            if !balance_addresses.insert(address.to_ascii_lowercase()) {
                return Err("제네시스 잔액 주소는 중복될 수 없습니다.".into());
            }
            initial_supply = initial_supply
                .checked_add(*balance)
                .ok_or("제네시스 발행량 합계가 u128 범위를 넘습니다.")?;
        }
        if initial_supply > self.max_supply {
            return Err("제네시스 발행량이 최대 발행량을 넘습니다.".into());
        }
        let mut locked = HashSet::new();
        for address in &self.locked_addresses {
            let normalized = address.to_ascii_lowercase();
            if !balance_addresses.contains(&normalized) || !locked.insert(normalized) {
                return Err("locked_addresses는 제네시스에 존재하는 고유 주소여야 합니다.".into());
            }
        }
        if self.validators.len() < 4 {
            return Err("BFT 제네시스 검증자는 최소 4개가 필요합니다.".into());
        }
        let mut validator_ids = HashSet::new();
        for validator in &self.validators {
            if validator.voting_power == 0
                || hex::decode(&validator.id)
                    .map(|bytes| bytes.len() != 32)
                    .unwrap_or(true)
                || !validator_ids.insert(&validator.id)
            {
                return Err(
                    "검증자 ID는 고유한 32바이트 Ed25519 공개키이고 투표권은 1 이상이어야 합니다."
                        .into(),
                );
            }
        }
        if self.max_block_bytes == 0 || self.max_block_bytes > 4 * 1024 * 1024 {
            return Err("모바일 호환을 위해 블록은 1바이트~4MiB여야 합니다.".into());
        }
        if self.max_active_block_bytes == 0 || self.max_active_block_bytes > 100_000_000 {
            return Err("활성 블록 전체 크기는 100MB 이하여야 합니다.".into());
        }
        Ok(())
    }

    /// 설정 파일 자체의 결정론적 식별자입니다. 운영망 배포 뒤에는 바꾸면 안 됩니다.
    pub fn genesis_hash(&self) -> Result<String, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::AccountWallet;

    #[test]
    fn bundled_genesis_is_mainnet_safe_and_excludes_public_development_balances() {
        let genesis: GenesisConfig =
            serde_json::from_str(include_str!("../config/genesis.json")).unwrap();
        genesis.validate().unwrap();
        assert_eq!(genesis.chain_id, 21_004);
        let total: u128 = genesis
            .initial_balances
            .iter()
            .map(|(_, value)| *value)
            .sum();
        assert_eq!(total, 21_070_100u128 * IEUM_BASE_UNIT);
        assert_eq!(genesis.max_supply, IEUM_MAX_SUPPLY);
        assert!(genesis.initial_balances.iter().any(|(address, balance)| {
            address.eq_ignore_ascii_case(IEUM_FOUNDATION_ADDRESS)
                && *balance == IEUM_FOUNDATION_ALLOCATION
        }));
        for (key_byte, address) in (42u8..=45).zip([
            "0xB0E5863D0DDf7e105e409Fee0eCC0123a362e14B",
            "0x3252b7b65e50B54508974dB8d634134B0bd6be90",
            "0xf0DCB0Ea878057Ff5C78C4737023f900ECe09e7B",
            "0xD5ac7674AC15E3Df0B7D737CF8Cb8f2Ea713F329",
        ]) {
            let wallet = AccountWallet::from_private_key([key_byte; 32]).unwrap();
            assert!(wallet.address().eq_ignore_ascii_case(address));
            assert!(
                !genesis
                    .initial_balances
                    .iter()
                    .any(|(candidate, _)| candidate.eq_ignore_ascii_case(address))
            );
        }
        assert_eq!(genesis.network_name, "ieum-mainnet");
        genesis.validate_production_safety().unwrap();
        assert_eq!(genesis.genesis_time, IEUM_MAINNET_GENESIS_TIME);
        assert_eq!(
            genesis.genesis_hash().unwrap(),
            "82cfc3615112766f3eb151a8677890c1b74ce6bce8463a1a3590991c383650f6"
        );
    }

    #[test]
    fn ci_genesis_keeps_the_four_transfer_test_balances() {
        let genesis: GenesisConfig =
            serde_json::from_str(include_str!("../config/genesis_test.json")).unwrap();
        genesis.validate().unwrap();
        assert_eq!(genesis.chain_id, 21_005);
        assert_eq!(genesis.network_name, "ieum-ci");
        assert_eq!(
            genesis
                .initial_balances
                .iter()
                .map(|(_, value)| *value)
                .sum::<u128>(),
            80_104u128 * 10u128.pow(18)
        );
        for address in [
            "0xB0E5863D0DDf7e105e409Fee0eCC0123a362e14B",
            "0x3252b7b65e50B54508974dB8d634134B0bd6be90",
            "0xf0DCB0Ea878057Ff5C78C4737023f900ECe09e7B",
            "0xD5ac7674AC15E3Df0B7D737CF8Cb8f2Ea713F329",
        ] {
            assert!(genesis.initial_balances.iter().any(|(candidate, balance)| {
                candidate.eq_ignore_ascii_case(address) && *balance == 10u128.pow(18)
            }));
        }
        assert!(genesis.validate_production_safety().is_err());
    }

    #[test]
    fn rejects_initial_supply_above_the_consensus_cap() {
        let mut genesis: GenesisConfig =
            serde_json::from_str(include_str!("../config/genesis.json")).unwrap();
        genesis.initial_balances[0].1 = IEUM_MAX_SUPPLY;
        assert!(genesis.validate().is_err());
    }

    #[test]
    fn mainnet_requires_the_exact_foundation_allocation() {
        let mut genesis: GenesisConfig =
            serde_json::from_str(include_str!("../config/genesis.json")).unwrap();
        let (_, balance) = genesis
            .initial_balances
            .iter_mut()
            .find(|(address, _)| address.eq_ignore_ascii_case(IEUM_FOUNDATION_ADDRESS))
            .unwrap();
        *balance -= IEUM_BASE_UNIT;
        assert!(genesis.validate_production_safety().is_err());
    }
}
