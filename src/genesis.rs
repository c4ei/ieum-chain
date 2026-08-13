use crate::consensus::Validator;
use crate::model::Address;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// 모든 노드가 동일하게 보관해야 하는 체인 시작 설정입니다.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisConfig {
    pub chain_id: u64,
    pub network_name: String,
    pub genesis_time: u64,
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
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.chain_id == 0 {
            return Err("chain_id는 0일 수 없습니다.".into());
        }
        let balance_addresses: HashSet<_> = self
            .initial_balances
            .iter()
            .map(|(address, _)| address.to_ascii_lowercase())
            .collect();
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
    fn bundled_genesis_includes_transfer_test_balance() {
        let genesis: GenesisConfig =
            serde_json::from_str(include_str!("../config/genesis.json")).unwrap();
        genesis.validate().unwrap();
        let total: u128 = genesis
            .initial_balances
            .iter()
            .map(|(_, value)| *value)
            .sum();
        assert_eq!(total, 80_104u128 * 10u128.pow(18));
        for (key_byte, address) in (42u8..=45).zip([
            "0xB0E5863D0DDf7e105e409Fee0eCC0123a362e14B",
            "0x3252b7b65e50B54508974dB8d634134B0bd6be90",
            "0xf0DCB0Ea878057Ff5C78C4737023f900ECe09e7B",
            "0xD5ac7674AC15E3Df0B7D737CF8Cb8f2Ea713F329",
        ]) {
            let wallet = AccountWallet::from_private_key([key_byte; 32]).unwrap();
            assert!(wallet.address().eq_ignore_ascii_case(address));
            assert!(genesis.initial_balances.iter().any(|(candidate, balance)| {
                candidate.eq_ignore_ascii_case(address) && *balance == 10u128.pow(18)
            }));
        }
        assert_eq!(genesis.genesis_time, 1_785_942_000);
        assert_eq!(genesis.genesis_hash().unwrap().len(), 64);
    }
}
