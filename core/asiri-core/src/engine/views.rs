//! Read models handed to the app. They are built from the encrypted database only; nothing is
//! fetched to build them (§13.2).

use crate::crypto::sas::EmojiWord;
use crate::ids::Id16;
use crate::store::{Chat, Contact, Message, Verification};

/// A chat as the chat list shows it.
#[derive(Clone, Debug)]
pub struct ChatSummary {
    /// The chat record.
    pub chat: Chat,
    /// Display title: the group name, or the other person's name.
    pub title: String,
    /// Photo: the group's, or the other person's.
    pub avatar: Option<Vec<u8>>,
    /// The newest message, for the preview.
    pub last_message: Option<Message>,
    /// The preview's sender name (groups).
    pub last_sender_name: Option<String>,
    /// For DMs: the other person's verification state.
    pub verification: Option<Verification>,
    /// Member count (including us).
    pub member_count: u32,
    /// Whether we are an admin of the chat.
    pub is_admin: bool,
}

/// A chat member.
#[derive(Clone, Debug)]
pub struct MemberView {
    /// The person.
    pub contact: Contact,
    /// Whether they are an admin.
    pub is_admin: bool,
    /// Whether this is the local user.
    pub is_me: bool,
}

/// The local profile.
#[derive(Clone, Debug)]
pub struct Profile {
    /// User ID.
    pub user_id: Id16,
    /// Display name.
    pub display_name: String,
    /// Photo.
    pub avatar: Option<Vec<u8>>,
    /// Signature-key fingerprint (formatted).
    pub fingerprint: String,
    /// Whether the device is registered with a relay.
    pub registered: bool,
    /// Whether this device administers the relay.
    pub is_admin: bool,
    /// The relay address (for Settings → Network).
    pub relay_address: Option<String>,
    /// Whether the relay is reached without Tor (development only).
    pub direct_transport: bool,
    /// The onion client key for "Set up a server".
    pub onion_client_key: String,
}

/// The result of scanning someone's code in person.
#[derive(Clone, Debug)]
pub struct PairingStep {
    /// The six emoji both screens must show.
    pub emoji: Vec<EmojiWord>,
    /// The other person's name, as their code says.
    pub peer_name: String,
    /// The other person's user ID.
    pub peer_user_id: Id16,
    /// The code to show back (QR-B), when this step produces one.
    pub reply_code: Option<String>,
    /// The DM, once it exists.
    pub chat_id: Option<Vec<u8>>,
}

/// Everything the call window needs (§10.5).
#[derive(Clone, Debug)]
pub struct CallSession {
    /// Call ID.
    pub call_id: Id16,
    /// Chat.
    pub chat_id: Vec<u8>,
    /// Random room alias.
    pub room_alias: String,
    /// LiveKit URL: the loopback signaling bridge.
    pub url: String,
    /// LiveKit access token.
    pub token: String,
    /// Our random participant alias.
    pub participant_alias: String,
    /// Shared E2EE key, base64 (only crosses into Swift at the LiveKit boundary, §14.1).
    pub key_base64: String,
    /// LiveKit key index for `key_base64`.
    pub key_index: u32,
    /// Four emoji derived from the call key.
    pub emoji: Vec<EmojiWord>,
    /// Video call.
    pub video: bool,
}

/// Where a call stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallState {
    /// Ringing; nobody has answered yet.
    Ringing,
    /// Someone answered.
    Active,
    /// Over (ended, declined, cancelled or missed).
    Ended,
}

/// A call in the history, with its live state.
#[derive(Clone, Debug)]
pub struct CallRecord {
    /// Call ID.
    pub call_id: Id16,
    /// Chat.
    pub chat_id: Vec<u8>,
    /// Video or voice.
    pub video: bool,
    /// We started it.
    pub outgoing: bool,
    /// Who started it.
    pub caller: Id16,
    /// Where it stands.
    pub state: CallState,
    /// When it started ringing.
    pub started_ms: i64,
    /// When someone answered.
    pub answered_ms: Option<i64>,
    /// When it ended.
    pub ended_ms: Option<i64>,
    /// This device is in the call right now.
    pub joined: bool,
}

/// Relay devices, for admins (Settings → Server).
#[derive(Clone, Debug)]
pub struct ServerStatus {
    /// `(device id, state, admin)`; state 0 pending, 1 active, 2 revoked.
    pub devices: Vec<(Id16, u32, bool)>,
    /// "admins" or "members".
    pub invite_policy: String,
    /// Stored encrypted-file bytes.
    pub stored_blob_bytes: u64,
    /// Envelopes waiting in inboxes.
    pub queued_envelopes: u64,
}

/// Preferences (Settings → Privacy, §13.3).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Send read receipts (on by default, §13.1).
    pub read_receipts: bool,
    /// Send typing indicators.
    pub typing_indicators: bool,
    /// Generate link previews on send (off by default).
    pub link_previews: bool,
    /// Default disappearing timer for new chats, seconds (0 = off).
    pub default_timer_s: u32,
    /// Download images automatically.
    pub auto_download_images: bool,
    /// Largest file downloaded automatically.
    pub auto_download_max_bytes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            read_receipts: true,
            typing_indicators: true,
            link_previews: false,
            default_timer_s: 0,
            auto_download_images: true,
            auto_download_max_bytes: 25 * 1024 * 1024,
        }
    }
}
