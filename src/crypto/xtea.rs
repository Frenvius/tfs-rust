use thiserror::Error;

pub type Key = [u32; 4];
pub type RoundKeys = [u32; 64];

const DELTA: u32 = 0x9E37_79B9;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum XteaError {
    #[error("XTEA input length must be a multiple of 8 bytes")]
    InvalidBlockLength,
}

pub fn expand_key(key: &Key) -> RoundKeys {
    let mut expanded = [0u32; 64];
    let mut sum = 0u32;
    let mut next_sum = sum.wrapping_add(DELTA);

    for i in (0..expanded.len()).step_by(2) {
        expanded[i] = sum.wrapping_add(key[(sum & 3) as usize]);
        expanded[i + 1] = next_sum.wrapping_add(key[((next_sum >> 11) & 3) as usize]);
        sum = next_sum;
        next_sum = next_sum.wrapping_add(DELTA);
    }

    expanded
}

pub fn encrypt(data: &mut [u8], key: &RoundKeys) -> Result<(), XteaError> {
    if data.len() % 8 != 0 {
        return Err(XteaError::InvalidBlockLength);
    }

    for i in (0..key.len()).step_by(2) {
        apply_rounds(data, |left, right| {
            *left = left
                .wrapping_add(((((*right) << 4) ^ ((*right) >> 5)).wrapping_add(*right)) ^ key[i]);
            *right = right
                .wrapping_add(((((*left) << 4) ^ ((*left) >> 5)).wrapping_add(*left)) ^ key[i + 1]);
        });
    }

    Ok(())
}

pub fn decrypt(data: &mut [u8], key: &RoundKeys) -> Result<(), XteaError> {
    if data.len() % 8 != 0 {
        return Err(XteaError::InvalidBlockLength);
    }

    for i in (1..key.len()).rev().step_by(2) {
        apply_rounds(data, |left, right| {
            *right = right
                .wrapping_sub(((((*left) << 4) ^ ((*left) >> 5)).wrapping_add(*left)) ^ key[i]);
            *left = left.wrapping_sub(
                ((((*right) << 4) ^ ((*right) >> 5)).wrapping_add(*right)) ^ key[i - 1],
            );
        });
    }

    Ok(())
}

fn apply_rounds(data: &mut [u8], mut round: impl FnMut(&mut u32, &mut u32)) {
    for block in data.chunks_exact_mut(8) {
        let mut left = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut right = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);

        round(&mut left, &mut right);

        block[..4].copy_from_slice(&left.to_le_bytes());
        block[4..].copy_from_slice(&right.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::{decrypt, encrypt, expand_key, Key, XteaError};

    #[test]
    fn expand_key_should_match_the_upstream_schedule_for_a_zero_key() {
        let expanded = expand_key(&[0, 0, 0, 0]);

        assert_eq!(expanded[0], 0);
        assert_eq!(expanded[1], 0x9E37_79B9);
        assert_eq!(expanded[2], 0x9E37_79B9);
        assert_eq!(expanded[3], 0x3C6E_F372);
    }

    #[test]
    fn encrypt_and_decrypt_should_round_trip_a_known_block() {
        let key: Key = [0x0011_2233, 0x4455_6677, 0x8899_AABB, 0xCCDD_EEFF];
        let round_keys = expand_key(&key);
        let original = [0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE];
        let mut encrypted = original;

        encrypt(&mut encrypted, &round_keys).expect("block length should be valid");
        decrypt(&mut encrypted, &round_keys).expect("block length should be valid");

        assert_eq!(encrypted, original);
    }

    #[test]
    fn encrypt_should_reject_non_aligned_input() {
        let round_keys = expand_key(&[0, 0, 0, 0]);
        let mut bytes = [0u8; 7];

        let error = encrypt(&mut bytes, &round_keys).expect_err("misaligned input should fail");

        assert_eq!(error, XteaError::InvalidBlockLength);
    }
}
