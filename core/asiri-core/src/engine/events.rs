//! Events the core pushes to the app (architecture §6.4). The app turns them into
//! `AsyncStream`s consumed by `@MainActor` view models.

use crate::ids::Id16;

/// The relay connection, as the UI shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not paired with a server yet.
    NotRegistered,
    /// No connection; retrying with backoff.
    Offline,
    /// Starting Tor ("Connecting securely…").
    StartingTor,
    /// Dialing and authenticating.
    Connecting,
    /// Connected; delivering the backlog that was waiting.
    Syncing,
    /// Connected and up to date.
    Connected,
}

/// An incoming call invitation.
#[derive(Clone, Debug)]
pub struct CallInvite {
    /// Call ID.
    pub call_id: Id16,
    /// The chat it belongs to.
    pub chat_id: Vec<u8>,
    /// Who is calling.
    pub caller: Id16,
    /// Video or voice.
    pub video: bool,
}

/// Something the UI should react to.
#[derive(Clone, Debug)]
pub enum CoreEvent {
    /// The connection state changed.
    ConnectionChanged(ConnectionState),
    /// The chat list should be reloaded (new chat, ordering, unread counts).
    ChatsChanged,
    /// One chat's metadata changed.
    ChatUpdated(Vec<u8>),
    /// A message was added.
    MessageAdded {
        /// Chat.
        chat_id: Vec<u8>,
        /// Message.
        message_id: Id16,
    },
    /// A message changed (status, edit, reaction, attachment progress).
    MessageUpdated {
        /// Chat.
        chat_id: Vec<u8>,
        /// Message.
        message_id: Id16,
    },
    /// A message disappeared (timer) or was deleted locally.
    MessageRemoved {
        /// Chat.
        chat_id: Vec<u8>,
        /// Message.
        message_id: Id16,
    },
    /// Someone started or stopped typing. Never stored; expires after 10 s.
    Typing {
        /// Chat.
        chat_id: Vec<u8>,
        /// Who.
        user_id: Id16,
        /// Typing now.
        active: bool,
    },
    /// A new incoming message the app may notify about (respecting the privacy level).
    Notify {
        /// Chat.
        chat_id: Vec<u8>,
        /// Message.
        message_id: Id16,
        /// Sender.
        sender: Id16,
    },
    /// Contacts (names, photos, verification) changed.
    ContactsChanged,
    /// In-person pairing finished; the DM is ready.
    PairingCompleted {
        /// The DM.
        chat_id: Vec<u8>,
        /// The new contact.
        user_id: Id16,
    },
    /// Someone is calling.
    IncomingCall(CallInvite),
    /// A call changed state (accepted elsewhere, declined, ended).
    CallUpdated {
        /// Call.
        call_id: Id16,
    },
    /// The group's epoch changed during a call: switch to the new key (§7.8).
    CallKeyRotated {
        /// Call.
        call_id: Id16,
        /// Base64 key for LiveKit's key provider.
        key_base64: String,
        /// LiveKit key index.
        key_index: u32,
    },
    /// Something is wrong and the user should know (a rejected group change, a key change).
    SecurityWarning {
        /// Chat, if the warning is about one.
        chat_id: Option<Vec<u8>>,
        /// Plain-language explanation.
        text: String,
    },
    /// A signed update is available.
    UpdateAvailable {
        /// Version string.
        version: String,
        /// Release notes.
        notes: String,
        /// Whether this build is too old to keep using.
        required: bool,
    },
}

/// Receives events. Implementations must be cheap and must not call back into the core
/// synchronously.
pub trait EventSink: Send + Sync {
    /// Called for every event, from a background thread.
    fn emit(&self, event: CoreEvent);
}

/// Discards events (tests, tools).
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: CoreEvent) {}
}
