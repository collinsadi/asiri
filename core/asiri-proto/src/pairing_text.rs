//! Text form of pairing payloads: `ASIRI1:` followed by unpadded base64url of a
//! [`PairingPayload`](crate::pairing::v1::PairingPayload).
//!
//! The same string is rendered as a QR code and can be pasted by hand, which is how an admin
//! hands the bootstrap invitation from a terminal to the Mac sitting in front of it.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use prost::Message as _;

use crate::pairing::v1::PairingPayload;

/// Prefix of every pairing string; the digit is the payload format version.
pub const PREFIX: &str = "ASIRI1:";

/// Largest payload accepted when decoding (QR codes stay far below this).
const MAX_ENCODED_LEN: usize = 4096;

/// Errors decoding a pairing string.
#[derive(Debug, PartialEq, Eq)]
pub enum PairingTextError {
    /// The string does not start with [`PREFIX`].
    MissingPrefix,
    /// The string is longer than any valid payload.
    TooLong,
    /// The body is not valid base64url.
    InvalidEncoding,
    /// The bytes are not a valid payload.
    InvalidPayload,
    /// The payload version is not supported by this build.
    UnsupportedVersion(u32),
}

impl std::fmt::Display for PairingTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPrefix => write!(f, "not an Asiri code"),
            Self::TooLong => write!(f, "code is too long"),
            Self::InvalidEncoding | Self::InvalidPayload => write!(f, "code is damaged"),
            Self::UnsupportedVersion(v) => write!(f, "code needs a newer Asiri (version {v})"),
        }
    }
}

impl std::error::Error for PairingTextError {}

/// Encodes a payload as `ASIRI1:<base64url>`.
#[must_use]
pub fn encode(payload: &PairingPayload) -> String {
    let mut out = String::from(PREFIX);
    out.push_str(&URL_SAFE_NO_PAD.encode(payload.encode_to_vec()));
    out
}

/// Decodes a string produced by [`encode`]. Surrounding whitespace is ignored.
pub fn decode(text: &str) -> Result<PairingPayload, PairingTextError> {
    let text = text.trim();
    if text.len() > MAX_ENCODED_LEN {
        return Err(PairingTextError::TooLong);
    }
    let body = text
        .strip_prefix(PREFIX)
        .ok_or(PairingTextError::MissingPrefix)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| PairingTextError::InvalidEncoding)?;
    let payload =
        PairingPayload::decode(bytes.as_slice()).map_err(|_| PairingTextError::InvalidPayload)?;
    if payload.version != 1 {
        return Err(PairingTextError::UnsupportedVersion(payload.version));
    }
    if payload.body.is_none() {
        return Err(PairingTextError::InvalidPayload);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::v1::{ContactCard, pairing_payload::Body};

    fn sample() -> PairingPayload {
        PairingPayload {
            version: 1,
            body: Some(Body::Contact(ContactCard {
                user_id: vec![1; 16],
                device_id: vec![2; 16],
                inbox_id: vec![3; 16],
                signature_key: vec![4; 32],
                display_name: "Ada".into(),
            })),
        }
    }

    #[test]
    fn round_trips() {
        let text = encode(&sample());
        assert!(text.starts_with(PREFIX));
        assert_eq!(decode(&format!("  {text}\n")).unwrap(), sample());
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(decode("hello"), Err(PairingTextError::MissingPrefix));
        assert_eq!(decode("ASIRI1:***"), Err(PairingTextError::InvalidEncoding));
        let mut p = sample();
        p.version = 9;
        assert_eq!(
            decode(&encode(&p)),
            Err(PairingTextError::UnsupportedVersion(9))
        );
        let empty = PairingPayload {
            version: 1,
            body: None,
        };
        assert_eq!(
            decode(&encode(&empty)),
            Err(PairingTextError::InvalidPayload)
        );
    }
}
