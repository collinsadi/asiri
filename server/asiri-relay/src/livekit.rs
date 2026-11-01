//! LiveKit access tokens (architecture §10.4).
//!
//! A LiveKit token is an HS256 JWT signed with the API secret. Minting one needs nothing more
//! than HMAC-SHA256, so the relay implements the format directly instead of pulling in the
//! LiveKit server SDK and its dependency tree. The identity is always a random alias — never a
//! device ID or a person's name.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;

use crate::config::LiveKitKeys;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VideoGrant<'a> {
    room: &'a str,
    room_join: bool,
    can_publish: bool,
    can_subscribe: bool,
    can_publish_data: bool,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    sub: &'a str,
    jti: &'a str,
    nbf: i64,
    exp: i64,
    video: VideoGrant<'a>,
}

/// Room aliases are random strings chosen by clients: 1–64 of `[A-Za-z0-9_-]`.
#[must_use]
pub fn valid_room_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Mints a token that lets `identity` join `room` for `ttl_seconds`, starting at `now`.
pub fn mint(
    keys: &LiveKitKeys,
    identity: &str,
    room: &str,
    now: i64,
    ttl_seconds: u32,
) -> Result<String, serde_json::Error> {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = Claims {
        iss: &keys.api_key,
        sub: identity,
        jti: identity,
        nbf: now - 5,
        exp: now + i64::from(ttl_seconds),
        video: VideoGrant {
            room,
            room_join: true,
            can_publish: true,
            can_subscribe: true,
            can_publish_data: true,
        },
    };
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims)?);
    let signing_input = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(keys.api_secret.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(signing_input.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{signing_input}.{sig}"))
}

/// Verifies a token's signature and returns its decoded claims (used by tests and diagnostics).
#[must_use]
pub fn verify(keys: &LiveKitKeys, token: &str) -> Option<serde_json::Value> {
    let (signing_input, sig) = token.rsplit_once('.')?;
    let mut mac = Hmac::<Sha256>::new_from_slice(keys.api_secret.as_bytes()).ok()?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(sig).ok()?).ok()?;
    let (_, payload) = signing_input.split_once('.')?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> LiveKitKeys {
        LiveKitKeys {
            api_key: "APItest".into(),
            api_secret: "x".repeat(40),
        }
    }

    #[test]
    fn mints_verifiable_tokens() {
        let t = mint(&keys(), "alias-1", "room_A", 1_000, 600).unwrap();
        let claims = verify(&keys(), &t).unwrap();
        assert_eq!(claims["iss"], "APItest");
        assert_eq!(claims["sub"], "alias-1");
        assert_eq!(claims["exp"], 1_600);
        assert_eq!(claims["video"]["room"], "room_A");
        assert_eq!(claims["video"]["roomJoin"], true);
        let tampered = format!("{}A", &t[..t.len() - 1]);
        assert!(verify(&keys(), &tampered).is_none());
        let other = LiveKitKeys {
            api_key: "APItest".into(),
            api_secret: "y".repeat(40),
        };
        assert!(verify(&other, &t).is_none());
    }

    #[test]
    fn validates_room_aliases() {
        assert!(valid_room_alias("abc-DEF_123"));
        assert!(!valid_room_alias(""));
        assert!(!valid_room_alias("has space"));
        assert!(!valid_room_alias(&"a".repeat(65)));
    }
}
