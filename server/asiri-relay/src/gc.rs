//! Retention (architecture §9.7). Runs every few minutes and on `asiri-relay gc`.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::error::RelayResult;
use crate::ids::day;

/// What a GC pass removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Expired envelopes.
    pub envelopes: usize,
    /// Expired registration secrets.
    pub registrations: usize,
    /// Expired key packages.
    pub key_packages: usize,
    /// Expired or abandoned blobs.
    pub blobs: usize,
    /// Group heads idle for 180 days.
    pub group_heads: usize,
    /// Devices revoked more than 30 days ago.
    pub devices: usize,
}

/// Applies the retention rules at time `now`, deleting blob files from `blob_dir`.
pub fn run(conn: &mut Connection, blob_dir: &Path, now: i64) -> RelayResult<GcReport> {
    let mut report = GcReport::default();
    let tx = conn.transaction()?;
    report.envelopes = tx.execute("DELETE FROM envelopes WHERE expires_at <= ?1", params![now])?;
    report.registrations = tx.execute(
        "DELETE FROM registrations WHERE expires_at <= ?1",
        params![now],
    )?;
    // Pending devices whose registration expired never become usable.
    tx.execute(
        "DELETE FROM devices WHERE state = 0
           AND device_id NOT IN (SELECT device_id FROM registrations)",
        [],
    )?;
    report.key_packages = tx.execute(
        "DELETE FROM key_packages WHERE expires_at <= ?1",
        params![now],
    )?;
    report.group_heads = tx.execute(
        "DELETE FROM group_heads WHERE updated_day < ?1",
        params![day(now) - 180],
    )?;
    let revoked: Vec<Vec<u8>> = {
        let mut stmt =
            tx.prepare("SELECT inbox_id FROM devices WHERE state = 2 AND revoked_day < ?1")?;
        let rows = stmt.query_map(params![day(now) - 30], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for inbox in &revoked {
        tx.execute(
            "DELETE FROM inbox_counters WHERE inbox_id = ?1",
            params![inbox],
        )?;
    }
    report.devices = tx.execute(
        "DELETE FROM devices WHERE state = 2 AND revoked_day < ?1",
        params![day(now) - 30],
    )?;
    let expired_blobs: Vec<Vec<u8>> = {
        let mut stmt = tx.prepare("SELECT blob_id FROM blobs WHERE expires_at <= ?1")?;
        let rows = stmt.query_map(params![now], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    report.blobs = tx.execute("DELETE FROM blobs WHERE expires_at <= ?1", params![now])?;
    tx.commit()?;

    for id in expired_blobs {
        let name = hex::encode(&id);
        for suffix in ["", ".part"] {
            let path = blob_dir.join(format!("{name}{suffix}"));
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("could not delete an expired blob file: {e}");
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn removes_only_expired_rows() {
        let database = db::Db::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        database
            .call_blocking(|c| {
                let p = db::create_pending_device(c, [1; 32], None, false, 1_000)?;
                db::activate_device(c, &p.device_id, &[2; 32])?;
                for (id, expires_at) in [([1u8; 16], 500), ([2u8; 16], 5_000)] {
                    let envelope = db::OutgoingEnvelope {
                        envelope_id: &id,
                        kind: 1,
                        payload: b"x",
                        expires_at,
                    };
                    db::fan_out(c, &[p.inbox_id], &envelope, None, 10, 0)?;
                }
                db::create_blob(c, &[5; 16], &p.device_id, 10, 100, 500)?;
                std::fs::write(dir.path().join(hex::encode([5u8; 16])), b"x").unwrap();
                let report = run(c, dir.path(), 1_000)?;
                assert_eq!(report.envelopes, 1);
                assert_eq!(report.blobs, 1);
                assert_eq!(db::queued_count(c, &p.inbox_id)?, 1);
                assert!(!dir.path().join(hex::encode([5u8; 16])).exists());
                Ok(())
            })
            .unwrap();
    }
}
