//! Relay configuration.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Quotas and limits (architecture §9.8, §9.7).
#[derive(Clone, Debug)]
pub struct Limits {
    /// Largest accepted envelope payload.
    pub envelope_bytes: usize,
    /// Largest accepted file.
    pub blob_bytes: u64,
    /// Total stored file bytes per device.
    pub blob_quota_per_device: u64,
    /// Sends per minute per device.
    pub sends_per_minute: u32,
    /// Key-package fetches per hour per device.
    pub key_package_fetches_per_hour: u32,
    /// Device authorizations per day per device.
    pub authorizations_per_day: u32,
    /// Maximum queued envelopes per inbox.
    pub inbox_backlog: u64,
    /// Longest an envelope may wait in an inbox.
    pub envelope_ttl_seconds: u32,
    /// How long uploaded files are kept.
    pub blob_ttl_seconds: u32,
    /// Lifetime of a key package uploaded to the directory.
    pub key_package_ttl_seconds: u32,
    /// Lifetime of a one-time registration secret.
    pub registration_ttl_seconds: u32,
    /// How many unacknowledged envelopes may be in flight per session.
    pub delivery_window: usize,
    /// Largest single blob upload chunk.
    pub blob_chunk_bytes: usize,
    /// Lifetime of minted LiveKit tokens.
    pub call_token_ttl_seconds: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            envelope_bytes: 256 * 1024,
            blob_bytes: 200 * 1024 * 1024,
            blob_quota_per_device: 2 * 1024 * 1024 * 1024,
            sends_per_minute: 600,
            key_package_fetches_per_hour: 60,
            authorizations_per_day: 10,
            inbox_backlog: 50_000,
            envelope_ttl_seconds: 30 * 86_400,
            blob_ttl_seconds: 30 * 86_400,
            key_package_ttl_seconds: 30 * 86_400,
            registration_ttl_seconds: 86_400,
            delivery_window: 64,
            blob_chunk_bytes: 1024 * 1024 + 4096,
            call_token_ttl_seconds: 600,
        }
    }
}

/// LiveKit API credentials, used only to mint access tokens (architecture §10.4).
#[derive(Clone)]
pub struct LiveKitKeys {
    /// The API key (JWT issuer).
    pub api_key: String,
    /// The shared secret (HMAC key).
    pub api_secret: String,
}

impl std::fmt::Debug for LiveKitKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveKitKeys")
            .field("api_key", &self.api_key)
            .field("api_secret", &"<redacted>")
            .finish()
    }
}

impl LiveKitKeys {
    /// Reads keys from a file in LiveKit's `keys:` format: one `KEY: secret` pair per line
    /// (`KEY=secret` is accepted too). The first pair wins; blank lines and `#` comments are
    /// ignored.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
            .ok_or_else(|| anyhow::anyhow!("no LiveKit key pair in {}", path.display()))
    }

    /// Parses the file format described in [`LiveKitKeys::from_file`].
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#') && *l != "keys:")
            .find_map(|line| {
                let (k, v) = line.split_once(':').or_else(|| line.split_once('='))?;
                let key = k.trim().trim_matches('"');
                let secret = v.trim().trim_matches('"');
                (!key.is_empty() && secret.len() >= 32).then(|| Self {
                    api_key: key.to_owned(),
                    api_secret: secret.to_owned(),
                })
            })
    }
}

/// Everything the relay needs to run.
#[derive(Clone, Debug)]
pub struct Config {
    /// Where to listen. Behind tor this is `127.0.0.1:8080` (architecture §15.2).
    pub listen: SocketAddr,
    /// Database, encrypted files and release artifacts live here.
    pub data_dir: PathBuf,
    /// Where to publish the desired onion restricted-discovery keys for `asiri-authsync`.
    pub authorized_dir: Option<PathBuf>,
    /// The address devices dial, bound into every auth signature (e.g. `<56 chars>.onion`).
    pub public_address: String,
    /// LiveKit credentials; call tokens are refused when absent.
    pub livekit: Option<LiveKitKeys>,
    /// Quotas and limits.
    pub limits: Limits,
}

impl Config {
    /// A configuration suitable for tests and local development.
    #[must_use]
    pub fn for_dev(data_dir: PathBuf) -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 0)),
            data_dir,
            authorized_dir: None,
            public_address: "127.0.0.1".into(),
            livekit: None,
            limits: Limits::default(),
        }
    }

    /// Path of the SQLite database.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("relay.db")
    }

    /// Directory holding encrypted files.
    #[must_use]
    pub fn blob_dir(&self) -> PathBuf {
        self.data_dir.join("blobs")
    }

    /// Directory holding signed release manifests and installers.
    #[must_use]
    pub fn releases_dir(&self) -> PathBuf {
        self.data_dir.join("releases")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_livekit_key_files() {
        let secret = "s".repeat(40);
        let k = LiveKitKeys::parse(&format!("# comment\nkeys:\n  APIabc: \"{secret}\"\n")).unwrap();
        assert_eq!(k.api_key, "APIabc");
        assert_eq!(k.api_secret, secret);
        assert!(
            LiveKitKeys::parse("APIabc: short").is_none(),
            "secrets must be long"
        );
        assert!(format!("{k:?}").contains("redacted"));
    }
}
