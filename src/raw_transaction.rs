use crate::model::{Transaction, TransactionAction};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha3::{Digest, Keccak256};

/// EIP-155 legacy(9개 RLP 필드) 송금 거래를 검증해 IEUM 거래로 변환합니다.
/// 컨트랙트 생성과 calldata 실행은 EVM이 추가되기 전까지 거부합니다.
pub fn decode_legacy(raw_hex: &str, expected_chain_id: u64) -> Result<Transaction, String> {
    let raw = hex::decode(raw_hex.trim_start_matches("0x"))
        .map_err(|_| "raw transaction이 올바른 hex가 아닙니다.")?;
    let fields = decode_list(&raw)?;
    if fields.len() != 9 {
        return Err("v0.0.4는 EIP-155 legacy RLP 거래만 지원합니다.".into());
    }
    let nonce = value_u64(fields[0], "nonce")?;
    let gas_price = value_u64(fields[1], "gasPrice")?;
    let gas_limit = value_u64(fields[2], "gasLimit")?;
    let to = fields[3];
    if to.len() != 20 {
        return Err("현재는 20바이트 수신 주소 송금만 지원합니다.".into());
    }
    let amount = value_u128(fields[4], "value")?;
    let action = decode_action(to, fields[5])?;
    let v = value_u64(fields[6], "v")?;
    if v < 35 {
        return Err("EIP-155 체인 ID가 없는 거래는 재생 공격 방지를 위해 거부합니다.".into());
    }
    let chain_id = (v - 35) / 2;
    if chain_id != expected_chain_id {
        return Err(format!(
            "체인 ID 불일치: 기대 {expected_chain_id}, 입력 {chain_id}"
        ));
    }
    let recovery_id =
        RecoveryId::from_byte(((v - 35) % 2) as u8).ok_or("복구 ID가 올바르지 않습니다.")?;
    let r = padded_scalar(fields[7], "r")?;
    let s = padded_scalar(fields[8], "s")?;
    let signature = Signature::from_scalars(r, s).map_err(|_| "ECDSA r/s가 잘못되었습니다.")?;

    let signing = encode_list(&[
        encode_u64(nonce),
        encode_u64(gas_price),
        encode_u64(gas_limit),
        encode_bytes(to),
        encode_u128(amount),
        encode_bytes(fields[5]),
        encode_u64(chain_id),
        encode_bytes(&[]),
        encode_bytes(&[]),
    ]);
    let digest = Keccak256::digest(signing);
    let key = VerifyingKey::recover_from_prehash(&digest, &signature, recovery_id)
        .map_err(|_| "raw transaction 서명에서 공개키를 복구하지 못했습니다.")?;
    let point = key.to_encoded_point(false);
    let address_hash = Keccak256::digest(&point.as_bytes()[1..]);
    let from = format!("0x{}", hex::encode(&address_hash[12..]));
    let fee = u128::from(gas_price)
        .checked_mul(u128::from(gas_limit))
        .ok_or("gasPrice와 gasLimit의 곱이 u128 범위를 넘습니다.")?;
    Ok(Transaction {
        from,
        to: format!("0x{}", hex::encode(to)),
        amount,
        fee,
        nonce,
        action,
        signature: format!("ethraw:{}", hex::encode(raw)),
    })
}

fn decode_action(to: &[u8], data: &[u8]) -> Result<TransactionAction, String> {
    if data.is_empty() {
        return Ok(TransactionAction::Transfer);
    }
    if format!("0x{}", hex::encode(to)) != crate::staking::STAKING_SYSTEM_ADDRESS {
        return Err("EVM calldata는 스테이킹 시스템 주소에서만 지원합니다.".into());
    }
    let text = std::str::from_utf8(data).map_err(|_| "스테이킹 calldata는 UTF-8이어야 합니다.")?;
    if text == "claim" {
        return Ok(TransactionAction::ClaimUnbonded);
    }
    if let Some(v) = text.strip_prefix("delegate:") {
        crate::staking::validate_validator(v)?;
        return Ok(TransactionAction::Delegate {
            validator: v.into(),
        });
    }
    if let Some(v) = text.strip_prefix("undelegate:") {
        crate::staking::validate_validator(v)?;
        return Ok(TransactionAction::Undelegate {
            validator: v.into(),
        });
    }
    Err("지원하지 않는 스테이킹 calldata입니다.".into())
}

pub fn verify_embedded(transaction: &Transaction, expected_chain_id: u64) -> Result<(), String> {
    let raw = transaction
        .signature
        .strip_prefix("ethraw:")
        .ok_or("Ethereum raw transaction 표식이 없습니다.")?;
    let bytes = hex::decode(raw).map_err(|_| "raw transaction이 올바른 hex가 아닙니다.")?;
    let fields = decode_list(&bytes)?;
    if fields.len() != 9 {
        return Err("legacy RLP 거래 필드 수가 9개가 아닙니다.".into());
    }
    let v = value_u64(fields[6], "v")?;
    if v < 35 {
        return Err("EIP-155 체인 ID가 없는 거래입니다.".into());
    }
    let decoded = decode_legacy(raw, expected_chain_id)?;
    if &decoded == transaction {
        Ok(())
    } else {
        Err("raw transaction과 원장 거래 필드가 일치하지 않습니다.".into())
    }
}

fn value_u64(value: &[u8], name: &str) -> Result<u64, String> {
    if value.len() > 8 {
        return Err(format!("{name}이 u64 범위를 벗어났습니다."));
    }
    Ok(value
        .iter()
        .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
}

fn value_u128(value: &[u8], name: &str) -> Result<u128, String> {
    if value.len() > 16 {
        return Err(format!("{name}이 u128 범위를 벗어났습니다."));
    }
    Ok(value
        .iter()
        .fold(0u128, |acc, byte| (acc << 8) | u128::from(*byte)))
}

fn padded_scalar(value: &[u8], name: &str) -> Result<[u8; 32], String> {
    if value.len() > 32 {
        return Err(format!("서명 {name}이 32바이트를 넘습니다."));
    }
    let mut out = [0u8; 32];
    out[32 - value.len()..].copy_from_slice(value);
    Ok(out)
}

fn decode_list(input: &[u8]) -> Result<Vec<&[u8]>, String> {
    let (payload, consumed) = decode_item(input, true)?;
    if consumed != input.len() {
        return Err("RLP 목록 뒤에 불필요한 바이트가 있습니다.".into());
    }
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < payload.len() {
        let (value, size) = decode_item(&payload[offset..], false)?;
        fields.push(value);
        offset += size;
    }
    Ok(fields)
}

fn decode_item(input: &[u8], require_list: bool) -> Result<(&[u8], usize), String> {
    let prefix = *input.first().ok_or("빈 RLP입니다.")?;
    let (is_list, start, length) = match prefix {
        0x00..=0x7f => (false, 0, 1),
        0x80..=0xb7 => (false, 1, usize::from(prefix - 0x80)),
        0xb8..=0xbf => {
            let width = usize::from(prefix - 0xb7);
            let length = read_length(input, 1, width)?;
            (false, 1 + width, length)
        }
        0xc0..=0xf7 => (true, 1, usize::from(prefix - 0xc0)),
        _ => {
            let width = usize::from(prefix - 0xf7);
            let length = read_length(input, 1, width)?;
            (true, 1 + width, length)
        }
    };
    if require_list != is_list {
        return Err("예상한 RLP 항목 종류와 다릅니다.".into());
    }
    let end = start.checked_add(length).ok_or("RLP 길이가 넘쳤습니다.")?;
    if end > input.len() {
        return Err("RLP 길이가 실제 데이터보다 큽니다.".into());
    }
    Ok((&input[start..end], end))
}

fn read_length(input: &[u8], start: usize, width: usize) -> Result<usize, String> {
    if width == 0 || width > 8 || start + width > input.len() {
        return Err("RLP 길이 필드가 잘못되었습니다.".into());
    }
    Ok(input[start..start + width]
        .iter()
        .fold(0usize, |acc, byte| (acc << 8) | usize::from(*byte)))
}

fn encode_u64(value: u64) -> Vec<u8> {
    if value == 0 {
        return encode_bytes(&[]);
    }
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(7);
    encode_bytes(&bytes[first..])
}

fn encode_u128(value: u128) -> Vec<u8> {
    if value == 0 {
        return encode_bytes(&[]);
    }
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(15);
    encode_bytes(&bytes[first..])
}

fn encode_bytes(value: &[u8]) -> Vec<u8> {
    if value.len() == 1 && value[0] < 0x80 {
        return value.to_vec();
    }
    let mut out = encode_length(value.len(), 0x80, 0xb7);
    out.extend_from_slice(value);
    out
}

fn encode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload: Vec<u8> = items.iter().flatten().copied().collect();
    let mut out = encode_length(payload.len(), 0xc0, 0xf7);
    out.extend(payload);
    out
}

fn encode_length(length: usize, short_base: u8, long_base: u8) -> Vec<u8> {
    if length <= 55 {
        return vec![short_base + length as u8];
    }
    let bytes = length.to_be_bytes();
    let first = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len() - 1);
    let encoded = &bytes[first..];
    let mut out = vec![long_base + encoded.len() as u8];
    out.extend_from_slice(encoded);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;

    fn signed_raw(to: [u8; 20], amount: u128, data: &[u8], chain_id: u64) -> String {
        let fields = [
            encode_u64(0),
            encode_u64(1),
            encode_u64(50_000),
            encode_bytes(&to),
            encode_u128(amount),
            encode_bytes(data),
            encode_u64(chain_id),
            encode_bytes(&[]),
            encode_bytes(&[]),
        ];
        let signing = encode_list(&fields);
        let key = SigningKey::from_bytes((&[1u8; 32]).into()).unwrap();
        let (signature, recovery_id) = key
            .sign_digest_recoverable(Keccak256::new_with_prefix(&signing))
            .unwrap();
        let bytes = signature.to_bytes();
        hex::encode(encode_list(&[
            encode_u64(0),
            encode_u64(1),
            encode_u64(50_000),
            encode_bytes(&to),
            encode_u128(amount),
            encode_bytes(data),
            encode_u64(35 + chain_id * 2 + u64::from(recovery_id.to_byte())),
            encode_bytes(&bytes[..32]),
            encode_bytes(&bytes[32..]),
        ]))
    }

    #[test]
    fn staking_calldata_decodes_only_at_system_address() {
        let to: [u8; 20] =
            hex::decode(crate::staking::STAKING_SYSTEM_ADDRESS.trim_start_matches("0x"))
                .unwrap()
                .try_into()
                .unwrap();
        let validator = "11".repeat(32);
        let raw = signed_raw(
            to,
            crate::staking::MINIMUM_DELEGATION,
            format!("delegate:{validator}").as_bytes(),
            21_004,
        );
        let tx = decode_legacy(&raw, 21_004).unwrap();
        assert_eq!(tx.action, TransactionAction::Delegate { validator });
        verify_embedded(&tx, 21_004).unwrap();
    }

    #[test]
    fn eip155_legacy_transfer_recovers_sender_and_fields() {
        let chain_id = 31_337;
        let to = [2u8; 20];
        let signing = encode_list(&[
            encode_u64(0),
            encode_u64(1),
            encode_u64(21_000),
            encode_bytes(&to),
            encode_u64(100),
            encode_bytes(&[]),
            encode_u64(chain_id),
            encode_bytes(&[]),
            encode_bytes(&[]),
        ]);
        let key = SigningKey::from_bytes(
            (&[
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 1,
            ])
                .into(),
        )
        .unwrap();
        let (signature, recovery_id) = key
            .sign_digest_recoverable(Keccak256::new_with_prefix(&signing))
            .unwrap();
        let signature_bytes = signature.to_bytes();
        let v = 35 + chain_id * 2 + u64::from(recovery_id.to_byte());
        let raw = encode_list(&[
            encode_u64(0),
            encode_u64(1),
            encode_u64(21_000),
            encode_bytes(&to),
            encode_u64(100),
            encode_bytes(&[]),
            encode_u64(v),
            encode_bytes(&signature_bytes[..32]),
            encode_bytes(&signature_bytes[32..]),
        ]);
        let transaction = decode_legacy(&hex::encode(raw), chain_id).unwrap();

        assert_eq!(
            transaction.from,
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"
        );
        assert_eq!(transaction.to, format!("0x{}", hex::encode(to)));
        assert_eq!(transaction.amount, 100);
        assert_eq!(transaction.fee, 21_000);
        assert_eq!(transaction.nonce, 0);
        verify_embedded(&transaction, chain_id).unwrap();
    }

    #[test]
    fn legacy_transfer_accepts_value_larger_than_u64() {
        let chain_id = 21_004;
        let amount = u128::from(u64::MAX) + 1;
        let to = [3u8; 20];
        let signing = encode_list(&[
            encode_u64(0),
            encode_u64(1),
            encode_u64(21_000),
            encode_bytes(&to),
            encode_u128(amount),
            encode_bytes(&[]),
            encode_u64(chain_id),
            encode_bytes(&[]),
            encode_bytes(&[]),
        ]);
        let key = SigningKey::from_bytes((&[1u8; 32]).into()).unwrap();
        let (signature, recovery_id) = key
            .sign_digest_recoverable(Keccak256::new_with_prefix(&signing))
            .unwrap();
        let signature_bytes = signature.to_bytes();
        let v = 35 + chain_id * 2 + u64::from(recovery_id.to_byte());
        let raw = encode_list(&[
            encode_u64(0),
            encode_u64(1),
            encode_u64(21_000),
            encode_bytes(&to),
            encode_u128(amount),
            encode_bytes(&[]),
            encode_u64(v),
            encode_bytes(&signature_bytes[..32]),
            encode_bytes(&signature_bytes[32..]),
        ]);
        assert_eq!(
            decode_legacy(&hex::encode(raw), chain_id).unwrap().amount,
            amount
        );
    }
}
