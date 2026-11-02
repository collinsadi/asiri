//! App schema, versioned with `PRAGMA user_version`. MLS state lives in the same encrypted file,
//! in tables owned by `openmls_sqlite_storage` (architecture §6.6).

/// Each entry upgrades the schema by one version.
pub const MIGRATIONS: &[&str] = &[
    // v1
    r"
CREATE TABLE kv (
  key   TEXT PRIMARY KEY,
  value BLOB NOT NULL
);

CREATE TABLE contacts (
  user_id      BLOB PRIMARY KEY,
  display_name TEXT NOT NULL DEFAULT '',
  avatar       BLOB,
  verification INTEGER NOT NULL DEFAULT 0,
  blocked      INTEGER NOT NULL DEFAULT 0,
  created_ms   INTEGER NOT NULL
);

CREATE TABLE contact_devices (
  device_id     BLOB PRIMARY KEY,
  user_id       BLOB NOT NULL REFERENCES contacts(user_id) ON DELETE CASCADE,
  inbox_id      BLOB NOT NULL,
  signature_key BLOB NOT NULL,
  verified      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX contact_devices_user ON contact_devices(user_id);

CREATE TABLE chats (
  chat_id          BLOB PRIMARY KEY,
  kind             INTEGER NOT NULL,
  group_route      BLOB NOT NULL UNIQUE,
  name             TEXT NOT NULL DEFAULT '',
  description      TEXT NOT NULL DEFAULT '',
  avatar           BLOB,
  dm_user          BLOB,
  suite            INTEGER NOT NULL,
  created_ms       INTEGER NOT NULL,
  last_activity_ms INTEGER NOT NULL,
  unread           INTEGER NOT NULL DEFAULT 0,
  muted            INTEGER NOT NULL DEFAULT 0,
  pinned           INTEGER NOT NULL DEFAULT 0,
  archived         INTEGER NOT NULL DEFAULT 0,
  timer_s          INTEGER NOT NULL DEFAULT 0,
  left_chat        INTEGER NOT NULL DEFAULT 0,
  last_self_update_ms INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE messages (
  row_id         INTEGER PRIMARY KEY,
  message_id     BLOB NOT NULL UNIQUE,
  chat_id        BLOB NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
  sender_user    BLOB NOT NULL,
  sender_device  BLOB NOT NULL,
  outgoing       INTEGER NOT NULL,
  kind           INTEGER NOT NULL,
  body           TEXT NOT NULL DEFAULT '',
  reply_to       BLOB,
  sent_at_ms     INTEGER NOT NULL,
  received_at_ms INTEGER NOT NULL,
  sort_ms        INTEGER NOT NULL,
  status         INTEGER NOT NULL,
  edited         INTEGER NOT NULL DEFAULT 0,
  deleted        INTEGER NOT NULL DEFAULT 0,
  expires_at_ms  INTEGER
);
CREATE INDEX messages_chat_sort ON messages(chat_id, sort_ms, row_id);
CREATE INDEX messages_expiry ON messages(expires_at_ms) WHERE expires_at_ms IS NOT NULL;

CREATE VIRTUAL TABLE message_fts USING fts5(
  body, content='messages', content_rowid='row_id', tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages WHEN new.kind IN (1, 2) BEGIN
  INSERT INTO message_fts(rowid, body) VALUES (new.row_id, new.body);
END;
CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages WHEN old.kind IN (1, 2) BEGIN
  INSERT INTO message_fts(message_fts, rowid, body) VALUES ('delete', old.row_id, old.body);
END;
CREATE TRIGGER messages_fts_update AFTER UPDATE OF body ON messages WHEN old.kind IN (1, 2) BEGIN
  INSERT INTO message_fts(message_fts, rowid, body) VALUES ('delete', old.row_id, old.body);
  INSERT INTO message_fts(rowid, body) VALUES (new.row_id, new.body);
END;

CREATE TABLE attachments (
  message_id   BLOB PRIMARY KEY REFERENCES messages(message_id) ON DELETE CASCADE,
  blob_id      BLOB NOT NULL,
  key          BLOB NOT NULL,
  sha256       BLOB NOT NULL,
  size         INTEGER NOT NULL,
  blob_size    INTEGER NOT NULL,
  mime         TEXT NOT NULL,
  file_name    TEXT NOT NULL,
  width        INTEGER NOT NULL DEFAULT 0,
  height       INTEGER NOT NULL DEFAULT 0,
  duration_ms  INTEGER NOT NULL DEFAULT 0,
  thumbnail    BLOB,
  blurhash     TEXT NOT NULL DEFAULT '',
  voice_note   INTEGER NOT NULL DEFAULT 0,
  waveform     BLOB,
  state        INTEGER NOT NULL,
  progress     REAL NOT NULL DEFAULT 0
);

CREATE TABLE reactions (
  message_id BLOB NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
  user_id    BLOB NOT NULL,
  emoji      TEXT NOT NULL,
  PRIMARY KEY (message_id, user_id, emoji)
);

CREATE TABLE receipts (
  message_id   BLOB NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
  user_id      BLOB NOT NULL,
  delivered_ms INTEGER,
  read_ms      INTEGER,
  PRIMARY KEY (message_id, user_id)
);

CREATE TABLE outbox (
  id          INTEGER PRIMARY KEY,
  envelope_id BLOB NOT NULL UNIQUE,
  chat_id     BLOB,
  message_id  BLOB,
  kind        INTEGER NOT NULL,
  recipients  BLOB NOT NULL,
  payload     BLOB NOT NULL,
  plaintext   BLOB,
  ttl_s       INTEGER NOT NULL DEFAULT 0,
  attempts    INTEGER NOT NULL DEFAULT 0,
  not_before_ms INTEGER NOT NULL DEFAULT 0,
  created_ms  INTEGER NOT NULL
);

CREATE TABLE processed_envelopes (
  envelope_id BLOB PRIMARY KEY,
  at_ms       INTEGER NOT NULL
);

-- MLS messages from an epoch this device hasn't reached yet (or for a group it hasn't joined
-- yet), kept until it gets there (architecture §12, ordering).
CREATE TABLE deferred_envelopes (
  id          INTEGER PRIMARY KEY,
  group_id    BLOB NOT NULL,
  epoch       INTEGER NOT NULL,
  kind        INTEGER NOT NULL,
  payload     BLOB NOT NULL,
  received_ms INTEGER NOT NULL
);
CREATE INDEX deferred_envelopes_group ON deferred_envelopes(group_id, epoch, id);

CREATE TABLE my_seqs (
  chat_id BLOB PRIMARY KEY,
  next    INTEGER NOT NULL
);

CREATE TABLE sender_seqs (
  chat_id   BLOB NOT NULL,
  device_id BLOB NOT NULL,
  highest   INTEGER NOT NULL,
  PRIMARY KEY (chat_id, device_id)
);

CREATE TABLE missing_seqs (
  chat_id   BLOB NOT NULL,
  device_id BLOB NOT NULL,
  seq       INTEGER NOT NULL,
  since_ms  INTEGER NOT NULL,
  reported  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (chat_id, device_id, seq)
);

CREATE TABLE pending_invites (
  signature_key BLOB PRIMARY KEY,
  user_id       BLOB NOT NULL,
  display_name  TEXT NOT NULL,
  created_ms    INTEGER NOT NULL
);

CREATE TABLE calls (
  call_id    BLOB PRIMARY KEY,
  chat_id    BLOB NOT NULL,
  room_alias TEXT NOT NULL,
  video      INTEGER NOT NULL,
  outgoing   INTEGER NOT NULL,
  caller     BLOB NOT NULL,
  state      INTEGER NOT NULL,
  started_ms INTEGER NOT NULL,
  answered_ms INTEGER,
  ended_ms   INTEGER
);
",
];
