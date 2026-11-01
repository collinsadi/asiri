//! Publishes the onion restricted-discovery keys that tor should accept (architecture §15.6).
//!
//! The relay never writes to tor's own directory. It writes the *desired* set of keys, one file
//! per device, into a directory it owns; `asiri-authsync` (a systemd path unit) validates them,
//! installs them into `/var/lib/tor/asiri/authorized_clients/` and reloads tor.

use std::path::{Path, PathBuf};

use crate::ids::Id;

/// Formats a key the way tor's `authorized_clients/*.auth` files expect it.
#[must_use]
pub fn auth_line(onion_client_key: &[u8; 32]) -> String {
    let b32 = data_encoding::BASE32_NOPAD.encode(onion_client_key);
    format!("descriptor:x25519:{b32}")
}

fn file_for(dir: &Path, device_id: &Id) -> PathBuf {
    dir.join(format!("{}.auth", hex::encode(device_id)))
}

/// Writes (atomically) the key file for one device.
pub fn write_key(dir: &Path, device_id: &Id, onion_client_key: &[u8; 32]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = file_for(dir, device_id);
    let tmp = path.with_extension("auth.tmp");
    std::fs::write(&tmp, format!("{}\n", auth_line(onion_client_key)))?;
    std::fs::rename(tmp, path)
}

/// Removes the key file for one device.
pub fn remove_key(dir: &Path, device_id: &Id) -> std::io::Result<()> {
    match std::fs::remove_file(file_for(dir, device_id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Makes the directory contain exactly `keys` (startup resync).
pub fn resync(dir: &Path, keys: &[(Id, [u8; 32])]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let wanted: std::collections::HashSet<String> = keys
        .iter()
        .map(|(id, _)| format!("{}.auth", hex::encode(id)))
        .collect();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".auth") && !wanted.contains(&name) {
            std::fs::remove_file(entry.path())?;
        }
    }
    for (id, key) in keys {
        write_key(dir, id, key)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_tor_compatible_files() {
        let dir = tempfile::tempdir().unwrap();
        let key = [7u8; 32];
        write_key(dir.path(), &[1; 16], &key).unwrap();
        write_key(dir.path(), &[2; 16], &key).unwrap();
        let text = std::fs::read_to_string(file_for(dir.path(), &[1; 16])).unwrap();
        assert!(text.starts_with("descriptor:x25519:"));
        assert_eq!(text.trim().len(), "descriptor:x25519:".len() + 52);
        resync(dir.path(), &[([2; 16], key)]).unwrap();
        assert!(!file_for(dir.path(), &[1; 16]).exists());
        assert!(file_for(dir.path(), &[2; 16]).exists());
        remove_key(dir.path(), &[2; 16]).unwrap();
        remove_key(dir.path(), &[2; 16]).unwrap();
    }
}
