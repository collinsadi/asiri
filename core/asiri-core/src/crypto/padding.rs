//! Length hiding (architecture §7.3 "Padding", §7.7 step 4).
//!
//! * Envelopes: every payload handed to the relay is `u32 length || payload || zeros`, rounded up
//!   to a multiple of 256 bytes, so the relay learns only coarse sizes.
//! * Attachments: the stored ciphertext is padded to a size bucket — the next power of two up to
//!   1 MiB, then the next multiple of 1 MiB.

use crate::error::{CoreError, CoreResult};

/// Envelope padding block size.
pub const ENVELOPE_BLOCK: usize = 256;

const MIB: u64 = 1024 * 1024;

/// Pads `payload` to a multiple of [`ENVELOPE_BLOCK`] with a 4-byte length prefix.
#[must_use]
pub fn pad_envelope(payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).expect("payloads are far below 4 GiB");
    let total = (4 + payload.len()).div_ceil(ENVELOPE_BLOCK) * ENVELOPE_BLOCK;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    out.resize(total, 0);
    out
}

/// Removes envelope padding, rejecting malformed input.
pub fn unpad_envelope(padded: &[u8]) -> CoreResult<&[u8]> {
    if padded.len() < 4 || !padded.len().is_multiple_of(ENVELOPE_BLOCK) {
        return Err(CoreError::Crypto("envelope padding is malformed".into()));
    }
    let len = u32::from_be_bytes([padded[0], padded[1], padded[2], padded[3]]) as usize;
    let body = padded
        .get(4..4 + len)
        .ok_or_else(|| CoreError::Crypto("envelope length is out of range".into()))?;
    if padded[4 + len..].iter().any(|b| *b != 0) {
        return Err(CoreError::Crypto("envelope padding is not zero".into()));
    }
    Ok(body)
}

/// The padded size bucket for an attachment ciphertext of `len` bytes.
#[must_use]
pub fn attachment_bucket(len: u64) -> u64 {
    if len <= 1 {
        return 1;
    }
    if len <= MIB {
        len.next_power_of_two()
    } else {
        len.div_ceil(MIB) * MIB
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn buckets() {
        assert_eq!(attachment_bucket(1000), 1024);
        assert_eq!(attachment_bucket(1024), 1024);
        assert_eq!(attachment_bucket(MIB), MIB);
        assert_eq!(attachment_bucket(MIB + 1), 2 * MIB);
        assert_eq!(attachment_bucket(5 * MIB - 3), 5 * MIB);
    }

    #[test]
    fn rejects_tampering() {
        let mut p = pad_envelope(b"hello");
        assert_eq!(unpad_envelope(&p).unwrap(), b"hello");
        p[200] = 1;
        assert!(unpad_envelope(&p).is_err());
        assert!(unpad_envelope(&p[..255]).is_err());
        let mut long = pad_envelope(b"x");
        long[0..4].copy_from_slice(&1000u32.to_be_bytes());
        assert!(unpad_envelope(&long).is_err());
    }

    proptest! {
        #[test]
        fn envelope_round_trip(payload in prop::collection::vec(any::<u8>(), 0..5000)) {
            let padded = pad_envelope(&payload);
            prop_assert_eq!(padded.len() % ENVELOPE_BLOCK, 0);
            prop_assert!(padded.len() >= payload.len() + 4);
            prop_assert!(padded.len() < payload.len() + 4 + ENVELOPE_BLOCK);
            prop_assert_eq!(unpad_envelope(&padded).unwrap(), payload.as_slice());
        }

        #[test]
        fn buckets_are_bounded(len in 1u64..(300 * MIB)) {
            let b = attachment_bucket(len);
            prop_assert!(b >= len);
            if len <= MIB { prop_assert!(b < 2 * len.max(1)); } else { prop_assert!(b - len < MIB); }
        }
    }
}
