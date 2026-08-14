use crate::model::{Address, Transaction};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;

#[derive(Debug)]
pub struct Wallet {
    signing_key: SigningKey,
}

impl Wallet {
    /// 운영체제의 안전한 난수 생성기를 사용해 새 Ed25519 개인키를 만듭니다.
    pub fn new() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// 테스트에서 항상 같은 키를 재현할 때만 사용합니다.
    /// 운영 환경에서 사람이 정한 seed를 넣으면 개인키가 쉽게 탈취될 수 있습니다.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&seed),
        }
    }

    /// 현재 예제에서는 공개키의 hex 표현을 주소로 사용합니다.
    /// 추후 주소 형식과 checksum을 도입할 때 이 부분을 교체합니다.
    pub fn address(&self) -> Address {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    /// 거래 서명 외에도 합의 메시지, 체크포인트 등에 사용할 공통 서명 함수입니다.
    pub fn sign_bytes(&self, message: &[u8]) -> String {
        hex::encode(self.signing_key.sign(message).to_bytes())
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

impl Default for Wallet {
    fn default() -> Self {
        Self::new()
    }
}

pub fn verify_transaction(tx: &Transaction) -> Result<(), String> {
    if tx.signature.starts_with("ethraw:") {
        // raw 거래는 decode_legacy에서 EIP-155 서명과 chain_id를 검증합니다.
        // 원장 적용 시에는 현재 네트워크 ID로 다시 검증해야 하므로 chain.rs에서 처리합니다.
        return Ok(());
    }
    if tx.from.starts_with("0x") {
        return crate::account::verify_account_signature(
            &tx.from,
            &tx.signing_bytes(),
            &tx.signature,
        );
    }
    verify_signature(&tx.from, &tx.signing_bytes(), &tx.signature)
}

/// 공개키 주소와 메시지, 서명만으로 Ed25519 서명을 검증합니다.
/// 네트워크에서 받은 데이터는 상태에 반영하기 전에 반드시 이 함수를 거쳐야 합니다.
pub fn verify_signature(address: &str, message: &[u8], signature: &str) -> Result<(), String> {
    let public_key_bytes: [u8; 32] = hex::decode(address)
        .map_err(|_| "보내는 주소가 hex 문자열이 아닙니다.")?
        .try_into()
        .map_err(|_| "보내는 주소 길이가 잘못되었습니다.")?;
    let signature_bytes: [u8; 64] = hex::decode(signature)
        .map_err(|_| "서명이 hex 문자열이 아닙니다.")?
        .try_into()
        .map_err(|_| "서명 길이가 잘못되었습니다.")?;
    let public_key =
        VerifyingKey::from_bytes(&public_key_bytes).map_err(|_| "공개키가 잘못되었습니다.")?;
    public_key
        .verify(message, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| "전자서명이 일치하지 않습니다.".into())
}
