//! Encrypted local storage (architecture §6.6, §7.9).
//!
//! One SQLCipher 4 file (AES-256 pages, HMAC-SHA512 page authentication), opened in raw-key
//! mode with the 32-byte database master key that the Mac app unwraps with the Secure Enclave.
//! The same connection carries OpenMLS's state, so a received message and the MLS ratchet step
//! that decrypted it commit in one transaction.

mod schema;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, id16, now_ms};

pub use rusqlite;

/// Kind of chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatKind {
    /// A two-person chat.
    Direct = 1,
    /// A group (up to 50 members).
    Group = 2,
}

impl ChatKind {
    fn from_i64(v: i64) -> Self {
        if v == 2 { Self::Group } else { Self::Direct }
    }
}

/// How a contact was verified (architecture §7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// Never verified.
    Unverified = 0,
    /// QR codes scanned in person and emoji confirmed.
    InPerson = 1,
    /// Safety number compared remotely.
    Remote = 2,
    /// A previously verified contact presented a key that was not linked: blocked until
    /// re-verified.
    Changed = 3,
}

impl Verification {
    /// From the stored integer.
    #[must_use]
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::InPerson,
            2 => Self::Remote,
            3 => Self::Changed,
            _ => Self::Unverified,
        }
    }
}

/// Message kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    /// Text.
    Text = 1,
    /// Image, file or voice note.
    Attachment = 2,
    /// "Ada joined", timer changes, gap warnings.
    System = 3,
    /// Call history entry.
    Call = 4,
    /// A message type this build doesn't understand.
    Unsupported = 5,
}

impl MessageKind {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Text,
            2 => Self::Attachment,
            3 => Self::System,
            4 => Self::Call,
            _ => Self::Unsupported,
        }
    }
}

/// Delivery state (architecture §13.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessageStatus {
    /// Saved locally, waiting to send (clock).
    Pending = 0,
    /// Accepted by the relay (one tick).
    Sent = 1,
    /// Delivered to every recipient device (two ticks).
    Delivered = 2,
    /// Read (two accent ticks).
    Read = 3,
    /// Couldn't send (red exclamation mark).
    Failed = 4,
    /// An incoming message.
    Received = 10,
    /// An incoming message the local user has read.
    Seen = 11,
}

impl MessageStatus {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Sent,
            2 => Self::Delivered,
            3 => Self::Read,
            4 => Self::Failed,
            10 => Self::Received,
            11 => Self::Seen,
            _ => Self::Pending,
        }
    }
}

/// Where an attachment's bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentState {
    /// Only on the relay.
    Remote = 0,
    /// Being downloaded.
    Downloading = 1,
    /// Ciphertext stored locally.
    Local = 2,
    /// Upload or download failed.
    Failed = 3,
    /// Being uploaded.
    Uploading = 4,
}

impl AttachmentState {
    fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Downloading,
            2 => Self::Local,
            3 => Self::Failed,
            4 => Self::Uploading,
            _ => Self::Remote,
        }
    }
}

/// A chat.
#[derive(Clone, Debug)]
pub struct Chat {
    /// The MLS group ID (32 random bytes).
    pub chat_id: Vec<u8>,
    /// DM or group.
    pub kind: ChatKind,
    /// The only group identifier the relay sees.
    pub group_route: Id16,
    /// Group name (empty for DMs).
    pub name: String,
    /// Group description.
    pub description: String,
    /// Group photo (JPEG).
    pub avatar: Option<Vec<u8>>,
    /// The other person, for DMs.
    pub dm_user: Option<Id16>,
    /// The MLS cipher suite code point, recorded for future ReInit migrations.
    pub suite: u16,
    /// Creation time.
    pub created_ms: i64,
    /// Last message time (sorting).
    pub last_activity_ms: i64,
    /// Unread count.
    pub unread: u32,
    /// Notifications muted.
    pub muted: bool,
    /// Pinned to the top.
    pub pinned: bool,
    /// Archived.
    pub archived: bool,
    /// Disappearing-message timer; 0 means off.
    pub timer_s: u32,
    /// The local user left or was removed.
    pub left: bool,
}

/// A person.
#[derive(Clone, Debug)]
pub struct Contact {
    /// Random user ID.
    pub user_id: Id16,
    /// Name they chose, shared only with friends.
    pub display_name: String,
    /// Profile photo.
    pub avatar: Option<Vec<u8>>,
    /// How they were verified.
    pub verification: Verification,
    /// Blocked.
    pub blocked: bool,
}

/// One of a contact's devices.
#[derive(Clone, Debug)]
pub struct ContactDevice {
    /// Device ID.
    pub device_id: Id16,
    /// Owner.
    pub user_id: Id16,
    /// Inbox to deliver to.
    pub inbox_id: Id16,
    /// MLS signature public key.
    pub signature_key: Vec<u8>,
}

/// Attachment metadata.
#[derive(Clone, Debug, Default)]
pub struct AttachmentInfo {
    /// Blob on the relay.
    pub blob_id: Id16,
    /// Per-file key.
    pub key: Vec<u8>,
    /// SHA-256 of the padded blob.
    pub sha256: Vec<u8>,
    /// Plaintext size.
    pub size: u64,
    /// Padded blob size.
    pub blob_size: u64,
    /// MIME type.
    pub mime: String,
    /// File name.
    pub file_name: String,
    /// Image/video width.
    pub width: u32,
    /// Image/video height.
    pub height: u32,
    /// Audio/video duration.
    pub duration_ms: u32,
    /// Tiny inline JPEG.
    pub thumbnail: Option<Vec<u8>>,
    /// BlurHash placeholder.
    pub blurhash: String,
    /// Voice note.
    pub voice_note: bool,
    /// Voice note waveform buckets.
    pub waveform: Option<Vec<u8>>,
    /// Transfer state.
    pub state: Option<AttachmentState>,
    /// Transfer progress 0…1.
    pub progress: f64,
}

/// Aggregated reactions on a message.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactionSummary {
    /// The emoji.
    pub emoji: String,
    /// How many people reacted with it.
    pub count: u32,
    /// Whether the local user is one of them.
    pub mine: bool,
}

/// A stored message.
#[derive(Clone, Debug)]
pub struct Message {
    /// Local row ID (stable ordering tie-breaker).
    pub row_id: i64,
    /// Random message ID.
    pub message_id: Id16,
    /// Chat.
    pub chat_id: Vec<u8>,
    /// Author.
    pub sender_user: Id16,
    /// Author's device.
    pub sender_device: Id16,
    /// Sent by the local user.
    pub outgoing: bool,
    /// Kind.
    pub kind: MessageKind,
    /// Text, caption or system text.
    pub body: String,
    /// The message this replies to.
    pub reply_to: Option<Id16>,
    /// Sender's clock.
    pub sent_at_ms: i64,
    /// Local receive time.
    pub received_at_ms: i64,
    /// Display order key (sender clock, clamped).
    pub sort_ms: i64,
    /// Delivery state.
    pub status: MessageStatus,
    /// Edited at least once.
    pub edited: bool,
    /// Deleted for everyone.
    pub deleted: bool,
    /// Disappearing-message deadline.
    pub expires_at_ms: Option<i64>,
    /// Attachment metadata, for attachment messages.
    pub attachment: Option<AttachmentInfo>,
    /// Reactions.
    pub reactions: Vec<ReactionSummary>,
}

/// A full-text search hit.
#[derive(Clone, Debug)]
pub struct SearchHit {
    /// The chat.
    pub chat_id: Vec<u8>,
    /// The message.
    pub message_id: Id16,
    /// Snippet with the match highlighted by `[` `]`.
    pub snippet: String,
    /// When it was sent.
    pub sent_at_ms: i64,
}

/// A queued outgoing envelope (outbox pattern, architecture §6.4).
#[derive(Clone, Debug)]
pub struct OutboxItem {
    /// Local ID.
    pub id: i64,
    /// Idempotency key.
    pub envelope_id: Id16,
    /// Chat, when the envelope belongs to one.
    pub chat_id: Option<Vec<u8>>,
    /// Message whose status follows this envelope.
    pub message_id: Option<Id16>,
    /// `PayloadKind`.
    pub kind: i32,
    /// Recipient inboxes.
    pub recipients: Vec<Id16>,
    /// Padded MLS ciphertext.
    pub payload: Vec<u8>,
    /// Relay TTL (0 = default).
    pub ttl_s: u32,
    /// Send attempts so far.
    pub attempts: u32,
}

/// Thread-safe handle to the encrypted database.
pub struct Store {
    conn: Mutex<Connection>,
    path: PathBuf,
}

fn apply_key(conn: &Connection, key: &[u8; 32]) -> CoreResult<()> {
    // SQLCipher would otherwise log decryption failures to stderr.
    let _ = conn.execute_batch("PRAGMA cipher_log_level = NONE;");
    let pragma = Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", hex::encode(key)));
    conn.execute_batch(&pragma)?;
    // A wrong key surfaces here, not on first use.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map_err(|_| CoreError::Storage("the database key is wrong or the file is damaged".into()))?;
    Ok(())
}

impl Store {
    /// Opens (or creates) the encrypted database at `path` with the raw 32-byte `key`.
    pub fn open(path: &Path, key: &[u8; 32]) -> CoreResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        apply_key(&conn, key)?;
        Self::init(conn, path.to_path_buf())
    }

    /// An encrypted in-memory database for tests.
    pub fn open_in_memory() -> CoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        apply_key(&conn, &[7u8; 32])?;
        Self::init(conn, PathBuf::from(":memory:"))
    }

    fn init(mut conn: Connection, path: PathBuf) -> CoreResult<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        if path.as_os_str() != ":memory:" {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "secure_delete", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        migrate(&mut conn)?;
        crate::mls::run_storage_migrations(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    /// Path of the database file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Locks the connection for a synchronous unit of work.
    pub fn lock(&self) -> CoreResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| CoreError::Storage("database lock poisoned".into()))
    }

    /// Runs `f` with the connection inside one transaction.
    pub fn tx<T>(&self, f: impl FnOnce(&Connection) -> CoreResult<T>) -> CoreResult<T> {
        let conn = self.lock()?;
        let tx = conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

fn migrate(conn: &mut Connection) -> CoreResult<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let current = i64::try_from(schema::MIGRATIONS.len()).map_err(CoreError::internal)?;
    if version > current {
        return Err(CoreError::Storage(
            "this database was created by a newer version of Asiri".into(),
        ));
    }
    for (i, sql) in schema::MIGRATIONS
        .iter()
        .enumerate()
        .skip(usize::try_from(version).unwrap_or(0))
    {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i64::try_from(i + 1).unwrap_or(1))?;
        tx.commit()?;
    }
    Ok(())
}

fn blob16(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Id16> {
    let v: Vec<u8> = row.get(idx)?;
    id16(&v).ok_or_else(|| {
        rusqlite::Error::InvalidColumnType(idx, "id16".into(), rusqlite::types::Type::Blob)
    })
}

fn opt_blob16(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<Id16>> {
    let v: Option<Vec<u8>> = row.get(idx)?;
    Ok(v.and_then(|v| id16(&v)))
}

fn u32_col(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<u32> {
    Ok(u32::try_from(row.get::<_, i64>(idx)?).unwrap_or(0))
}

// ─── key/value ──────────────────────────────────────────────────────────────

/// Reads a value.
pub fn kv_get(conn: &Connection, key: &str) -> CoreResult<Option<Vec<u8>>> {
    Ok(conn
        .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// Writes a value.
pub fn kv_set(conn: &Connection, key: &str, value: &[u8]) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO kv(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Deletes a value.
pub fn kv_delete(conn: &Connection, key: &str) -> CoreResult<()> {
    conn.execute("DELETE FROM kv WHERE key = ?1", params![key])?;
    Ok(())
}

// ─── contacts ───────────────────────────────────────────────────────────────

/// Inserts a contact or updates their name.
pub fn upsert_contact(
    conn: &Connection,
    user_id: &Id16,
    display_name: Option<&str>,
) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO contacts(user_id, display_name, created_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET display_name =
           CASE WHEN ?2 = '' THEN contacts.display_name ELSE ?2 END",
        params![&user_id[..], display_name.unwrap_or(""), now_ms()],
    )?;
    Ok(())
}

/// Sets a contact's profile photo.
pub fn set_contact_avatar(
    conn: &Connection,
    user_id: &Id16,
    avatar: Option<&[u8]>,
) -> CoreResult<()> {
    conn.execute(
        "UPDATE contacts SET avatar = ?2 WHERE user_id = ?1",
        params![&user_id[..], avatar],
    )?;
    Ok(())
}

/// Sets a contact's verification state.
pub fn set_verification(conn: &Connection, user_id: &Id16, v: Verification) -> CoreResult<()> {
    conn.execute(
        "UPDATE contacts SET verification = ?2 WHERE user_id = ?1",
        params![&user_id[..], v as i64],
    )?;
    Ok(())
}

/// Blocks or unblocks a contact.
pub fn set_blocked(conn: &Connection, user_id: &Id16, blocked: bool) -> CoreResult<()> {
    conn.execute(
        "UPDATE contacts SET blocked = ?2 WHERE user_id = ?1",
        params![&user_id[..], i64::from(blocked)],
    )?;
    Ok(())
}

fn contact_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Contact> {
    Ok(Contact {
        user_id: blob16(row, 0)?,
        display_name: row.get(1)?,
        avatar: row.get(2)?,
        verification: Verification::from_i64(row.get(3)?),
        blocked: row.get::<_, i64>(4)? != 0,
    })
}

/// One contact.
pub fn contact(conn: &Connection, user_id: &Id16) -> CoreResult<Option<Contact>> {
    Ok(conn
        .query_row(
            "SELECT user_id, display_name, avatar, verification, blocked FROM contacts WHERE user_id = ?1",
            params![&user_id[..]],
            contact_from_row,
        )
        .optional()?)
}

/// All contacts, by name.
pub fn contacts(conn: &Connection) -> CoreResult<Vec<Contact>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, display_name, avatar, verification, blocked FROM contacts
         ORDER BY display_name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], contact_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Records a contact's device.
pub fn upsert_device(conn: &Connection, d: &ContactDevice) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO contact_devices(device_id, user_id, inbox_id, signature_key) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(device_id) DO UPDATE SET inbox_id = excluded.inbox_id",
        params![&d.device_id[..], &d.user_id[..], &d.inbox_id[..], &d.signature_key],
    )?;
    Ok(())
}

fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContactDevice> {
    Ok(ContactDevice {
        device_id: blob16(row, 0)?,
        user_id: blob16(row, 1)?,
        inbox_id: blob16(row, 2)?,
        signature_key: row.get(3)?,
    })
}

/// A device by ID.
pub fn device(conn: &Connection, device_id: &Id16) -> CoreResult<Option<ContactDevice>> {
    Ok(conn
        .query_row(
            "SELECT device_id, user_id, inbox_id, signature_key FROM contact_devices WHERE device_id = ?1",
            params![&device_id[..]],
            device_from_row,
        )
        .optional()?)
}

/// A user's devices.
pub fn devices_of(conn: &Connection, user_id: &Id16) -> CoreResult<Vec<ContactDevice>> {
    let mut stmt = conn.prepare(
        "SELECT device_id, user_id, inbox_id, signature_key FROM contact_devices WHERE user_id = ?1",
    )?;
    let rows = stmt.query_map(params![&user_id[..]], device_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ─── pending invites (inviter side of §11.2) ────────────────────────────────

/// Remembers a device whose QR code we scanned, so its Welcome is recognised as verified.
pub fn add_pending_invite(
    conn: &Connection,
    signature_key: &[u8],
    user_id: &Id16,
    name: &str,
) -> CoreResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO pending_invites(signature_key, user_id, display_name, created_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![signature_key, &user_id[..], name, now_ms()],
    )?;
    Ok(())
}

/// Takes the pending invite for a signature key, if any.
pub fn take_pending_invite(
    conn: &Connection,
    signature_key: &[u8],
) -> CoreResult<Option<(Id16, String)>> {
    let row = conn
        .query_row(
            "SELECT user_id, display_name FROM pending_invites WHERE signature_key = ?1",
            params![signature_key],
            |r| Ok((blob16(r, 0)?, r.get::<_, String>(1)?)),
        )
        .optional()?;
    if row.is_some() {
        conn.execute(
            "DELETE FROM pending_invites WHERE signature_key = ?1",
            params![signature_key],
        )?;
    }
    Ok(row)
}

// ─── chats ──────────────────────────────────────────────────────────────────

const CHAT_COLUMNS: &str = "chat_id, kind, group_route, name, description, avatar, dm_user, suite,
  created_ms, last_activity_ms, unread, muted, pinned, archived, timer_s, left_chat";

fn chat_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chat> {
    Ok(Chat {
        chat_id: row.get(0)?,
        kind: ChatKind::from_i64(row.get(1)?),
        group_route: blob16(row, 2)?,
        name: row.get(3)?,
        description: row.get(4)?,
        avatar: row.get(5)?,
        dm_user: opt_blob16(row, 6)?,
        suite: u16::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
        created_ms: row.get(8)?,
        last_activity_ms: row.get(9)?,
        unread: u32_col(row, 10)?,
        muted: row.get::<_, i64>(11)? != 0,
        pinned: row.get::<_, i64>(12)? != 0,
        archived: row.get::<_, i64>(13)? != 0,
        timer_s: u32_col(row, 14)?,
        left: row.get::<_, i64>(15)? != 0,
    })
}

/// Creates a chat record.
pub fn insert_chat(conn: &Connection, chat: &Chat) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO chats(chat_id, kind, group_route, name, description, avatar, dm_user, suite,
           created_ms, last_activity_ms, timer_s)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            chat.chat_id,
            chat.kind as i64,
            &chat.group_route[..],
            chat.name,
            chat.description,
            chat.avatar,
            chat.dm_user.as_ref().map(|u| &u[..]),
            i64::from(chat.suite),
            chat.created_ms,
            chat.last_activity_ms,
            i64::from(chat.timer_s)
        ],
    )?;
    Ok(())
}

/// One chat.
pub fn chat(conn: &Connection, chat_id: &[u8]) -> CoreResult<Option<Chat>> {
    Ok(conn
        .query_row(
            &format!("SELECT {CHAT_COLUMNS} FROM chats WHERE chat_id = ?1"),
            params![chat_id],
            chat_from_row,
        )
        .optional()?)
}

/// The chat with a given relay route.
pub fn chat_by_route(conn: &Connection, route: &Id16) -> CoreResult<Option<Chat>> {
    Ok(conn
        .query_row(
            &format!("SELECT {CHAT_COLUMNS} FROM chats WHERE group_route = ?1"),
            params![&route[..]],
            chat_from_row,
        )
        .optional()?)
}

/// The DM with a user, if one exists.
pub fn dm_with(conn: &Connection, user_id: &Id16) -> CoreResult<Option<Chat>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {CHAT_COLUMNS} FROM chats WHERE kind = 1 AND dm_user = ?1 AND left_chat = 0"
            ),
            params![&user_id[..]],
            chat_from_row,
        )
        .optional()?)
}

/// Every chat, pinned first, then by recent activity.
pub fn chats(conn: &Connection) -> CoreResult<Vec<Chat>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CHAT_COLUMNS} FROM chats ORDER BY pinned DESC, last_activity_ms DESC"
    ))?;
    let rows = stmt.query_map([], chat_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Updates group metadata.
pub fn set_chat_meta(
    conn: &Connection,
    chat_id: &[u8],
    name: &str,
    description: &str,
    avatar: Option<&[u8]>,
) -> CoreResult<()> {
    conn.execute(
        "UPDATE chats SET name = ?2, description = ?3, avatar = COALESCE(?4, avatar) WHERE chat_id = ?1",
        params![chat_id, name, description, avatar],
    )?;
    Ok(())
}

/// Sets a boolean chat flag (`muted`, `pinned`, `archived`, `left_chat`).
pub fn set_chat_flag(conn: &Connection, chat_id: &[u8], flag: &str, value: bool) -> CoreResult<()> {
    let column = match flag {
        "muted" | "pinned" | "archived" | "left_chat" => flag,
        _ => return Err(CoreError::internal("unknown chat flag")),
    };
    conn.execute(
        &format!("UPDATE chats SET {column} = ?2 WHERE chat_id = ?1"),
        params![chat_id, i64::from(value)],
    )?;
    Ok(())
}

/// Sets the disappearing-message timer.
pub fn set_chat_timer(conn: &Connection, chat_id: &[u8], seconds: u32) -> CoreResult<()> {
    conn.execute(
        "UPDATE chats SET timer_s = ?2 WHERE chat_id = ?1",
        params![chat_id, i64::from(seconds)],
    )?;
    Ok(())
}

/// Bumps a chat's activity time and optionally its unread count.
pub fn touch_chat(
    conn: &Connection,
    chat_id: &[u8],
    at_ms: i64,
    unread_delta: i64,
) -> CoreResult<()> {
    conn.execute(
        "UPDATE chats SET last_activity_ms = MAX(last_activity_ms, ?2), unread = MAX(0, unread + ?3)
         WHERE chat_id = ?1",
        params![chat_id, at_ms, unread_delta],
    )?;
    Ok(())
}

/// Clears the unread count.
pub fn clear_unread(conn: &Connection, chat_id: &[u8]) -> CoreResult<()> {
    conn.execute(
        "UPDATE chats SET unread = 0 WHERE chat_id = ?1",
        params![chat_id],
    )?;
    Ok(())
}

/// When this device last sent a self-update commit in the chat.
pub fn last_self_update(conn: &Connection, chat_id: &[u8]) -> CoreResult<i64> {
    Ok(conn.query_row(
        "SELECT last_self_update_ms FROM chats WHERE chat_id = ?1",
        params![chat_id],
        |r| r.get(0),
    )?)
}

/// Records a self-update commit.
pub fn set_last_self_update(conn: &Connection, chat_id: &[u8], at_ms: i64) -> CoreResult<()> {
    conn.execute(
        "UPDATE chats SET last_self_update_ms = ?2 WHERE chat_id = ?1",
        params![chat_id, at_ms],
    )?;
    Ok(())
}

/// Deletes a chat and everything in it.
pub fn delete_chat(conn: &Connection, chat_id: &[u8]) -> CoreResult<()> {
    conn.execute("DELETE FROM outbox WHERE chat_id = ?1", params![chat_id])?;
    conn.execute("DELETE FROM chats WHERE chat_id = ?1", params![chat_id])?;
    Ok(())
}

// ─── sequence numbers and gap detection (§12 "Gap detection") ───────────────

/// The next `sender_seq` this device uses in a chat.
pub fn next_my_seq(conn: &Connection, chat_id: &[u8]) -> CoreResult<u64> {
    let seq: i64 = conn.query_row(
        "INSERT INTO my_seqs(chat_id, next) VALUES (?1, 2)
         ON CONFLICT(chat_id) DO UPDATE SET next = next + 1
         RETURNING next - 1",
        params![chat_id],
        |r| r.get(0),
    )?;
    Ok(u64::try_from(seq).unwrap_or(1))
}

/// Records a received `sender_seq`; returns the newly detected missing sequence numbers.
pub fn observe_seq(
    conn: &Connection,
    chat_id: &[u8],
    device: &Id16,
    seq: u64,
    at_ms: i64,
) -> CoreResult<Vec<u64>> {
    let seq_i = i64::try_from(seq).map_err(CoreError::internal)?;
    conn.execute(
        "DELETE FROM missing_seqs WHERE chat_id = ?1 AND device_id = ?2 AND seq = ?3",
        params![chat_id, &device[..], seq_i],
    )?;
    let highest: Option<i64> = conn
        .query_row(
            "SELECT highest FROM sender_seqs WHERE chat_id = ?1 AND device_id = ?2",
            params![chat_id, &device[..]],
            |r| r.get(0),
        )
        .optional()?;
    let mut missing = Vec::new();
    match highest {
        Some(h) if seq_i <= h => {}
        prev => {
            // Without history (first message we see from this device in this chat) we don't
            // know what came before, so only gaps after the first observation count.
            if let Some(h) = prev {
                let gap = (seq_i - h - 1).clamp(0, 500);
                for s in (h + 1)..(h + 1 + gap) {
                    conn.execute(
                        "INSERT OR IGNORE INTO missing_seqs(chat_id, device_id, seq, since_ms) VALUES (?1, ?2, ?3, ?4)",
                        params![chat_id, &device[..], s, at_ms],
                    )?;
                    missing.push(u64::try_from(s).unwrap_or(0));
                }
            }
            conn.execute(
                "INSERT INTO sender_seqs(chat_id, device_id, highest) VALUES (?1, ?2, ?3)
                 ON CONFLICT(chat_id, device_id) DO UPDATE SET highest = excluded.highest",
                params![chat_id, &device[..], seq_i],
            )?;
        }
    }
    Ok(missing)
}

/// Gaps older than `older_than_ms` that have not been reported yet: `(chat, device, count)`.
pub fn take_overdue_gaps(
    conn: &Connection,
    older_than_ms: i64,
) -> CoreResult<Vec<(Vec<u8>, Id16, u32)>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, device_id, COUNT(*) FROM missing_seqs
         WHERE reported = 0 AND since_ms < ?1 GROUP BY chat_id, device_id",
    )?;
    let rows: Vec<(Vec<u8>, Id16, u32)> = stmt
        .query_map(params![older_than_ms], |r| {
            Ok((r.get(0)?, blob16(r, 1)?, u32_col(r, 2)?))
        })?
        .collect::<Result<_, _>>()?;
    conn.execute(
        "UPDATE missing_seqs SET reported = 1 WHERE reported = 0 AND since_ms < ?1",
        params![older_than_ms],
    )?;
    Ok(rows)
}

// ─── envelopes we have already processed (replay defence) ───────────────────

/// Marks an envelope as processed; returns `false` if it was already seen (a replay).
pub fn mark_envelope_processed(
    conn: &Connection,
    envelope_id: &[u8],
    at_ms: i64,
) -> CoreResult<bool> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO processed_envelopes(envelope_id, at_ms) VALUES (?1, ?2)",
        params![envelope_id, at_ms],
    )?;
    Ok(inserted == 1)
}

/// Forgets processed-envelope IDs older than `before_ms` (relay retention is 30 days).
pub fn prune_processed(conn: &Connection, before_ms: i64) -> CoreResult<()> {
    conn.execute(
        "DELETE FROM processed_envelopes WHERE at_ms < ?1",
        params![before_ms],
    )?;
    Ok(())
}

// ─── messages ───────────────────────────────────────────────────────────────

/// A new message to store.
pub struct NewMessage<'a> {
    /// Random ID.
    pub message_id: Id16,
    /// Chat.
    pub chat_id: &'a [u8],
    /// Author.
    pub sender_user: Id16,
    /// Author's device.
    pub sender_device: Id16,
    /// Sent by us.
    pub outgoing: bool,
    /// Kind.
    pub kind: MessageKind,
    /// Text.
    pub body: &'a str,
    /// Reply target.
    pub reply_to: Option<Id16>,
    /// Sender clock.
    pub sent_at_ms: i64,
    /// Receive time.
    pub received_at_ms: i64,
    /// Initial status.
    pub status: MessageStatus,
    /// Disappearing deadline.
    pub expires_at_ms: Option<i64>,
}

/// Inserts a message; returns `false` if a message with this ID already exists.
pub fn insert_message(conn: &Connection, m: &NewMessage<'_>) -> CoreResult<bool> {
    // Timestamps far in the future are clamped (§12 "Ordering").
    let sort_ms = m.sent_at_ms.min(m.received_at_ms + 60_000);
    let n = conn.execute(
        "INSERT OR IGNORE INTO messages(message_id, chat_id, sender_user, sender_device, outgoing, kind,
           body, reply_to, sent_at_ms, received_at_ms, sort_ms, status, expires_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            &m.message_id[..],
            m.chat_id,
            &m.sender_user[..],
            &m.sender_device[..],
            i64::from(m.outgoing),
            m.kind as i64,
            m.body,
            m.reply_to.as_ref().map(|r| &r[..]),
            m.sent_at_ms,
            m.received_at_ms,
            sort_ms,
            m.status as i64,
            m.expires_at_ms
        ],
    )?;
    Ok(n == 1)
}

/// Stores attachment metadata for a message.
pub fn insert_attachment(
    conn: &Connection,
    message_id: &Id16,
    a: &AttachmentInfo,
) -> CoreResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO attachments(message_id, blob_id, key, sha256, size, blob_size, mime, file_name,
           width, height, duration_ms, thumbnail, blurhash, voice_note, waveform, state, progress)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            &message_id[..],
            &a.blob_id[..],
            a.key,
            a.sha256,
            i64::try_from(a.size).unwrap_or(i64::MAX),
            i64::try_from(a.blob_size).unwrap_or(i64::MAX),
            a.mime,
            a.file_name,
            i64::from(a.width),
            i64::from(a.height),
            i64::from(a.duration_ms),
            a.thumbnail,
            a.blurhash,
            i64::from(a.voice_note),
            a.waveform,
            a.state.unwrap_or(AttachmentState::Remote) as i64,
            a.progress
        ],
    )?;
    Ok(())
}

/// Updates an attachment's transfer state.
pub fn set_attachment_state(
    conn: &Connection,
    message_id: &Id16,
    state: AttachmentState,
    progress: f64,
) -> CoreResult<()> {
    conn.execute(
        "UPDATE attachments SET state = ?2, progress = ?3 WHERE message_id = ?1",
        params![&message_id[..], state as i64, progress],
    )?;
    Ok(())
}

const MESSAGE_COLUMNS: &str = "m.row_id, m.message_id, m.chat_id, m.sender_user, m.sender_device, m.outgoing,
  m.kind, m.body, m.reply_to, m.sent_at_ms, m.received_at_ms, m.sort_ms, m.status, m.edited, m.deleted,
  m.expires_at_ms, a.blob_id, a.key, a.sha256, a.size, a.blob_size, a.mime, a.file_name, a.width, a.height,
  a.duration_ms, a.thumbnail, a.blurhash, a.voice_note, a.waveform, a.state, a.progress";

fn message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let has_attachment: Option<Vec<u8>> = row.get(16)?;
    let attachment = match has_attachment {
        Some(blob_id) => Some(AttachmentInfo {
            blob_id: id16(&blob_id).unwrap_or_default(),
            key: row.get(17)?,
            sha256: row.get(18)?,
            size: u64::try_from(row.get::<_, i64>(19)?).unwrap_or(0),
            blob_size: u64::try_from(row.get::<_, i64>(20)?).unwrap_or(0),
            mime: row.get(21)?,
            file_name: row.get(22)?,
            width: u32_col(row, 23)?,
            height: u32_col(row, 24)?,
            duration_ms: u32_col(row, 25)?,
            thumbnail: row.get(26)?,
            blurhash: row.get(27)?,
            voice_note: row.get::<_, i64>(28)? != 0,
            waveform: row.get(29)?,
            state: Some(AttachmentState::from_i64(row.get(30)?)),
            progress: row.get(31)?,
        }),
        None => None,
    };
    Ok(Message {
        row_id: row.get(0)?,
        message_id: blob16(row, 1)?,
        chat_id: row.get(2)?,
        sender_user: blob16(row, 3)?,
        sender_device: blob16(row, 4)?,
        outgoing: row.get::<_, i64>(5)? != 0,
        kind: MessageKind::from_i64(row.get(6)?),
        body: row.get(7)?,
        reply_to: opt_blob16(row, 8)?,
        sent_at_ms: row.get(9)?,
        received_at_ms: row.get(10)?,
        sort_ms: row.get(11)?,
        status: MessageStatus::from_i64(row.get(12)?),
        edited: row.get::<_, i64>(13)? != 0,
        deleted: row.get::<_, i64>(14)? != 0,
        expires_at_ms: row.get(15)?,
        attachment,
        reactions: vec![],
    })
}

fn load_reactions(conn: &Connection, me: &Id16, m: &mut Message) -> CoreResult<()> {
    let mut stmt = conn.prepare_cached(
        "SELECT emoji, COUNT(*), SUM(user_id = ?2) FROM reactions WHERE message_id = ?1
         GROUP BY emoji ORDER BY MIN(rowid)",
    )?;
    m.reactions = stmt
        .query_map(params![&m.message_id[..], &me[..]], |r| {
            Ok(ReactionSummary {
                emoji: r.get(0)?,
                count: u32_col(r, 1)?,
                mine: r.get::<_, i64>(2)? > 0,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(())
}

/// One message.
pub fn message(conn: &Connection, me: &Id16, message_id: &Id16) -> CoreResult<Option<Message>> {
    let m = conn
        .query_row(
            &format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m LEFT JOIN attachments a USING (message_id)
                 WHERE m.message_id = ?1"
            ),
            params![&message_id[..]],
            message_from_row,
        )
        .optional()?;
    match m {
        Some(mut m) => {
            load_reactions(conn, me, &mut m)?;
            Ok(Some(m))
        }
        None => Ok(None),
    }
}

/// A page of messages, newest last. `before` is a `(sort_ms, row_id)` cursor.
pub fn messages(
    conn: &Connection,
    me: &Id16,
    chat_id: &[u8],
    before: Option<(i64, i64)>,
    limit: u32,
) -> CoreResult<Vec<Message>> {
    let (bs, br) = before.unwrap_or((i64::MAX, i64::MAX));
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages m LEFT JOIN attachments a USING (message_id)
         WHERE m.chat_id = ?1 AND (m.sort_ms < ?2 OR (m.sort_ms = ?2 AND m.row_id < ?3))
         ORDER BY m.sort_ms DESC, m.row_id DESC LIMIT ?4"
    ))?;
    let mut out: Vec<Message> = stmt
        .query_map(params![chat_id, bs, br, i64::from(limit)], message_from_row)?
        .collect::<Result<_, _>>()?;
    out.reverse();
    for m in &mut out {
        load_reactions(conn, me, m)?;
    }
    Ok(out)
}

/// The newest message in a chat (for the chat list preview).
pub fn last_message(conn: &Connection, me: &Id16, chat_id: &[u8]) -> CoreResult<Option<Message>> {
    Ok(messages(conn, me, chat_id, None, 1)?.pop())
}

/// Updates a message's delivery state; never moves backwards except to `Failed`.
pub fn set_status(conn: &Connection, message_id: &Id16, status: MessageStatus) -> CoreResult<bool> {
    let n = conn.execute(
        "UPDATE messages SET status = ?2 WHERE message_id = ?1 AND outgoing = 1
           AND (status < ?2 OR ?2 = 4 OR status = 4)",
        params![&message_id[..], status as i64],
    )?;
    Ok(n > 0)
}

/// Replaces a message's text after an authorized edit.
pub fn edit_message(conn: &Connection, message_id: &Id16, body: &str) -> CoreResult<()> {
    conn.execute(
        "UPDATE messages SET body = ?2, edited = 1 WHERE message_id = ?1 AND deleted = 0",
        params![&message_id[..], body],
    )?;
    Ok(())
}

/// Deletes a message for everyone: content, attachment and reactions are removed.
pub fn tombstone_message(conn: &Connection, message_id: &Id16) -> CoreResult<()> {
    conn.execute(
        "UPDATE messages SET body = '', deleted = 1 WHERE message_id = ?1",
        params![&message_id[..]],
    )?;
    conn.execute(
        "DELETE FROM attachments WHERE message_id = ?1",
        params![&message_id[..]],
    )?;
    conn.execute(
        "DELETE FROM reactions WHERE message_id = ?1",
        params![&message_id[..]],
    )?;
    Ok(())
}

/// Adds or removes a reaction.
pub fn set_reaction(
    conn: &Connection,
    message_id: &Id16,
    user: &Id16,
    emoji: &str,
    remove: bool,
) -> CoreResult<()> {
    if remove {
        conn.execute(
            "DELETE FROM reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
            params![&message_id[..], &user[..], emoji],
        )?;
    } else {
        conn.execute(
            "INSERT OR IGNORE INTO reactions(message_id, user_id, emoji) VALUES (?1, ?2, ?3)",
            params![&message_id[..], &user[..], emoji],
        )?;
    }
    Ok(())
}

/// Records a delivery or read receipt from `user`.
pub fn record_receipt(
    conn: &Connection,
    message_id: &Id16,
    user: &Id16,
    read: bool,
    at_ms: i64,
) -> CoreResult<()> {
    conn.execute(
        "INSERT INTO receipts(message_id, user_id, delivered_ms, read_ms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(message_id, user_id) DO UPDATE SET
           delivered_ms = COALESCE(receipts.delivered_ms, excluded.delivered_ms),
           read_ms = COALESCE(receipts.read_ms, excluded.read_ms)",
        params![&message_id[..], &user[..], at_ms, read.then_some(at_ms)],
    )?;
    Ok(())
}

/// How many recipients have delivered/read a message.
pub fn receipt_counts(conn: &Connection, message_id: &Id16) -> CoreResult<(u32, u32)> {
    Ok(conn.query_row(
        "SELECT COUNT(delivered_ms), COUNT(read_ms) FROM receipts WHERE message_id = ?1",
        params![&message_id[..]],
        |r| Ok((u32_col(r, 0)?, u32_col(r, 1)?)),
    )?)
}

/// Incoming messages in a chat at or before `up_to_ms` that we haven't sent read receipts for.
pub fn unread_incoming(
    conn: &Connection,
    chat_id: &[u8],
    up_to_ms: i64,
) -> CoreResult<Vec<(Id16, Id16)>> {
    let mut stmt = conn.prepare(
        "SELECT message_id, sender_user FROM messages
         WHERE chat_id = ?1 AND outgoing = 0 AND status = 10 AND kind IN (1, 2) AND sort_ms <= ?2",
    )?;
    let rows = stmt
        .query_map(params![chat_id, up_to_ms], |r| {
            Ok((blob16(r, 0)?, blob16(r, 1)?))
        })?
        .collect::<Result<_, _>>()?;
    conn.execute(
        "UPDATE messages SET status = 11 WHERE chat_id = ?1 AND outgoing = 0 AND status = 10 AND sort_ms <= ?2",
        params![chat_id, up_to_ms],
    )?;
    Ok(rows)
}

/// Deletes messages whose disappearing timer ran out; returns `(chat, message)` pairs.
pub fn purge_expired(conn: &Connection, now: i64) -> CoreResult<Vec<(Vec<u8>, Id16)>> {
    let mut stmt = conn.prepare(
        "SELECT chat_id, message_id FROM messages WHERE expires_at_ms IS NOT NULL AND expires_at_ms <= ?1",
    )?;
    let rows: Vec<(Vec<u8>, Id16)> = stmt
        .query_map(params![now], |r| Ok((r.get(0)?, blob16(r, 1)?)))?
        .collect::<Result<_, _>>()?;
    conn.execute(
        "DELETE FROM messages WHERE expires_at_ms IS NOT NULL AND expires_at_ms <= ?1",
        params![now],
    )?;
    Ok(rows)
}

/// Full-text search across all chats (FTS5 inside the encrypted database; never Spotlight).
pub fn search(conn: &Connection, query: &str, limit: u32) -> CoreResult<Vec<SearchHit>> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace('"', "")))
        .collect();
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let fts_query = terms.join(" ");
    let mut stmt = conn.prepare(
        "SELECT m.chat_id, m.message_id, snippet(message_fts, 0, '[', ']', '…', 12), m.sent_at_ms
         FROM message_fts JOIN messages m ON m.row_id = message_fts.rowid
         WHERE message_fts MATCH ?1 AND m.deleted = 0
         ORDER BY m.sort_ms DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![fts_query, i64::from(limit)], |r| {
            Ok(SearchHit {
                chat_id: r.get(0)?,
                message_id: blob16(r, 1)?,
                snippet: r.get(2)?,
                sent_at_ms: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

// ─── outbox ─────────────────────────────────────────────────────────────────

fn encode_ids(ids: &[Id16]) -> Vec<u8> {
    ids.iter().flat_map(|i| i.iter().copied()).collect()
}

fn decode_ids(bytes: &[u8]) -> Vec<Id16> {
    bytes.chunks_exact(16).filter_map(id16).collect()
}

/// Queues an envelope.
#[allow(clippy::too_many_arguments)]
pub fn enqueue(
    conn: &Connection,
    envelope_id: &Id16,
    chat_id: Option<&[u8]>,
    message_id: Option<&Id16>,
    kind: i32,
    recipients: &[Id16],
    payload: &[u8],
    ttl_s: u32,
) -> CoreResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO outbox(envelope_id, chat_id, message_id, kind, recipients, payload, ttl_s, created_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &envelope_id[..],
            chat_id,
            message_id.map(|m| &m[..]),
            kind,
            encode_ids(recipients),
            payload,
            i64::from(ttl_s),
            now_ms()
        ],
    )?;
    Ok(())
}

/// The next envelopes that are due, oldest first.
pub fn due_outbox(conn: &Connection, now: i64, limit: u32) -> CoreResult<Vec<OutboxItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, envelope_id, chat_id, message_id, kind, recipients, payload, ttl_s, attempts
         FROM outbox WHERE not_before_ms <= ?1 ORDER BY id LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, i64::from(limit)], |r| {
            let recipients: Vec<u8> = r.get(5)?;
            Ok(OutboxItem {
                id: r.get(0)?,
                envelope_id: blob16(r, 1)?,
                chat_id: r.get(2)?,
                message_id: opt_blob16(r, 3)?,
                kind: r.get(4)?,
                recipients: decode_ids(&recipients),
                payload: r.get(6)?,
                ttl_s: u32_col(r, 7)?,
                attempts: u32_col(r, 8)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Removes a sent envelope.
pub fn dequeue(conn: &Connection, id: i64) -> CoreResult<()> {
    conn.execute("DELETE FROM outbox WHERE id = ?1", params![id])?;
    Ok(())
}

/// Schedules a retry.
pub fn defer(conn: &Connection, id: i64, not_before_ms: i64) -> CoreResult<()> {
    conn.execute(
        "UPDATE outbox SET attempts = attempts + 1, not_before_ms = ?2 WHERE id = ?1",
        params![id, not_before_ms],
    )?;
    Ok(())
}

/// Replaces an envelope's recipients (after the relay reported unknown inboxes).
pub fn set_recipients(conn: &Connection, id: i64, recipients: &[Id16]) -> CoreResult<()> {
    conn.execute(
        "UPDATE outbox SET recipients = ?2 WHERE id = ?1",
        params![id, encode_ids(recipients)],
    )?;
    Ok(())
}

/// How many envelopes are waiting.
pub fn outbox_len(conn: &Connection) -> CoreResult<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM outbox", [], |r| u32_col(r, 0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_chat() -> (Store, Vec<u8>) {
        let store = Store::open_in_memory().unwrap();
        let chat_id = vec![1u8; 32];
        store
            .tx(|c| {
                insert_chat(
                    c,
                    &Chat {
                        chat_id: chat_id.clone(),
                        kind: ChatKind::Direct,
                        group_route: [2; 16],
                        name: String::new(),
                        description: String::new(),
                        avatar: None,
                        dm_user: Some([3; 16]),
                        suite: 0x4d,
                        created_ms: 1,
                        last_activity_ms: 1,
                        unread: 0,
                        muted: false,
                        pinned: false,
                        archived: false,
                        timer_s: 0,
                        left: false,
                    },
                )
            })
            .unwrap();
        (store, chat_id)
    }

    fn text<'a>(chat: &'a [u8], id: u8, body: &'a str, at: i64) -> NewMessage<'a> {
        NewMessage {
            message_id: [id; 16],
            chat_id: chat,
            sender_user: [3; 16],
            sender_device: [4; 16],
            outgoing: false,
            kind: MessageKind::Text,
            body,
            reply_to: None,
            sent_at_ms: at,
            received_at_ms: at,
            status: MessageStatus::Received,
            expires_at_ms: None,
        }
    }

    #[test]
    fn wrong_key_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        drop(Store::open(&path, &[1; 32]).unwrap());
        assert!(Store::open(&path, &[2; 32]).is_err());
        assert!(Store::open(&path, &[1; 32]).is_ok());
        // The file on disk must not contain the SQLite plaintext header.
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.starts_with(b"SQLite format 3"));
    }

    #[test]
    fn messages_page_search_and_dedupe() {
        let (store, chat) = store_with_chat();
        let me = [9u8; 16];
        store
            .tx(|c| {
                for i in 0..10u8 {
                    let body = format!("hello number {i}");
                    assert!(insert_message(
                        c,
                        &text(&chat, i, &body, 1000 + i64::from(i))
                    )?);
                }
                assert!(
                    !insert_message(c, &text(&chat, 0, "dup", 5))?,
                    "duplicate IDs ignored"
                );
                Ok(())
            })
            .unwrap();
        let conn = store.lock().unwrap();
        let page = messages(&conn, &me, &chat, None, 4).unwrap();
        assert_eq!(page.len(), 4);
        assert_eq!(page.last().unwrap().message_id, [9; 16]);
        let older = messages(
            &conn,
            &me,
            &chat,
            Some((page[0].sort_ms, page[0].row_id)),
            100,
        )
        .unwrap();
        assert_eq!(older.len(), 6);
        assert_eq!(search(&conn, "number 7", 10).unwrap().len(), 1);
        assert_eq!(search(&conn, "hel", 20).unwrap().len(), 10, "prefix search");
        edit_message(&conn, &[7; 16], "changed text").unwrap();
        assert_eq!(
            search(&conn, "number 7", 10).unwrap().len(),
            0,
            "FTS follows edits"
        );
        tombstone_message(&conn, &[8; 16]).unwrap();
        assert!(message(&conn, &me, &[8; 16]).unwrap().unwrap().deleted);
    }

    #[test]
    fn reactions_receipts_and_status() {
        let (store, chat) = store_with_chat();
        let me = [9u8; 16];
        let conn = store.lock().unwrap();
        let mut m = text(&chat, 1, "hi", 10);
        m.outgoing = true;
        m.status = MessageStatus::Pending;
        insert_message(&conn, &m).unwrap();
        set_reaction(&conn, &[1; 16], &me, "❤️", false).unwrap();
        set_reaction(&conn, &[1; 16], &[3; 16], "❤️", false).unwrap();
        set_reaction(&conn, &[1; 16], &[3; 16], "😂", false).unwrap();
        let msg = message(&conn, &me, &[1; 16]).unwrap().unwrap();
        assert_eq!(
            msg.reactions[0],
            ReactionSummary {
                emoji: "❤️".into(),
                count: 2,
                mine: true
            }
        );
        assert!(set_status(&conn, &[1; 16], MessageStatus::Delivered).unwrap());
        assert!(
            !set_status(&conn, &[1; 16], MessageStatus::Sent).unwrap(),
            "never moves backwards"
        );
        record_receipt(&conn, &[1; 16], &[3; 16], false, 5).unwrap();
        record_receipt(&conn, &[1; 16], &[3; 16], true, 6).unwrap();
        assert_eq!(receipt_counts(&conn, &[1; 16]).unwrap(), (1, 1));
    }

    #[test]
    fn gap_detection() {
        let (store, chat) = store_with_chat();
        let conn = store.lock().unwrap();
        let dev = [4u8; 16];
        assert!(observe_seq(&conn, &chat, &dev, 1, 0).unwrap().is_empty());
        assert_eq!(observe_seq(&conn, &chat, &dev, 4, 0).unwrap(), vec![2, 3]);
        assert!(
            observe_seq(&conn, &chat, &dev, 2, 0).unwrap().is_empty(),
            "late arrival fills the gap"
        );
        let overdue = take_overdue_gaps(&conn, 10).unwrap();
        assert_eq!(overdue, vec![(chat.clone(), dev, 1)]);
        assert!(
            take_overdue_gaps(&conn, 10).unwrap().is_empty(),
            "reported once"
        );
    }

    #[test]
    fn outbox_and_seqs() {
        let (store, chat) = store_with_chat();
        let conn = store.lock().unwrap();
        assert_eq!(next_my_seq(&conn, &chat).unwrap(), 1);
        assert_eq!(next_my_seq(&conn, &chat).unwrap(), 2);
        enqueue(
            &conn,
            &[1; 16],
            Some(&chat),
            None,
            1,
            &[[5; 16], [6; 16]],
            b"p",
            0,
        )
        .unwrap();
        let due = due_outbox(&conn, now_ms(), 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].recipients, vec![[5; 16], [6; 16]]);
        defer(&conn, due[0].id, i64::MAX).unwrap();
        assert!(due_outbox(&conn, now_ms(), 10).unwrap().is_empty());
        dequeue(&conn, due[0].id).unwrap();
        assert_eq!(outbox_len(&conn).unwrap(), 0);
        assert!(mark_envelope_processed(&conn, b"e1", 1).unwrap());
        assert!(
            !mark_envelope_processed(&conn, b"e1", 2).unwrap(),
            "replays are detected"
        );
    }
}
