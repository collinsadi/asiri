//! The core engine: the high-level API the app (through `asiri-ffi`) and the simulator use.
//!
//! Concurrency rules:
//! * Synchronous units of work run inside `Store::tx`, which holds the single SQLCipher
//!   connection; MLS state and app data change in the same transaction.
//! * Lock order is always identity → store. Neither is held across an `.await`.
//! * Events are collected during a transaction and emitted after it commits, so a listener can
//!   never observe uncommitted state or re-enter the core while a lock is held.

mod calls;
mod connection;
pub mod events;
mod groups;
mod inbound;
mod maintenance;
mod media;
mod outbound;
mod pairing;
pub mod updater;
pub mod views;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

pub use events::{CallInvite, ConnectionState, CoreEvent, EventSink, NullSink};
pub use maintenance::LONG_SLEEP_S;
pub use media::{AttachmentInput, blurhash_decode, blurhash_encode};
pub use updater::{ReleaseManifest, UpdateInfo};
pub use views::{
    CallRecord, CallSession, CallState, ChatSummary, MemberView, PairingStep, Profile,
    ServerStatus, Settings,
};

use crate::crypto::sas;
use crate::error::{CoreError, CoreResult};
use crate::identity::Identity;
use crate::ids::Id16;
use crate::mls::{self, Mls};
use crate::net::Dialer;
use crate::net::relay::RelayClient;
use crate::store::{self, ChatKind, Contact, Message, SearchHit, Store, Verification};

const K_SETTINGS: &str = "settings.v1";

/// How the core is opened.
#[derive(Clone)]
pub struct CoreConfig {
    /// The app container's data directory (database, attachments, Tor state).
    pub data_dir: PathBuf,
    /// Run on this runtime instead of creating one (tests and the simulator).
    pub runtime: Option<Handle>,
    /// Ed25519 public key that release manifests must be signed with (§16).
    pub release_public_key: Option<[u8; 32]>,
    /// This app's build number (for the updater).
    pub app_build: u32,
}

impl CoreConfig {
    /// A configuration rooted at `data_dir`.
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            runtime: None,
            release_public_key: None,
            app_build: 0,
        }
    }
}

/// A call this device is in.
pub(crate) struct ActiveCall {
    pub chat_id: Vec<u8>,
    /// The epoch the current call key was derived from.
    pub epoch: u64,
    /// Kept open for the call's lifetime; dropping it closes the signaling bridge.
    pub _bridge: crate::net::bridge::Bridge,
}

pub(crate) struct Inner {
    pub config: CoreConfig,
    pub store: Store,
    pub mls: Mls,
    pub identity: RwLock<Identity>,
    pub events: Arc<dyn EventSink>,
    pub rt: Handle,
    owned_rt: Mutex<Option<Runtime>>,
    pub client: Mutex<Option<Arc<RelayClient>>>,
    pub dialer: Mutex<Option<Arc<dyn Dialer>>>,
    pub state: Mutex<ConnectionState>,
    pub outbox_wake: Notify,
    pub network_wake: Notify,
    pub shutdown: watch::Sender<bool>,
    pub net_task: Mutex<Option<JoinHandle<()>>>,
    pub settings: RwLock<Settings>,
    pub calls: Mutex<HashMap<Id16, ActiveCall>>,
    pub typing_sent: Mutex<HashMap<Vec<u8>, Instant>>,
    /// Delivery receipts waiting to be batched into one message per chat.
    pub receipts_due: Mutex<HashMap<Vec<u8>, Vec<Id16>>>,
    /// Serializes key-package top-ups (pairing and maintenance may race).
    pub key_package_lock: tokio::sync::Mutex<()>,
    #[cfg(feature = "tor")]
    pub tor: tokio::sync::OnceCell<Arc<crate::net::tor::Tor>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(rt) = self.owned_rt.get_mut().ok().and_then(Option::take) {
            // The last reference may be dropped on one of the runtime's own threads, where a
            // runtime can't be shut down; a short-lived thread can always do it.
            let _ = std::thread::Builder::new()
                .name("asiri-core-shutdown".into())
                .spawn(move || rt.shutdown_background());
        }
    }
}

/// The Asiri core. Cheap to clone; all clones share one engine.
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<Inner>,
}

fn lock_err<T>(_: T) -> CoreError {
    CoreError::internal("lock poisoned")
}

impl Core {
    /// Opens the encrypted store with the database master key and loads (or creates) the
    /// device identity (§11.1).
    pub fn open(
        config: CoreConfig,
        db_key: &[u8; 32],
        events: Arc<dyn EventSink>,
    ) -> CoreResult<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(config.data_dir.join("attachments"))?;
        let store = Store::open(&config.data_dir.join("asiri.db"), db_key)?;
        let mls = Mls::new()?;
        let (identity, settings) = {
            let conn = store.lock()?;
            let identity = Identity::load_or_create(&conn, &mls)?;
            let settings = store::kv_get(&conn, K_SETTINGS)?
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            (identity, settings)
        };
        let (rt, owned) = match &config.runtime {
            Some(h) => (h.clone(), None),
            None => {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("asiri-core")
                    .enable_all()
                    .build()?;
                (rt.handle().clone(), Some(rt))
            }
        };
        let registered = identity.registration.is_some();
        let (shutdown, _) = watch::channel(false);
        let core = Self {
            inner: Arc::new(Inner {
                config,
                store,
                mls,
                identity: RwLock::new(identity),
                events,
                rt,
                owned_rt: Mutex::new(owned),
                client: Mutex::new(None),
                dialer: Mutex::new(None),
                state: Mutex::new(if registered {
                    ConnectionState::Offline
                } else {
                    ConnectionState::NotRegistered
                }),
                outbox_wake: Notify::new(),
                network_wake: Notify::new(),
                shutdown,
                net_task: Mutex::new(None),
                settings: RwLock::new(settings),
                calls: Mutex::new(HashMap::new()),
                typing_sent: Mutex::new(HashMap::new()),
                receipts_due: Mutex::new(HashMap::new()),
                key_package_lock: tokio::sync::Mutex::new(()),
                #[cfg(feature = "tor")]
                tor: tokio::sync::OnceCell::new(),
            }),
        };
        maintenance::spawn_local(&core.inner);
        Ok(core)
    }

    /// The runtime the core's background work runs on.
    #[must_use]
    pub fn runtime(&self) -> Handle {
        self.inner.rt.clone()
    }

    pub(crate) fn emit(&self, event: CoreEvent) {
        self.inner.events.emit(event);
    }

    pub(crate) fn emit_all(&self, events: Vec<CoreEvent>) {
        for e in events {
            self.emit(e);
        }
    }

    pub(crate) fn me(&self) -> CoreResult<(Id16, Id16)> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        let reg = id.registered()?;
        Ok((id.user_id, reg.device_id))
    }

    pub(crate) fn user_id(&self) -> CoreResult<Id16> {
        Ok(self.inner.identity.read().map_err(lock_err)?.user_id)
    }

    pub(crate) fn data_dir(&self) -> &Path {
        &self.inner.config.data_dir
    }

    /// The local profile.
    pub fn profile(&self) -> CoreResult<Profile> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        Ok(Profile {
            user_id: id.user_id,
            display_name: id.display_name.clone(),
            avatar: id.avatar.clone(),
            fingerprint: sas::format_fingerprint(&id.fingerprint()),
            registered: id.registration.is_some(),
            is_admin: id.registration.as_ref().is_some_and(|r| r.is_admin),
            relay_address: id.registration.as_ref().map(|r| r.endpoint.address.clone()),
            direct_transport: id
                .registration
                .as_ref()
                .is_some_and(|r| r.endpoint.is_direct()),
            onion_client_key: id.onion_public_line(),
        })
    }

    /// Current preferences.
    pub fn settings(&self) -> CoreResult<Settings> {
        Ok(self.inner.settings.read().map_err(lock_err)?.clone())
    }

    /// Saves preferences.
    pub fn set_settings(&self, settings: Settings) -> CoreResult<()> {
        let bytes = serde_json::to_vec(&settings)?;
        self.inner
            .store
            .tx(|c| store::kv_set(c, K_SETTINGS, &bytes))?;
        *self.inner.settings.write().map_err(lock_err)? = settings;
        Ok(())
    }

    /// The connection state.
    pub fn connection_state(&self) -> ConnectionState {
        self.inner
            .state
            .lock()
            .map(|s| *s)
            .unwrap_or(ConnectionState::Offline)
    }

    // ─── read models ───────────────────────────────────────────────────────

    fn summarize(
        &self,
        conn: &rusqlite::Connection,
        me: &Id16,
        chat: store::Chat,
    ) -> CoreResult<ChatSummary> {
        let last_message = store::last_message(conn, me, &chat.chat_id)?;
        let last_sender_name = match &last_message {
            Some(m) if !m.outgoing && chat.kind == ChatKind::Group => {
                store::contact(conn, &m.sender_user)?.map(|c| c.display_name)
            }
            _ => None,
        };
        let (member_count, is_admin) = match self.inner.mls.load(conn, &chat.chat_id) {
            Ok(group) => (
                u32::try_from(
                    mls::members(&group)
                        .iter()
                        .map(|m| m.user_id)
                        .collect::<std::collections::HashSet<_>>()
                        .len(),
                )
                .unwrap_or(0),
                mls::admins(&group).contains(me),
            ),
            Err(_) => (0, false),
        };
        let (title, avatar, verification) = match (chat.kind, chat.dm_user) {
            (ChatKind::Direct, Some(user)) => {
                let c = store::contact(conn, &user)?;
                (
                    c.as_ref()
                        .map(|c| c.display_name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "Friend".into()),
                    c.as_ref().and_then(|c| c.avatar.clone()),
                    c.map(|c| c.verification),
                )
            }
            _ => (
                if chat.name.is_empty() {
                    "New group".into()
                } else {
                    chat.name.clone()
                },
                chat.avatar.clone(),
                None,
            ),
        };
        Ok(ChatSummary {
            chat,
            title,
            avatar,
            last_message,
            last_sender_name,
            verification,
            member_count,
            is_admin,
        })
    }

    /// The chat list: pinned first, then most recent.
    pub fn chats(&self) -> CoreResult<Vec<ChatSummary>> {
        let me = self.user_id()?;
        let conn = self.inner.store.lock()?;
        store::chats(&conn)?
            .into_iter()
            .map(|c| self.summarize(&conn, &me, c))
            .collect()
    }

    /// One chat.
    pub fn chat(&self, chat_id: &[u8]) -> CoreResult<ChatSummary> {
        let me = self.user_id()?;
        let conn = self.inner.store.lock()?;
        let chat = store::chat(&conn, chat_id)?.ok_or(CoreError::NotFound)?;
        self.summarize(&conn, &me, chat)
    }

    /// A page of messages (newest last). Pass the oldest loaded message to page backwards.
    pub fn messages(
        &self,
        chat_id: &[u8],
        before: Option<&Id16>,
        limit: u32,
    ) -> CoreResult<Vec<Message>> {
        let me = self.user_id()?;
        let conn = self.inner.store.lock()?;
        let cursor = match before {
            Some(id) => store::message(&conn, &me, id)?.map(|m| (m.sort_ms, m.row_id)),
            None => None,
        };
        store::messages(&conn, &me, chat_id, cursor, limit.min(500))
    }

    /// One message.
    pub fn message(&self, message_id: &Id16) -> CoreResult<Message> {
        let me = self.user_id()?;
        let conn = self.inner.store.lock()?;
        store::message(&conn, &me, message_id)?.ok_or(CoreError::NotFound)
    }

    /// Members of a chat.
    pub fn members(&self, chat_id: &[u8]) -> CoreResult<Vec<MemberView>> {
        let me = self.user_id()?;
        let conn = self.inner.store.lock()?;
        let group = self.inner.mls.load(&conn, chat_id)?;
        let admins = mls::admins(&group);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for m in mls::members(&group) {
            if !seen.insert(m.user_id) {
                continue;
            }
            let contact = if m.user_id == me {
                let id = self.inner.identity.read().map_err(lock_err)?;
                Contact {
                    user_id: me,
                    display_name: id.display_name.clone(),
                    avatar: id.avatar.clone(),
                    verification: Verification::InPerson,
                    blocked: false,
                }
            } else {
                store::contact(&conn, &m.user_id)?.unwrap_or(Contact {
                    user_id: m.user_id,
                    display_name: String::new(),
                    avatar: None,
                    verification: Verification::Unverified,
                    blocked: false,
                })
            };
            out.push(MemberView {
                is_admin: admins.contains(&m.user_id),
                is_me: m.user_id == me,
                contact,
            });
        }
        out.sort_by(|a, b| {
            b.is_me
                .cmp(&a.is_me)
                .then(a.contact.display_name.cmp(&b.contact.display_name))
        });
        Ok(out)
    }

    /// Every contact.
    pub fn contacts(&self) -> CoreResult<Vec<Contact>> {
        let conn = self.inner.store.lock()?;
        store::contacts(&conn)
    }

    /// Full-text search across chats (inside the encrypted database).
    pub fn search(&self, query: &str, limit: u32) -> CoreResult<Vec<SearchHit>> {
        let conn = self.inner.store.lock()?;
        store::search(&conn, query, limit.min(200))
    }

    /// Mutes, pins or archives a chat.
    pub fn set_chat_flag(&self, chat_id: &[u8], flag: ChatFlag, value: bool) -> CoreResult<()> {
        let column = match flag {
            ChatFlag::Muted => "muted",
            ChatFlag::Pinned => "pinned",
            ChatFlag::Archived => "archived",
        };
        self.inner
            .store
            .tx(|c| store::set_chat_flag(c, chat_id, column, value))?;
        self.emit(CoreEvent::ChatsChanged);
        Ok(())
    }

    /// Blocks or unblocks a contact: their messages are dropped while blocked.
    pub fn set_blocked(&self, user_id: &Id16, blocked: bool) -> CoreResult<()> {
        self.inner
            .store
            .tx(|c| store::set_blocked(c, user_id, blocked))?;
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    /// Deletes a chat and its history from this Mac (the group itself is unaffected).
    pub fn delete_chat_locally(&self, chat_id: &[u8]) -> CoreResult<()> {
        self.inner.store.tx(|c| store::delete_chat(c, chat_id))?;
        self.emit(CoreEvent::ChatsChanged);
        Ok(())
    }

    /// How many envelopes are waiting to be sent.
    pub fn pending_outgoing(&self) -> CoreResult<u32> {
        let conn = self.inner.store.lock()?;
        store::outbox_len(&conn)
    }

    /// Crypto-shreds everything this core stored: closes the session, then deletes the
    /// database, attachments and Tor state (§6.6 "Panic wipe"). The app destroys the Secure
    /// Enclave key first, which already makes the database unreadable.
    pub async fn panic_wipe(&self) -> CoreResult<()> {
        self.disconnect().await;
        let dir = self.inner.config.data_dir.clone();
        for name in ["asiri.db", "asiri.db-wal", "asiri.db-shm"] {
            let _ = std::fs::remove_file(dir.join(name));
        }
        let _ = std::fs::remove_dir_all(dir.join("attachments"));
        let _ = std::fs::remove_dir_all(dir.join("tor"));
        Ok(())
    }
}

/// Chat flags the UI toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatFlag {
    /// No notifications.
    Muted,
    /// Pinned to the top.
    Pinned,
    /// Archived.
    Archived,
}
