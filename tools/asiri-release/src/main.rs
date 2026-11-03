//! `asiri-release`: the offline release key and signed update manifests (architecture §16).
//!
//! ```text
//! asiri-release keygen --out release.key           create the release key (keep it offline)
//! asiri-release public-key --key release.key       the key to build into the app
//! asiri-release sign --key release.key --dmg Asiri-1.2.0.dmg --version 1.2.0 --build 42 \
//!     [--min-build 38] [--notes "…"] [--out latest.json]
//! asiri-release verify --public-key <base64> latest.json [--dmg Asiri-1.2.0.dmg]
//! ```
//!
//! The app only installs a disk image whose size and SHA-256 match a manifest signed by this
//! key, so the key never goes near the server: sign on your Mac, then publish the manifest and
//! the DMG with `scripts/deploy-vps.sh … --publish` (docs/releasing.md).

#![allow(clippy::print_stdout)]

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use asiri_core::engine::updater::{sign_manifest, verify_manifest};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use sha2::{Digest as _, Sha256};

#[derive(Parser)]
#[command(
    name = "asiri-release",
    version,
    about = "Asiri release key and signed update manifests"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the release key. Keep the file offline; anyone with it can ship "updates".
    Keygen {
        /// Where to write the key (created with owner-only permissions; never overwritten).
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the public key to build into the app (ASIRI_RELEASE_PUBLIC_KEY).
    PublicKey {
        #[arg(long)]
        key: PathBuf,
    },
    /// Write a signed manifest for a disk image.
    Sign {
        #[arg(long)]
        key: PathBuf,
        /// The disk image people will download.
        #[arg(long)]
        dmg: PathBuf,
        /// Semantic version, e.g. 1.2.0.
        #[arg(long)]
        version: String,
        /// Build number (CFBundleVersion); it only ever goes up.
        #[arg(long)]
        build: u32,
        /// Builds older than this must update before they can be used.
        #[arg(long)]
        min_build: Option<u32>,
        /// Release notes shown in the app.
        #[arg(long, default_value = "")]
        notes: String,
        /// Where to write the manifest.
        #[arg(long, default_value = "latest.json")]
        out: PathBuf,
    },
    /// Check a manifest's signature (and, with --dmg, that the image matches it).
    Verify {
        /// The public key, base64.
        #[arg(long)]
        public_key: String,
        manifest: PathBuf,
        #[arg(long)]
        dmg: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Keygen { out } => keygen(&out),
        Command::PublicKey { key } => {
            println!("{}", public_key(&read_key(&key)?));
            Ok(())
        }
        Command::Sign {
            key,
            dmg,
            version,
            build,
            min_build,
            notes,
            out,
        } => {
            let manifest = sign(
                &read_key(&key)?,
                &dmg,
                &version,
                build,
                min_build.unwrap_or(build),
                &notes,
            )?;
            std::fs::write(&out, &manifest)
                .with_context(|| format!("writing {}", out.display()))?;
            println!("signed {} → {}", dmg.display(), out.display());
            Ok(())
        }
        Command::Verify {
            public_key,
            manifest,
            dmg,
        } => {
            let summary = verify(&public_key, &manifest, dmg.as_deref())?;
            println!("{summary}");
            Ok(())
        }
    }
}

fn keygen(out: &Path) -> anyhow::Result<()> {
    ensure!(
        !out.exists(),
        "{} already exists; refusing to overwrite a release key",
        out.display()
    );
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    write_private(out, &format!("{}\n", STANDARD.encode(key.to_bytes())))?;
    println!(
        "Release key written to {} — keep it offline and backed up.",
        out.display()
    );
    println!("Public key (build it into the app as ASIRI_RELEASE_PUBLIC_KEY):");
    println!("{}", public_key(&key));
    Ok(())
}

fn write_private(path: &Path, text: &str) -> anyhow::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    Ok(())
}

fn read_key(path: &Path) -> anyhow::Result<SigningKey> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let bytes = STANDARD
        .decode(text.trim())
        .context("the release key file isn't base64")?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("the release key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&seed))
}

fn public_key(key: &SigningKey) -> String {
    STANDARD.encode(key.verifying_key().to_bytes())
}

/// SHA-256 (hex) and size of a file, streamed.
fn digest(path: &Path) -> anyhow::Result<(String, u64)> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

fn sign(
    key: &SigningKey,
    dmg: &Path,
    version: &str,
    build: u32,
    min_build: u32,
    notes: &str,
) -> anyhow::Result<String> {
    ensure!(
        min_build <= build,
        "--min-build can't be newer than --build"
    );
    let (sha256, size) = digest(dmg)?;
    let name = dmg
        .file_name()
        .and_then(|n| n.to_str())
        .context("the disk image needs a file name")?;
    let manifest = serde_json::json!({
        "version": version,
        "build": build,
        "min_supported_build": min_build,
        "dmg_sha256": sha256,
        "dmg_size": size,
        "dmg_name": name,
        "notes": notes,
    });
    Ok(sign_manifest(manifest.to_string().as_bytes(), key)?)
}

fn verify(public_key_b64: &str, manifest: &Path, dmg: Option<&Path>) -> anyhow::Result<String> {
    let key: [u8; 32] = STANDARD
        .decode(public_key_b64.trim())
        .context("the public key isn't base64")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("the public key must be 32 bytes"))?;
    let json =
        std::fs::read(manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let m = verify_manifest(&json, &key)?;
    if let Some(dmg) = dmg {
        let (sha256, size) = digest(dmg)?;
        if size != m.dmg_size || sha256 != m.dmg_sha256 {
            bail!("{} doesn't match the manifest", dmg.display());
        }
    }
    Ok(format!(
        "valid: Asiri {} (build {}, required from build {}), {} bytes{}",
        m.version,
        m.build,
        m.min_supported_build,
        m.dmg_size,
        if dmg.is_some() {
            ", and the disk image matches"
        } else {
            ""
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sign_and_verify_round_trip() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let key_path = dir.path().join("release.key");
        keygen(&key_path)?;
        assert!(keygen(&key_path).is_err(), "never overwrites a key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&key_path)?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let key = read_key(&key_path)?;
        let dmg = dir.path().join("Asiri-1.2.0.dmg");
        std::fs::write(&dmg, b"not really a disk image")?;
        let manifest = sign(
            &key,
            &dmg,
            "1.2.0",
            42,
            38,
            "Group calls show who is speaking.",
        )?;
        let path = dir.path().join("latest.json");
        std::fs::write(&path, &manifest)?;

        let ok = verify(&public_key(&key), &path, Some(&dmg))?;
        assert!(ok.contains("Asiri 1.2.0 (build 42, required from build 38)"));

        // Another key, a changed manifest or a different image are all refused.
        let other = SigningKey::from_bytes(&[9; 32]);
        assert!(verify(&public_key(&other), &path, None).is_err());
        std::fs::write(&path, manifest.replace("1.2.0", "1.2.1"))?;
        assert!(verify(&public_key(&key), &path, None).is_err());
        std::fs::write(&path, &manifest)?;
        std::fs::write(&dmg, b"a tampered disk image")?;
        assert!(verify(&public_key(&key), &path, Some(&dmg)).is_err());
        Ok(())
    }

    #[test]
    fn minimum_build_cannot_exceed_the_build() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let dmg = dir.path().join("Asiri-1.0.0.dmg");
        std::fs::write(&dmg, b"x")?;
        assert!(sign(&SigningKey::from_bytes(&[1; 32]), &dmg, "1.0.0", 5, 6, "").is_err());
        Ok(())
    }
}
