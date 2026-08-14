use crate::model::{Address, Transaction};
use bip32::{DerivationPath, XPrv};
use bip39::{Language, Mnemonic};
use k256::ecdsa::{
    Signature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use rand_core::{OsRng, RngCore};
use sha3::{Digest, Keccak256};
use std::str::FromStr;

/// 사용자 코인 계정용 secp256k1 지갑입니다.
///
/// 합의 검증자와 P2P 키는 기존 Ed25519를 유지하고, 사용자 계정만 Ethereum과
/// 같은 곡선과 주소 계산법을 사용합니다. 두 키의 역할을 섞지 않는 것이 중요합니다.
#[derive(Clone, Debug)]
pub struct AccountWallet {
    signing_key: SigningKey,
}

impl AccountWallet {
    /// 운영체제 난수로 geth가 사용하는 것과 같은 32바이트 secp256k1 개인키를 만듭니다.
    pub fn new() -> Self {
        Self {
            signing_key: SigningKey::random(&mut OsRng),
        }
    }

    pub fn from_private_key(bytes: [u8; 32]) -> Result<Self, String> {
        let signing_key = SigningKey::from_bytes((&bytes).into())
            .map_err(|_| "유효하지 않은 secp256k1 개인키입니다.")?;
        Ok(Self { signing_key })
    }

    pub fn from_private_key_hex(value: &str) -> Result<Self, String> {
        let bytes: [u8; 32] = hex::decode(value.trim_start_matches("0x"))
            .map_err(|_| "개인키가 hex 문자열이 아닙니다.")?
            .try_into()
            .map_err(|_| "개인키는 정확히 32바이트여야 합니다.")?;
        Self::from_private_key(bytes)
    }

    /// BIP-39 문구와 geth/MetaMask 표준 BIP-44 경로로 계정을 복원합니다.
    pub fn from_mnemonic(words: &str, index: u32) -> Result<Self, String> {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, words)
            .map_err(|error| format!("BIP-39 seed 문구 오류: {error}"))?;
        let path = DerivationPath::from_str(&format!("m/44'/60'/0'/0/{index}"))
            .map_err(|error| format!("HD 파생 경로 오류: {error}"))?;
        let key = XPrv::derive_from_path(mnemonic.to_seed(""), &path)
            .map_err(|error| format!("HD 개인키 파생 오류: {error}"))?;
        Self::from_private_key(key.private_key().to_bytes().into())
    }

    /// 128비트 엔트로피의 12단어 BIP-39 문구를 생성합니다.
    pub fn generate_mnemonic() -> Result<String, String> {
        let mut entropy = [0u8; 16];
        OsRng.fill_bytes(&mut entropy);
        Mnemonic::from_entropy_in(Language::English, &entropy)
            .map(|mnemonic| mnemonic.to_string())
            .map_err(|error| format!("BIP-39 seed 생성 오류: {error}"))
    }

    /// Ethereum 표준: 비압축 공개키(0x04 제외)의 Keccak-256 마지막 20바이트입니다.
    pub fn address(&self) -> Address {
        ethereum_address(self.signing_key.verifying_key())
    }

    /// 암호화 keystore 저장 직전에만 사용하는 개인키 바이트입니다.
    /// 호출자는 평문을 파일·로그·RPC 응답에 남기면 안 됩니다.
    pub(crate) fn private_key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes().into()
    }

    /// 내부 IEUM 거래 서명은 공개키와 ECDSA 서명을 함께 담습니다.
    ///
    /// 주소와 키는 geth 호환이지만 이 직렬화는 Ethereum RLP raw transaction이 아닙니다.
    pub fn sign_bytes(&self, message: &[u8]) -> String {
        let signature: Signature = self.signing_key.sign(message);
        let public_key = self.signing_key.verifying_key().to_encoded_point(false);
        let mut bytes = Vec::with_capacity(129);
        bytes.extend_from_slice(public_key.as_bytes());
        bytes.extend_from_slice(signature.to_bytes().as_ref());
        hex::encode(bytes)
    }

    pub fn sign_transfer(&self, to: Address, amount: u128, fee: u128, nonce: u64) -> Transaction {
        self.sign_action(
            to,
            amount,
            fee,
            nonce,
            crate::model::TransactionAction::Transfer,
        )
    }

    pub fn sign_action(
        &self,
        to: Address,
        amount: u128,
        fee: u128,
        nonce: u64,
        action: crate::model::TransactionAction,
    ) -> Transaction {
        let mut tx = Transaction {
            from: self.address(),
            to,
            amount,
            fee,
            nonce,
            action,
            signature: String::new(),
        };
        tx.signature = self.sign_bytes(&tx.signing_bytes());
        tx
    }
}

impl Default for AccountWallet {
    fn default() -> Self {
        Self::new()
    }
}

pub fn verify_account_signature(
    address: &str,
    message: &[u8],
    encoded_signature: &str,
) -> Result<(), String> {
    let bytes = hex::decode(encoded_signature).map_err(|_| "서명이 hex 문자열이 아닙니다.")?;
    if bytes.len() != 129 {
        return Err("secp256k1 서명은 공개키 65바이트와 서명 64바이트여야 합니다.".into());
    }
    let verifying_key = VerifyingKey::from_sec1_bytes(&bytes[..65])
        .map_err(|_| "secp256k1 공개키가 잘못되었습니다.")?;
    if ethereum_address(&verifying_key) != normalize_address(address) {
        return Err("서명 공개키에서 계산한 Ethereum 주소가 보내는 주소와 다릅니다.".into());
    }
    let signature =
        Signature::from_slice(&bytes[65..]).map_err(|_| "ECDSA 서명이 잘못되었습니다.")?;
    verifying_key
        .verify(message, &signature)
        .map_err(|_| "전자서명이 일치하지 않습니다.".into())
}

fn ethereum_address(verifying_key: &VerifyingKey) -> String {
    let point = verifying_key.to_encoded_point(false);
    let digest = Keccak256::digest(&point.as_bytes()[1..]);
    format!("0x{}", hex::encode(&digest[12..]))
}

fn normalize_address(address: &str) -> String {
    format!(
        "0x{}",
        address.trim_start_matches("0x").to_ascii_lowercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ethereum_known_private_key_produces_known_address() {
        let wallet = AccountWallet::from_private_key_hex(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        assert_eq!(
            wallet.address(),
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"
        );
    }

    #[test]
    fn bip39_standard_path_is_deterministic() {
        let words = "test test test test test test test test test test test junk";
        let wallet = AccountWallet::from_mnemonic(words, 0).unwrap();
        assert_eq!(
            wallet.address(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }
}
