//! Identifiers, randomness and time.

use std::time::{SystemTime, UNIX_EPOCH};

/// Fills a fixed-size array from the operating system's CSPRNG.
///
/// # Panics
/// Panics if the operating system cannot provide randomness, which is unrecoverable for a
/// cryptographic application.
#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("operating system RNG unavailable");
    out
}

/// Fills a buffer from the operating system's CSPRNG.
pub fn fill_random(buf: &mut [u8]) {
    getrandom::fill(buf).expect("operating system RNG unavailable");
}

/// A 16-byte random identifier (messages, envelopes, blobs, routes, users, devices, calls).
pub type Id16 = [u8; 16];

/// A fresh random 16-byte identifier.
#[must_use]
pub fn random_id() -> Id16 {
    random_bytes()
}

/// Parses an exact-length identifier.
#[must_use]
pub fn id16(bytes: &[u8]) -> Option<Id16> {
    bytes.try_into().ok()
}

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn now_s() -> i64 {
    now_ms() / 1000
}

/// Lowercase hex, used for IDs that cross the FFI boundary as strings.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Parses lowercase or uppercase hex.
#[must_use]
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    hex::decode(text).ok()
}
