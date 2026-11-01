//! Administrative operations shared by the CLI and tests (architecture §9.4, §9.10).

use asiri_proto::pairing::v1::{self as pairing, relay_endpoint::Transport};

use crate::authorized;
use crate::config::Config;
use crate::db::{self, Db};
use crate::ids::now;

/// Parses an onion restricted-discovery public key in any of the forms people will paste:
/// `descriptor:x25519:<BASE32>`, bare base32 (52 characters), or 64 hex characters.
pub fn parse_onion_client_key(text: &str) -> anyhow::Result<[u8; 32]> {
    let t = text.trim();
    let t = t.strip_prefix("descriptor:x25519:").unwrap_or(t);
    let bytes = if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        hex::decode(t)?
    } else {
        data_encoding::BASE32_NOPAD
            .decode(t.to_ascii_uppercase().as_bytes())
            .map_err(|_| anyhow::anyhow!("not a valid x25519 key"))?
    };
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("an x25519 key is 32 bytes"))
}

/// Creates the first admin device and returns the invitation it must scan (or paste) to
/// register (architecture §9.4 step 1).
///
/// `onion_client_key` is `None` when the relay is reached without Tor (development).
pub fn bootstrap(
    db: &Db,
    config: &Config,
    onion_client_key: Option<[u8; 32]>,
    transport: Transport,
    address: &str,
    call_address: Option<&str>,
) -> anyhow::Result<pairing::Invitation> {
    let onion = onion_client_key.unwrap_or([0u8; 32]);
    let expires = now() + i64::from(config.limits.registration_ttl_seconds);
    let pending = db.call_blocking(|c| db::create_pending_device(c, onion, None, true, expires))?;
    if let (Some(dir), Some(key)) = (&config.authorized_dir, onion_client_key) {
        authorized::write_key(dir, &pending.device_id, &key)?;
    }
    Ok(pairing::Invitation {
        relay: Some(pairing::RelayEndpoint {
            transport: transport as i32,
            address: address.to_owned(),
            call_address: call_address.unwrap_or_default().to_owned(),
        }),
        registration_secret: pending.registration_secret.to_vec(),
        device_id: pending.device_id.to_vec(),
        inbox_id: pending.inbox_id.to_vec(),
        inviter: None,
    })
}

/// Wraps an invitation in its pairing text form (`ASIRI1:…`).
#[must_use]
pub fn invitation_text(invitation: pairing::Invitation) -> String {
    asiri_proto::pairing_text::encode(&pairing::PairingPayload {
        version: 1,
        body: Some(pairing::pairing_payload::Body::Invitation(invitation)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_onion_keys_in_every_form() {
        let key = [42u8; 32];
        let line = authorized::auth_line(&key);
        assert_eq!(parse_onion_client_key(&line).unwrap(), key);
        assert_eq!(
            parse_onion_client_key(line.trim_start_matches("descriptor:x25519:")).unwrap(),
            key
        );
        assert_eq!(parse_onion_client_key(&hex::encode(key)).unwrap(), key);
        assert!(parse_onion_client_key("nope").is_err());
    }

    #[test]
    fn bootstrap_creates_a_pending_admin() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::for_dev(dir.path().to_path_buf());
        let db = Db::open_in_memory().unwrap();
        let inv = bootstrap(
            &db,
            &config,
            None,
            Transport::Direct,
            "127.0.0.1:8080",
            None,
        )
        .unwrap();
        let text = invitation_text(inv.clone());
        let back = asiri_proto::pairing_text::decode(&text).unwrap();
        assert!(matches!(
            back.body,
            Some(pairing::pairing_payload::Body::Invitation(_))
        ));
        let device = db
            .call_blocking(|c| db::pending_device_for_secret(c, &inv.registration_secret, now()))
            .unwrap()
            .unwrap();
        assert!(device.is_admin);
    }
}
