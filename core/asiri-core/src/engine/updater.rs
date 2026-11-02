//! Signed app updates delivered through the relay (architecture §16).
//!
//! The release manifest is JSON signed with the offline Ed25519 release key. The signature
//! covers the manifest's canonical form: the JSON object without its `signature` field, with
//! object keys sorted and no whitespace. The DMG is accepted only if its size and SHA-256 match
//! the signed manifest; nothing is installed behind the user's back.

use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer as _, SigningKey, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Core, CoreEvent};
use crate::error::{CoreError, CoreResult};
use crate::ids::now_ms;
use crate::net::http;
use crate::store;

const K_LAST_CHECK: &str = "updater.last_check_ms";
const CHECK_EVERY_MS: i64 = 86_400_000;
/// The relay accepts release files up to this size.
const MAX_DMG_BYTES: u64 = 600 * 1024 * 1024;

/// A release manifest (§16.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// Semantic version, e.g. `1.2.0`.
    pub version: String,
    /// Monotonic build number.
    pub build: u32,
    /// Builds older than this must update before they can be used.
    pub min_supported_build: u32,
    /// Hex SHA-256 of the DMG.
    pub dmg_sha256: String,
    /// DMG size in bytes.
    pub dmg_size: u64,
    /// File name on the relay (`Asiri-<version>.dmg` when absent).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dmg_name: String,
    /// Release notes.
    #[serde(default)]
    pub notes: String,
}

impl ReleaseManifest {
    /// The DMG's file name on the relay.
    #[must_use]
    pub fn file_name(&self) -> String {
        if self.dmg_name.is_empty() {
            format!("Asiri-{}.dmg", self.version)
        } else {
            self.dmg_name.clone()
        }
    }

    fn validate(&self) -> CoreResult<()> {
        let bad = |why: &str| {
            Err(CoreError::Crypto(format!(
                "invalid release manifest: {why}"
            )))
        };
        if self.version.is_empty()
            || self.version.len() > 32
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
        {
            return bad("version");
        }
        if self.dmg_sha256.len() != 64 || !self.dmg_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return bad("hash");
        }
        if self.dmg_size == 0 || self.dmg_size > MAX_DMG_BYTES {
            return bad("size");
        }
        let name = self.file_name();
        if name.len() > 128
            || name.starts_with('.')
            || !name.ends_with(".dmg")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return bad("file name");
        }
        if self.notes.len() > 8 * 1024 {
            return bad("notes");
        }
        Ok(())
    }
}

/// What the app shows about an available update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Version string.
    pub version: String,
    /// Build number.
    pub build: u32,
    /// Release notes.
    pub notes: String,
    /// DMG size in bytes.
    pub size: u64,
    /// Whether this build is too old to keep using (§16.2 "Forced updates").
    pub required: bool,
}

/// The canonical form the release signature covers: sorted keys, no whitespace.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Splits a manifest into its canonical signed bytes and its signature.
fn signed_parts(manifest_json: &[u8]) -> CoreResult<(Value, Vec<u8>, Option<String>)> {
    let mut value: Value = serde_json::from_slice(manifest_json)
        .map_err(|_| CoreError::Crypto("the release manifest is not valid JSON".into()))?;
    let Value::Object(map) = &mut value else {
        return Err(CoreError::Crypto(
            "the release manifest is not a JSON object".into(),
        ));
    };
    let signature = match map.remove("signature") {
        Some(Value::String(s)) => Some(s),
        None => None,
        Some(_) => return Err(CoreError::Crypto("malformed release signature".into())),
    };
    let canonical = canonical_json(&value).into_bytes();
    Ok((value, canonical, signature))
}

/// Signs a manifest with the release key (used by `asiri-release`); returns the manifest with
/// its `signature` field, pretty-printed.
pub fn sign_manifest(manifest_json: &[u8], key: &SigningKey) -> CoreResult<String> {
    let (mut value, canonical, _) = signed_parts(manifest_json)?;
    let manifest: ReleaseManifest = serde_json::from_value(value.clone())
        .map_err(|e| CoreError::Invalid(format!("release manifest: {e}")))?;
    manifest.validate()?;
    let signature = key.sign(&canonical);
    if let Value::Object(map) = &mut value {
        map.insert(
            "signature".into(),
            Value::String(STANDARD.encode(signature.to_bytes())),
        );
    }
    serde_json::to_string_pretty(&value).map_err(CoreError::internal)
}

/// Verifies a manifest against the release public key built into the app.
pub fn verify_manifest(manifest_json: &[u8], public_key: &[u8; 32]) -> CoreResult<ReleaseManifest> {
    let (value, canonical, signature) = signed_parts(manifest_json)?;
    let signature =
        signature.ok_or_else(|| CoreError::Crypto("the release manifest is not signed".into()))?;
    let bytes = STANDARD
        .decode(signature.trim())
        .map_err(|_| CoreError::Crypto("malformed release signature".into()))?;
    let signature = Signature::from_slice(&bytes)
        .map_err(|_| CoreError::Crypto("malformed release signature".into()))?;
    let key = VerifyingKey::from_bytes(public_key)
        .map_err(|_| CoreError::Crypto("invalid release key".into()))?;
    key.verify(&canonical, &signature)
        .map_err(|_| CoreError::Crypto("the release manifest's signature is not valid".into()))?;
    let manifest: ReleaseManifest = serde_json::from_value(value)
        .map_err(|e| CoreError::Crypto(format!("invalid release manifest: {e}")))?;
    manifest.validate()?;
    Ok(manifest)
}

impl Core {
    fn release_key(&self) -> CoreResult<[u8; 32]> {
        self.inner
            .config
            .release_public_key
            .ok_or_else(|| CoreError::Invalid("This build can't check for updates".into()))
    }

    /// Fetches and verifies the latest manifest; `None` when the relay has no release.
    async fn fetch_manifest(&self) -> CoreResult<Option<ReleaseManifest>> {
        let key = self.release_key()?;
        let client = self.client()?;
        let dialer = self.ensure_dialer().await?;
        let resp = http::request(
            &dialer,
            "GET",
            "/v1/releases/latest",
            Some(&client.session_token),
            &[],
            vec![],
        )
        .await?;
        match resp.status {
            200 => Ok(Some(verify_manifest(&resp.body, &key)?)),
            404 => Ok(None),
            _ => Err(http::error_from(&resp)),
        }
    }

    /// Checks for a signed update now (Settings → "Check for updates"). Emits
    /// [`CoreEvent::UpdateAvailable`] when there is one.
    pub async fn check_for_update(&self) -> CoreResult<Option<UpdateInfo>> {
        let manifest = self.fetch_manifest().await?;
        self.inner
            .store
            .tx(|c| store::kv_set(c, K_LAST_CHECK, &now_ms().to_le_bytes()))?;
        let build = self.inner.config.app_build;
        let Some(m) = manifest.filter(|m| m.build > build) else {
            return Ok(None);
        };
        let info = UpdateInfo {
            version: m.version.clone(),
            build: m.build,
            notes: m.notes.clone(),
            size: m.dmg_size,
            required: build < m.min_supported_build,
        };
        self.emit(CoreEvent::UpdateAvailable {
            version: info.version.clone(),
            notes: info.notes.clone(),
            required: info.required,
        });
        Ok(Some(info))
    }

    /// The daily check (§16.2), from the maintenance loop.
    pub(crate) async fn check_for_update_if_due(&self) -> CoreResult<()> {
        if self.inner.config.release_public_key.is_none() {
            return Ok(());
        }
        let last = {
            let conn = self.inner.store.lock()?;
            store::kv_get(&conn, K_LAST_CHECK)?
                .and_then(|v| <[u8; 8]>::try_from(v.as_slice()).ok())
                .map_or(0, i64::from_le_bytes)
        };
        if now_ms() - last < CHECK_EVERY_MS {
            return Ok(());
        }
        match self.check_for_update().await {
            Ok(_) => Ok(()),
            Err(e) if e.is_transient() => Err(e),
            Err(e) => {
                tracing::warn!("update check failed: {e}");
                self.inner
                    .store
                    .tx(|c| store::kv_set(c, K_LAST_CHECK, &now_ms().to_le_bytes()))?;
                Ok(())
            }
        }
    }

    /// Downloads the update and checks it against the signed manifest. Returns the path of the
    /// verified DMG for the app to open.
    pub async fn download_update(&self) -> CoreResult<PathBuf> {
        let m = self.fetch_manifest().await?.ok_or(CoreError::NotFound)?;
        if m.build <= self.inner.config.app_build {
            return Err(CoreError::Invalid("Asiri is up to date".into()));
        }
        let dir = self.data_dir().join("updates");
        tokio::fs::create_dir_all(&dir).await?;
        let name = m.file_name();
        let dest = dir.join(&name);
        let part = dir.join(format!("{name}.part"));
        let client = self.client()?;
        let dialer = self.ensure_dialer().await?;
        let got = http::download(
            &dialer,
            &format!("/v1/releases/{name}"),
            Some(&client.session_token),
            &part,
            m.dmg_size,
        )
        .await?;
        if let Some(resp) = &got.error {
            return Err(http::error_from(resp));
        }
        if got.written != m.dmg_size || !hex::encode(got.sha256).eq_ignore_ascii_case(&m.dmg_sha256)
        {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(CoreError::Crypto(
                "The downloaded update doesn't match its signature, so it was deleted.".into(),
            ));
        }
        tokio::fs::rename(&part, &dest).await?;
        Ok(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> String {
        serde_json::json!({
            "version": "1.2.0",
            "build": 42,
            "min_supported_build": 38,
            "dmg_sha256": "ab".repeat(32),
            "dmg_size": 48_213_504,
            "notes": "Group calls now show who is speaking.",
        })
        .to_string()
    }

    #[test]
    fn canonical_form_sorts_keys_and_strips_whitespace() {
        let v: Value =
            serde_json::from_str(r#"{ "b": [1, {"d": true, "c": null}], "a": "x\"y" }"#).unwrap();
        assert_eq!(
            canonical_json(&v),
            r#"{"a":"x\"y","b":[1,{"c":null,"d":true}]}"#
        );
    }

    #[test]
    fn signed_manifests_verify_and_tampering_is_caught() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let public = key.verifying_key().to_bytes();
        let signed = sign_manifest(manifest().as_bytes(), &key).unwrap();
        let m = verify_manifest(signed.as_bytes(), &public).unwrap();
        assert_eq!(m.build, 42);
        assert_eq!(m.file_name(), "Asiri-1.2.0.dmg");

        // Re-ordering and re-formatting keep the signature valid…
        let v: Value = serde_json::from_str(&signed).unwrap();
        assert!(verify_manifest(v.to_string().as_bytes(), &public).is_ok());
        // …but any change to the content breaks it.
        let tampered = signed.replace("\"build\": 42", "\"build\": 43");
        assert_ne!(tampered, signed);
        assert!(verify_manifest(tampered.as_bytes(), &public).is_err());
        // So does a different key, or no signature at all.
        let other = SigningKey::from_bytes(&[6; 32]).verifying_key().to_bytes();
        assert!(verify_manifest(signed.as_bytes(), &other).is_err());
        assert!(verify_manifest(manifest().as_bytes(), &public).is_err());
    }

    #[test]
    fn hostile_file_names_are_rejected() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let mut v: Value = serde_json::from_str(&manifest()).unwrap();
        v["dmg_name"] = Value::String("../../evil.dmg".into());
        assert!(sign_manifest(v.to_string().as_bytes(), &key).is_err());
    }
}
