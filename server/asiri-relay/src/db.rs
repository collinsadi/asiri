//! The relay database (architecture §9.6): SQLite in WAL mode with `secure_delete` on.
//! No names, no IP addresses, no activity timestamps; only random IDs, public keys, ciphertext
//! and expiry times.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::{RelayError, RelayResult};
use crate::ids::{Id, day, parse_id, random_bytes, random_id};

/// Schema version stored in `meta`.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS devices (
  device_id        BLOB PRIMARY KEY,
  inbox_id         BLOB NOT NULL UNIQUE,
  auth_pubkey      BLOB,
  onion_client_key BLOB NOT NULL,
  is_admin         INTEGER NOT NULL DEFAULT 0,
  state            INTEGER NOT NULL,
  revoked_day      INTEGER
);
CREATE TABLE IF NOT EXISTS registrations (
  device_id   BLOB PRIMARY KEY REFERENCES devices(device_id) ON DELETE CASCADE,
  secret_hash BLOB NOT NULL UNIQUE,
  expires_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS envelopes (
  inbox_id    BLOB NOT NULL,
  seq         INTEGER NOT NULL,
  envelope_id BLOB NOT NULL,
  kind        INTEGER NOT NULL,
  payload     BLOB NOT NULL,
  expires_at  INTEGER NOT NULL,
  PRIMARY KEY (inbox_id, seq),
  UNIQUE (inbox_id, envelope_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS envelopes_expiry ON envelopes(expires_at);
CREATE TABLE IF NOT EXISTS inbox_counters (
  inbox_id BLOB PRIMARY KEY,
  next_seq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS key_packages (
  id          INTEGER PRIMARY KEY,
  device_id   BLOB NOT NULL REFERENCES devices(device_id) ON DELETE CASCADE,
  key_package BLOB NOT NULL,
  last_resort INTEGER NOT NULL DEFAULT 0,
  expires_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS key_packages_device ON key_packages(device_id, last_resort, id);
CREATE TABLE IF NOT EXISTS group_heads (
  group_route      BLOB PRIMARY KEY,
  epoch            INTEGER NOT NULL,
  last_envelope_id BLOB NOT NULL,
  updated_day      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blobs (
  blob_id      BLOB PRIMARY KEY,
  owner_device BLOB NOT NULL,
  size         INTEGER NOT NULL,
  received     INTEGER NOT NULL DEFAULT 0,
  complete     INTEGER NOT NULL DEFAULT 0,
  expires_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS blobs_owner ON blobs(owner_device);
";

/// Device lifecycle states (architecture §9.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    /// Authorized but not yet registered.
    Pending = 0,
    /// Registered and allowed to connect.
    Active = 1,
    /// Revoked; kept for 30 days, then deleted.
    Revoked = 2,
}

impl DeviceState {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Active,
            2 => Self::Revoked,
            _ => Self::Pending,
        }
    }
}

/// A registered or pending device.
#[derive(Clone, Debug)]
pub struct Device {
    /// Random device ID.
    pub device_id: Id,
    /// Random inbox ID.
    pub inbox_id: Id,
    /// Ed25519 relay auth key; `None` until a bootstrap device registers.
    pub auth_pubkey: Option<[u8; 32]>,
    /// X25519 onion restricted-discovery key (all zeros when running without Tor).
    pub onion_client_key: [u8; 32],
    /// Whether the device may administer the relay.
    pub is_admin: bool,
    /// Lifecycle state.
    pub state: DeviceState,
}

fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    let device_id: Vec<u8> = row.get(0)?;
    let inbox_id: Vec<u8> = row.get(1)?;
    let auth: Option<Vec<u8>> = row.get(2)?;
    let onion: Vec<u8> = row.get(3)?;
    let bad = || rusqlite::Error::InvalidQuery;
    Ok(Device {
        device_id: parse_id(&device_id).ok_or_else(bad)?,
        inbox_id: parse_id(&inbox_id).ok_or_else(bad)?,
        auth_pubkey: match auth {
            Some(a) => Some(a.as_slice().try_into().map_err(|_| bad())?),
            None => None,
        },
        onion_client_key: onion.as_slice().try_into().map_err(|_| bad())?,
        is_admin: row.get::<_, i64>(4)? != 0,
        state: DeviceState::from_i64(row.get(5)?),
    })
}

const DEVICE_COLUMNS: &str = "device_id, inbox_id, auth_pubkey, onion_client_key, is_admin, state";

/// One queued envelope.
#[derive(Clone, Debug)]
pub struct StoredEnvelope {
    /// Per-inbox sequence number.
    pub seq: u64,
    /// Sender-chosen idempotency key.
    pub envelope_id: Vec<u8>,
    /// `PayloadKind` as an integer.
    pub kind: i32,
    /// Padded ciphertext.
    pub payload: Vec<u8>,
}

/// An envelope to fan out.
pub struct OutgoingEnvelope<'a> {
    /// Sender-chosen idempotency key.
    pub envelope_id: &'a [u8],
    /// `PayloadKind` as an integer.
    pub kind: i32,
    /// Padded ciphertext.
    pub payload: &'a [u8],
    /// Absolute expiry (Unix seconds).
    pub expires_at: i64,
}

/// Outcome of trying to append a commit for `(group_route, epoch)`.
#[derive(Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The commit won its epoch and was queued for every recipient.
    Accepted,
    /// Another commit already won; the group is at `current_epoch`.
    StaleEpoch {
        /// The epoch the group is at now.
        current_epoch: u64,
    },
}

/// Outcome of a fan-out.
#[derive(Debug, PartialEq, Eq)]
pub enum FanOut {
    /// Queued for every recipient.
    Queued(CommitOutcome),
    /// Some recipients are unknown or revoked; nothing was queued.
    UnknownInboxes(Vec<Id>),
    /// A recipient's inbox is full; nothing was queued.
    InboxFull,
}

/// Thread-safe handle to the relay database.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens (or creates) the database at `path` and applies the schema.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// An in-memory database for tests.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "secure_delete", "ON")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES ('invite_policy', 'admins')",
            [],
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` on a blocking thread with exclusive access to the connection.
    pub async fn call<F, T>(&self, f: F) -> RelayResult<T>
    where
        F: FnOnce(&mut Connection) -> RelayResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| RelayError::Internal(anyhow::anyhow!("database lock poisoned")))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| RelayError::Internal(e.into()))?
    }

    /// Runs `f` on the current thread (CLI commands and tests).
    pub fn call_blocking<F, T>(&self, f: F) -> RelayResult<T>
    where
        F: FnOnce(&mut Connection) -> RelayResult<T>,
    {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| RelayError::Internal(anyhow::anyhow!("database lock poisoned")))?;
        f(&mut guard)
    }
}

// ─── meta ───────────────────────────────────────────────────────────────────

/// Who may authorize new devices (architecture §9.4, open question §22.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvitePolicy {
    /// Only admins (default).
    Admins,
    /// Any registered member.
    Members,
}

impl InvitePolicy {
    /// Wire/CLI name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admins => "admins",
            Self::Members => "members",
        }
    }

    /// Parses a wire/CLI name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "admins" => Some(Self::Admins),
            "members" => Some(Self::Members),
            _ => None,
        }
    }
}

/// Reads the invite policy.
pub fn invite_policy(conn: &Connection) -> RelayResult<InvitePolicy> {
    let v: String = conn.query_row(
        "SELECT value FROM meta WHERE key = 'invite_policy'",
        [],
        |r| r.get(0),
    )?;
    Ok(InvitePolicy::parse(&v).unwrap_or(InvitePolicy::Admins))
}

/// Sets the invite policy.
pub fn set_invite_policy(conn: &Connection, policy: InvitePolicy) -> RelayResult<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('invite_policy', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![policy.as_str()],
    )?;
    Ok(())
}

// ─── devices and registrations ──────────────────────────────────────────────

/// A freshly authorized device and its one-time registration secret.
pub struct PendingDevice {
    /// The new device ID.
    pub device_id: Id,
    /// The new inbox ID.
    pub inbox_id: Id,
    /// The one-time secret the device presents to `POST /v1/register`.
    pub registration_secret: [u8; 32],
}

/// Creates a pending device and its registration secret (architecture §9.4 steps 1–2).
pub fn create_pending_device(
    conn: &mut Connection,
    onion_client_key: [u8; 32],
    auth_pubkey: Option<[u8; 32]>,
    is_admin: bool,
    secret_expires_at: i64,
) -> RelayResult<PendingDevice> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = random_id();
    let inbox_id = random_id();
    let secret: [u8; 32] = random_bytes();
    tx.execute(
        "INSERT INTO devices(device_id, inbox_id, auth_pubkey, onion_client_key, is_admin, state)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        params![
            &device_id[..],
            &inbox_id[..],
            auth_pubkey.as_ref().map(|k| &k[..]),
            &onion_client_key[..],
            i64::from(is_admin)
        ],
    )?;
    tx.execute(
        "INSERT INTO registrations(device_id, secret_hash, expires_at) VALUES (?1, ?2, ?3)",
        params![&device_id[..], &sha256(&secret)[..], secret_expires_at],
    )?;
    tx.commit()?;
    Ok(PendingDevice {
        device_id,
        inbox_id,
        registration_secret: secret,
    })
}

/// SHA-256 helper.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

/// Looks up the pending device for a registration secret, if it has not expired.
pub fn pending_device_for_secret(
    conn: &Connection,
    secret: &[u8],
    now: i64,
) -> RelayResult<Option<Device>> {
    let hash = sha256(secret);
    let device = conn
        .query_row(
            &format!(
                "SELECT {DEVICE_COLUMNS} FROM devices
                 JOIN registrations USING (device_id)
                 WHERE registrations.secret_hash = ?1 AND registrations.expires_at > ?2
                   AND devices.state = 0"
            ),
            params![&hash[..], now],
            device_from_row,
        )
        .optional()?;
    Ok(device)
}

/// Activates a pending device, consuming its registration (architecture §9.4 step 3).
pub fn activate_device(
    conn: &mut Connection,
    device_id: &Id,
    auth_pubkey: &[u8; 32],
) -> RelayResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE devices SET state = 1, auth_pubkey = ?2 WHERE device_id = ?1 AND state = 0",
        params![&device_id[..], &auth_pubkey[..]],
    )?;
    if changed != 1 {
        return Err(RelayError::Conflict("device is not pending"));
    }
    tx.execute(
        "DELETE FROM registrations WHERE device_id = ?1",
        params![&device_id[..]],
    )?;
    let inbox: Vec<u8> = tx.query_row(
        "SELECT inbox_id FROM devices WHERE device_id = ?1",
        params![&device_id[..]],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO inbox_counters(inbox_id, next_seq) VALUES (?1, 1)",
        params![inbox],
    )?;
    tx.commit()?;
    Ok(())
}

/// Fetches a device by ID.
pub fn device(conn: &Connection, device_id: &Id) -> RelayResult<Option<Device>> {
    Ok(conn
        .query_row(
            &format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE device_id = ?1"),
            params![&device_id[..]],
            device_from_row,
        )
        .optional()?)
}

/// Lists every device (admins only, and the CLI).
pub fn list_devices(conn: &Connection) -> RelayResult<Vec<Device>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices ORDER BY state, is_admin DESC"
    ))?;
    let rows = stmt.query_map([], device_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every device that should be able to reach the onion service (pending and active).
pub fn reachable_onion_keys(conn: &Connection) -> RelayResult<Vec<(Id, [u8; 32])>> {
    Ok(list_devices(conn)?
        .into_iter()
        .filter(|d| d.state != DeviceState::Revoked && d.onion_client_key != [0u8; 32])
        .map(|d| (d.device_id, d.onion_client_key))
        .collect())
}

/// Revokes a device: it can no longer connect, and its mailbox, key packages and pending
/// registration are deleted immediately (architecture §11.7).
pub fn revoke_device(conn: &mut Connection, device_id: &Id, now: i64) -> RelayResult<Device> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let dev = tx
        .query_row(
            &format!("SELECT {DEVICE_COLUMNS} FROM devices WHERE device_id = ?1"),
            params![&device_id[..]],
            device_from_row,
        )
        .optional()?
        .ok_or(RelayError::NotFound)?;
    tx.execute(
        "UPDATE devices SET state = 2, revoked_day = ?2 WHERE device_id = ?1",
        params![&device_id[..], day(now)],
    )?;
    tx.execute(
        "DELETE FROM envelopes WHERE inbox_id = ?1",
        params![&dev.inbox_id[..]],
    )?;
    tx.execute(
        "DELETE FROM key_packages WHERE device_id = ?1",
        params![&device_id[..]],
    )?;
    tx.execute(
        "DELETE FROM registrations WHERE device_id = ?1",
        params![&device_id[..]],
    )?;
    tx.commit()?;
    Ok(dev)
}

// ─── envelopes ──────────────────────────────────────────────────────────────

/// Queues one payload for every recipient inbox in a single transaction.
///
/// For commits (`commit = Some((group_route, epoch))`) this is also where the relay enforces
/// "exactly one commit per epoch per group" (architecture §9.5): the head check, the head update
/// and every recipient's copy happen inside one `BEGIN IMMEDIATE` transaction, so two concurrent
/// commits can never both win and every member sees commits in the same order.
pub fn fan_out(
    conn: &mut Connection,
    recipients: &[Id],
    envelope: &OutgoingEnvelope<'_>,
    commit: Option<(&Id, u64)>,
    backlog_limit: u64,
    now: i64,
) -> RelayResult<FanOut> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    // Every recipient must be an active device.
    let mut unknown = Vec::new();
    {
        let mut stmt =
            tx.prepare_cached("SELECT 1 FROM devices WHERE inbox_id = ?1 AND state = 1")?;
        for inbox in recipients {
            if !stmt.exists(params![&inbox[..]])? {
                unknown.push(*inbox);
            }
        }
    }
    if !unknown.is_empty() {
        return Ok(FanOut::UnknownInboxes(unknown));
    }

    // Backlog limit (architecture §9.8).
    {
        let mut stmt = tx.prepare_cached("SELECT COUNT(*) FROM envelopes WHERE inbox_id = ?1")?;
        for inbox in recipients {
            let n: i64 = stmt.query_row(params![&inbox[..]], |r| r.get(0))?;
            if u64::try_from(n).unwrap_or(u64::MAX) >= backlog_limit {
                return Ok(FanOut::InboxFull);
            }
        }
    }

    if let Some((route, epoch)) = commit {
        let head: Option<(i64, Vec<u8>)> = tx
            .query_row(
                "SELECT epoch, last_envelope_id FROM group_heads WHERE group_route = ?1",
                params![&route[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let epoch_i = i64::try_from(epoch).map_err(|_| RelayError::BadRequest("epoch"))?;
        match head {
            None if epoch == 0 => {}
            Some((h, _)) if h == epoch_i => {}
            // Idempotent retry of the commit that already won this epoch.
            Some((h, ref last)) if h == epoch_i + 1 && last.as_slice() == envelope.envelope_id => {
                return Ok(FanOut::Queued(CommitOutcome::Accepted));
            }
            None => {
                return Ok(FanOut::Queued(CommitOutcome::StaleEpoch {
                    current_epoch: 0,
                }));
            }
            Some((h, _)) => {
                return Ok(FanOut::Queued(CommitOutcome::StaleEpoch {
                    current_epoch: u64::try_from(h).unwrap_or(0),
                }));
            }
        }
        tx.execute(
            "INSERT INTO group_heads(group_route, epoch, last_envelope_id, updated_day)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(group_route) DO UPDATE
               SET epoch = excluded.epoch, last_envelope_id = excluded.last_envelope_id,
                   updated_day = excluded.updated_day",
            params![&route[..], epoch_i + 1, envelope.envelope_id, day(now)],
        )?;
    }

    {
        let mut next = tx.prepare_cached(
            "UPDATE inbox_counters SET next_seq = next_seq + 1 WHERE inbox_id = ?1
             RETURNING next_seq - 1",
        )?;
        let mut dup =
            tx.prepare_cached("SELECT 1 FROM envelopes WHERE inbox_id = ?1 AND envelope_id = ?2")?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO envelopes(inbox_id, seq, envelope_id, kind, payload, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for inbox in recipients {
            // A retried send with the same envelope ID is a no-op for that inbox.
            if dup.exists(params![&inbox[..], envelope.envelope_id])? {
                continue;
            }
            let seq: i64 = next.query_row(params![&inbox[..]], |r| r.get(0))?;
            insert.execute(params![
                &inbox[..],
                seq,
                envelope.envelope_id,
                envelope.kind,
                envelope.payload,
                envelope.expires_at
            ])?;
        }
    }
    tx.commit()?;
    Ok(FanOut::Queued(CommitOutcome::Accepted))
}

/// Envelopes for `inbox` with `seq > after`, oldest first.
pub fn envelopes_after(
    conn: &Connection,
    inbox: &Id,
    after: u64,
    limit: usize,
    now: i64,
) -> RelayResult<Vec<StoredEnvelope>> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, envelope_id, kind, payload FROM envelopes
         WHERE inbox_id = ?1 AND seq > ?2 AND expires_at > ?3 ORDER BY seq LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![
            &inbox[..],
            i64::try_from(after).unwrap_or(i64::MAX),
            now,
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
        |r| {
            Ok(StoredEnvelope {
                seq: u64::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                envelope_id: r.get(1)?,
                kind: r.get(2)?,
                payload: r.get(3)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Number of envelopes waiting in an inbox.
pub fn queued_count(conn: &Connection, inbox: &Id) -> RelayResult<u64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM envelopes WHERE inbox_id = ?1",
        params![&inbox[..]],
        |r| r.get(0),
    )?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Deletes acknowledged envelopes (the device has stored them).
pub fn ack(conn: &mut Connection, inbox: &Id, seqs: &[u64]) -> RelayResult<usize> {
    let tx = conn.transaction()?;
    let mut deleted = 0;
    {
        let mut stmt =
            tx.prepare_cached("DELETE FROM envelopes WHERE inbox_id = ?1 AND seq = ?2")?;
        for seq in seqs {
            deleted += stmt.execute(params![&inbox[..], i64::try_from(*seq).unwrap_or(-1)])?;
        }
    }
    tx.commit()?;
    Ok(deleted)
}

// ─── key packages ───────────────────────────────────────────────────────────

/// Appends one-time key packages and optionally replaces the last-resort package.
pub fn put_key_packages(
    conn: &mut Connection,
    device_id: &Id,
    one_time: &[Vec<u8>],
    last_resort: Option<&[u8]>,
    expires_at: i64,
    max_per_device: usize,
) -> RelayResult<()> {
    let tx = conn.transaction()?;
    let existing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM key_packages WHERE device_id = ?1 AND last_resort = 0",
        params![&device_id[..]],
        |r| r.get(0),
    )?;
    if usize::try_from(existing).unwrap_or(usize::MAX) + one_time.len() > max_per_device {
        return Err(RelayError::QuotaExceeded);
    }
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO key_packages(device_id, key_package, last_resort, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for kp in one_time {
            insert.execute(params![&device_id[..], kp, 0i64, expires_at])?;
        }
        if let Some(lr) = last_resort {
            tx.execute(
                "DELETE FROM key_packages WHERE device_id = ?1 AND last_resort = 1",
                params![&device_id[..]],
            )?;
            insert.execute(params![&device_id[..], lr, 1i64, expires_at])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Takes one key package for `device_id`: the oldest one-time package (deleted on use), or the
/// last-resort package (kept) when none are left.
pub fn take_key_package(
    conn: &mut Connection,
    device_id: &Id,
    now: i64,
) -> RelayResult<Option<Vec<u8>>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let active: bool = tx
        .query_row(
            "SELECT 1 FROM devices WHERE device_id = ?1 AND state = 1",
            params![&device_id[..]],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !active {
        return Ok(None);
    }
    let one_time: Option<(i64, Vec<u8>)> = tx
        .query_row(
            "SELECT id, key_package FROM key_packages
             WHERE device_id = ?1 AND last_resort = 0 AND expires_at > ?2 ORDER BY id LIMIT 1",
            params![&device_id[..], now],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let kp = if let Some((id, kp)) = one_time {
        tx.execute("DELETE FROM key_packages WHERE id = ?1", params![id])?;
        Some(kp)
    } else {
        tx.query_row(
            "SELECT key_package FROM key_packages
             WHERE device_id = ?1 AND last_resort = 1 AND expires_at > ?2",
            params![&device_id[..], now],
            |r| r.get(0),
        )
        .optional()?
    };
    tx.commit()?;
    Ok(kp)
}

/// How many one-time packages remain, and whether a last-resort package exists.
pub fn key_package_count(conn: &Connection, device_id: &Id, now: i64) -> RelayResult<(u32, bool)> {
    let (n, lr): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(last_resort = 0), 0), COALESCE(SUM(last_resort = 1), 0)
         FROM key_packages WHERE device_id = ?1 AND expires_at > ?2",
        params![&device_id[..], now],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((u32::try_from(n).unwrap_or(0), lr > 0))
}

// ─── blobs ──────────────────────────────────────────────────────────────────

/// Upload state of one encrypted file.
#[derive(Clone, Debug)]
pub struct BlobRow {
    /// Uploader.
    pub owner_device: Id,
    /// Declared total size.
    pub size: u64,
    /// Bytes received so far.
    pub received: u64,
    /// Whether the upload finished.
    pub complete: bool,
}

/// Reads a blob's state.
pub fn blob(conn: &Connection, blob_id: &Id, now: i64) -> RelayResult<Option<BlobRow>> {
    Ok(conn
        .query_row(
            "SELECT owner_device, size, received, complete FROM blobs
             WHERE blob_id = ?1 AND expires_at > ?2",
            params![&blob_id[..], now],
            |r| {
                let owner: Vec<u8> = r.get(0)?;
                Ok(BlobRow {
                    owner_device: parse_id(&owner).ok_or(rusqlite::Error::InvalidQuery)?,
                    size: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                    received: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
                    complete: r.get::<_, i64>(3)? != 0,
                })
            },
        )
        .optional()?)
}

/// Creates a blob row after checking the per-device quota.
pub fn create_blob(
    conn: &mut Connection,
    blob_id: &Id,
    owner: &Id,
    size: u64,
    quota: u64,
    expires_at: i64,
) -> RelayResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let used: i64 = tx.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM blobs WHERE owner_device = ?1",
        params![&owner[..]],
        |r| r.get(0),
    )?;
    if u64::try_from(used).unwrap_or(u64::MAX).saturating_add(size) > quota {
        return Err(RelayError::QuotaExceeded);
    }
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO blobs(blob_id, owner_device, size, received, complete, expires_at)
         VALUES (?1, ?2, ?3, 0, 0, ?4)",
        params![
            &blob_id[..],
            &owner[..],
            i64::try_from(size).map_err(|_| RelayError::TooLarge)?,
            expires_at
        ],
    )?;
    if inserted == 0 {
        return Err(RelayError::Conflict("blob exists"));
    }
    tx.commit()?;
    Ok(())
}

/// Records progress of an upload.
pub fn set_blob_progress(
    conn: &Connection,
    blob_id: &Id,
    received: u64,
    complete: bool,
) -> RelayResult<()> {
    conn.execute(
        "UPDATE blobs SET received = ?2, complete = ?3 WHERE blob_id = ?1",
        params![
            &blob_id[..],
            i64::try_from(received).unwrap_or(i64::MAX),
            i64::from(complete)
        ],
    )?;
    Ok(())
}

/// Total bytes of stored files.
pub fn stored_blob_bytes(conn: &Connection) -> RelayResult<u64> {
    let n: i64 = conn.query_row("SELECT COALESCE(SUM(received), 0) FROM blobs", [], |r| {
        r.get(0)
    })?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Total queued envelopes.
pub fn total_queued(conn: &Connection) -> RelayResult<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM envelopes", [], |r| r.get(0))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_devices(n: usize) -> (Db, Vec<(Id, Id)>) {
        let db = Db::open_in_memory().unwrap();
        let mut out = Vec::new();
        db.call_blocking(|c| {
            for _ in 0..n {
                let p = create_pending_device(c, [1; 32], None, false, i64::MAX)?;
                activate_device(c, &p.device_id, &[2; 32])?;
                out.push((p.device_id, p.inbox_id));
            }
            Ok(())
        })
        .unwrap();
        (db, out)
    }

    fn env<'a>(id: &'a [u8], payload: &'a [u8]) -> OutgoingEnvelope<'a> {
        OutgoingEnvelope {
            envelope_id: id,
            kind: 1,
            payload,
            expires_at: i64::MAX,
        }
    }

    #[test]
    fn fan_out_orders_and_deduplicates() {
        let (db, devs) = db_with_devices(3);
        let inboxes: Vec<Id> = devs.iter().map(|d| d.1).collect();
        db.call_blocking(|c| {
            for i in 0..5u8 {
                let id = [i; 16];
                let r = fan_out(c, &inboxes, &env(&id, &[i; 8]), None, 100, 0)?;
                assert_eq!(r, FanOut::Queued(CommitOutcome::Accepted));
            }
            // Retrying envelope 0 must not create a duplicate.
            fan_out(c, &inboxes, &env(&[0; 16], &[0; 8]), None, 100, 0)?;
            let got = envelopes_after(c, &inboxes[0], 0, 100, 0)?;
            assert_eq!(got.len(), 5);
            assert!(got.windows(2).all(|w| w[0].seq < w[1].seq));
            assert_eq!(ack(c, &inboxes[0], &[got[0].seq, got[1].seq])?, 2);
            assert_eq!(queued_count(c, &inboxes[0])?, 3);
            assert_eq!(queued_count(c, &inboxes[1])?, 5);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn exactly_one_commit_per_epoch() {
        let (db, devs) = db_with_devices(2);
        let inboxes: Vec<Id> = devs.iter().map(|d| d.1).collect();
        let route = [9u8; 16];
        db.call_blocking(|c| {
            let a = fan_out(c, &inboxes, &env(&[1; 16], b"a"), Some((&route, 0)), 100, 0)?;
            assert_eq!(a, FanOut::Queued(CommitOutcome::Accepted));
            let b = fan_out(c, &inboxes, &env(&[2; 16], b"b"), Some((&route, 0)), 100, 0)?;
            assert_eq!(
                b,
                FanOut::Queued(CommitOutcome::StaleEpoch { current_epoch: 1 })
            );
            // Retrying the winner is idempotent.
            let a2 = fan_out(c, &inboxes, &env(&[1; 16], b"a"), Some((&route, 0)), 100, 0)?;
            assert_eq!(a2, FanOut::Queued(CommitOutcome::Accepted));
            let next = fan_out(c, &inboxes, &env(&[3; 16], b"c"), Some((&route, 1)), 100, 0)?;
            assert_eq!(next, FanOut::Queued(CommitOutcome::Accepted));
            // A brand-new route must start at epoch 0.
            let wrong = fan_out(
                c,
                &inboxes,
                &env(&[4; 16], b"d"),
                Some((&[8; 16], 3)),
                100,
                0,
            )?;
            assert_eq!(
                wrong,
                FanOut::Queued(CommitOutcome::StaleEpoch { current_epoch: 0 })
            );
            assert_eq!(queued_count(c, &inboxes[0])?, 2);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn unknown_and_revoked_inboxes_are_rejected() {
        let (db, devs) = db_with_devices(2);
        db.call_blocking(|c| {
            revoke_device(c, &devs[1].0, 0)?;
            let r = fan_out(
                c,
                &[devs[0].1, devs[1].1],
                &env(&[1; 16], b"x"),
                None,
                100,
                0,
            )?;
            assert_eq!(r, FanOut::UnknownInboxes(vec![devs[1].1]));
            assert_eq!(
                queued_count(c, &devs[0].1)?,
                0,
                "nothing is queued on rejection"
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn key_packages_are_one_time_with_last_resort() {
        let (db, devs) = db_with_devices(1);
        let d = devs[0].0;
        db.call_blocking(|c| {
            put_key_packages(
                c,
                &d,
                &[b"a".to_vec(), b"b".to_vec()],
                Some(b"lr"),
                i64::MAX,
                100,
            )?;
            assert_eq!(key_package_count(c, &d, 0)?, (2, true));
            assert_eq!(take_key_package(c, &d, 0)?.unwrap(), b"a");
            assert_eq!(take_key_package(c, &d, 0)?.unwrap(), b"b");
            assert_eq!(take_key_package(c, &d, 0)?.unwrap(), b"lr");
            assert_eq!(
                take_key_package(c, &d, 0)?.unwrap(),
                b"lr",
                "last resort is kept"
            );
            put_key_packages(c, &d, &[], Some(b"lr2"), i64::MAX, 100)?;
            assert_eq!(take_key_package(c, &d, 0)?.unwrap(), b"lr2");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn registration_secrets_are_single_use() {
        let db = Db::open_in_memory().unwrap();
        db.call_blocking(|c| {
            let p = create_pending_device(c, [1; 32], None, true, 100)?;
            assert!(pending_device_for_secret(c, &p.registration_secret, 50)?.is_some());
            assert!(pending_device_for_secret(c, &p.registration_secret, 150)?.is_none());
            activate_device(c, &p.device_id, &[3; 32])?;
            assert!(pending_device_for_secret(c, &p.registration_secret, 50)?.is_none());
            assert!(activate_device(c, &p.device_id, &[3; 32]).is_err());
            let d = device(c, &p.device_id)?.unwrap();
            assert_eq!(d.state, DeviceState::Active);
            assert!(d.is_admin);
            Ok(())
        })
        .unwrap();
    }
}
