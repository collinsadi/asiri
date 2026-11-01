//! Asiri's wire protocols, generated from `proto/` with `cargo run -p proto-gen`.
//!
//! * [`relay::v1`] — frames between a device and its relay (architecture §9.3).
//! * [`app::v1`] — the content of MLS application messages (architecture §12).
//! * [`pairing::v1`] — in-person QR payloads (architecture §11.2).
//!
//! The generated code is committed; CI verifies it is current.

#![forbid(unsafe_code)]

/// Relay protocol (`asiri.relay.v1`).
pub mod relay {
    /// Version 1.
    pub mod v1 {
        #![allow(missing_docs, clippy::all)]
        include!("gen/asiri.relay.v1.rs");
    }
}

/// App message protocol (`asiri.app.v1`).
pub mod app {
    /// Version 1.
    pub mod v1 {
        #![allow(missing_docs, clippy::all)]
        include!("gen/asiri.app.v1.rs");
    }
}

/// Pairing payloads (`asiri.pairing.v1`).
pub mod pairing {
    /// Version 1.
    pub mod v1 {
        #![allow(missing_docs, clippy::all)]
        include!("gen/asiri.pairing.v1.rs");
    }
}

pub use prost::Message;

pub mod pairing_text;

/// The relay protocol version spoken by this build.
pub const RELAY_PROTOCOL_VERSION: u32 = 1;

/// Length of every random identifier on the wire: device, inbox, envelope, blob, group route.
pub const ID_LEN: usize = 16;

/// Domain-separation prefix for the WebSocket challenge signature (§8.4).
pub const AUTH_CONTEXT: &[u8] = b"asiri-relay-auth-v1";

/// Domain-separation prefix for the registration signature (§9.4).
pub const REGISTER_CONTEXT: &[u8] = b"asiri-relay-register-v1";

/// Maximum size of a single WebSocket frame accepted by either side.
/// Envelopes are capped at 256 KiB (§9.8); the extra room covers framing.
pub const MAX_FRAME_BYTES: usize = 300 * 1024;

/// Builds the exact byte string a device signs to answer an [`relay::v1::AuthChallenge`].
#[must_use]
pub fn auth_transcript(nonce: &[u8], service_address: &str) -> Vec<u8> {
    let mut msg = Vec::with_capacity(AUTH_CONTEXT.len() + nonce.len() + service_address.len());
    msg.extend_from_slice(AUTH_CONTEXT);
    msg.extend_from_slice(nonce);
    msg.extend_from_slice(service_address.as_bytes());
    msg
}

/// Builds the exact byte string a device signs in a [`relay::v1::RegisterRequest`].
#[must_use]
pub fn register_transcript(registration_secret: &[u8], auth_pubkey: &[u8]) -> Vec<u8> {
    let mut msg =
        Vec::with_capacity(REGISTER_CONTEXT.len() + registration_secret.len() + auth_pubkey.len());
    msg.extend_from_slice(REGISTER_CONTEXT);
    msg.extend_from_slice(registration_secret);
    msg.extend_from_slice(auth_pubkey);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcripts_are_domain_separated() {
        let a = auth_transcript(b"nonce", "x.onion");
        let r = register_transcript(b"nonce", b"x.onion");
        assert_ne!(a, r);
        assert!(a.starts_with(AUTH_CONTEXT));
        assert!(r.starts_with(REGISTER_CONTEXT));
    }
}
