use anyhow::{Result, anyhow};

pub(crate) fn encode_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub(crate) fn decode_u64(payload: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = payload.try_into().map_err(|_| {
        anyhow!(
            "expected an 8-byte u64 payload, got {} bytes",
            payload.len()
        )
    })?;
    Ok(u64::from_le_bytes(bytes))
}
