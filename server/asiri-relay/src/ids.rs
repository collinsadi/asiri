//! Random identifiers, time helpers and small encodings shared across the relay.

use std::time::{SystemTime, UNIX_EPOCH};

/// A 16-byte random identifier (device, inbox, envelope, blob, group route).
pub type Id = [u8; asiri_proto::ID_LEN];

/// Fills a fixed-size array from the operating system's CSPRNG.
///
/// # Panics
/// Panics if the operating system cannot provide randomness, which is unrecoverable for a
/// cryptographic service.
#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("operating system RNG unavailable");
    out
}

/// A fresh random [`Id`].
#[must_use]
pub fn random_id() -> Id {
    random_bytes()
}

/// Parses a wire identifier, rejecting anything that is not exactly 16 bytes.
#[must_use]
pub fn parse_id(bytes: &[u8]) -> Option<Id> {
    bytes.try_into().ok()
}

/// Parses a 32-character lowercase/uppercase hex identifier (used in URLs and the CLI).
#[must_use]
pub fn parse_hex_id(text: &str) -> Option<Id> {
    let bytes = hex::decode(text).ok()?;
    parse_id(&bytes)
}

/// Current Unix time in seconds.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Coarse day number used for retention bookkeeping, so the relay never stores precise activity
/// times (architecture §9.6).
#[must_use]
pub fn day(unix_seconds: i64) -> i64 {
    unix_seconds / 86_400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_and_parse() {
        let a = random_id();
        let b = random_id();
        assert_ne!(a, b);
        assert_eq!(parse_id(&a), Some(a));
        assert_eq!(parse_id(&a[..15]), None);
        assert_eq!(parse_hex_id(&hex::encode(a)), Some(a));
        assert_eq!(parse_hex_id("zz"), None);
    }
}
