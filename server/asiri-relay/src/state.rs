//! Shared relay state: database, rate limits, online sessions and HTTP session tokens.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::config::Config;
use crate::db::Db;
use crate::ids::{Id, random_bytes};
use crate::ratelimit::{Kind, RateLimiter};

/// How long an HTTP session token stays valid after it is issued.
const TOKEN_TTL: Duration = Duration::from_secs(2 * 3600);

/// A connected device's session.
#[derive(Clone)]
pub struct SessionEntry {
    /// Distinguishes a reconnect from the session it replaced.
    pub session_id: u64,
    /// The device's inbox.
    pub inbox_id: Id,
    /// Notified when new envelopes arrive in the inbox.
    pub wake: Arc<Notify>,
    /// Notified when the session must end (replaced or revoked).
    pub kick: Arc<Notify>,
}

/// What a bearer token grants.
#[derive(Clone, Copy, Debug)]
pub struct TokenEntry {
    /// The authenticated device.
    pub device_id: Id,
    /// Whether that device is an admin.
    pub is_admin: bool,
    expires: Instant,
}

/// State shared by every connection.
pub struct AppState {
    /// Configuration.
    pub config: Config,
    /// The database.
    pub db: Db,
    /// Per-device rate limits.
    pub limiter: RateLimiter,
    sessions: Mutex<HashMap<Id, SessionEntry>>,
    inbox_owner: Mutex<HashMap<Id, Id>>,
    tokens: Mutex<HashMap<[u8; 32], TokenEntry>>,
    next_session: AtomicU64,
}

impl AppState {
    /// Builds the state, wiring rate limits from the configuration.
    #[must_use]
    pub fn new(config: Config, db: Db) -> Arc<Self> {
        let l = &config.limits;
        let limiter = RateLimiter::new()
            .with_rule(Kind::Send, l.sends_per_minute, Duration::from_secs(60))
            .with_rule(
                Kind::KeyPackageFetch,
                l.key_package_fetches_per_hour,
                Duration::from_secs(3600),
            )
            .with_rule(
                Kind::Authorize,
                l.authorizations_per_day,
                Duration::from_secs(86_400),
            )
            .with_rule(Kind::BlobChunk, 2_000, Duration::from_secs(60))
            .with_rule(Kind::CallToken, 60, Duration::from_secs(60));
        Arc::new(Self {
            config,
            db,
            limiter,
            sessions: Mutex::new(HashMap::new()),
            inbox_owner: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
        })
    }

    /// Registers a new session for `device_id`, ending any previous one.
    pub fn open_session(&self, device_id: Id, inbox_id: Id) -> SessionEntry {
        let entry = SessionEntry {
            session_id: self.next_session.fetch_add(1, Ordering::Relaxed),
            inbox_id,
            wake: Arc::new(Notify::new()),
            kick: Arc::new(Notify::new()),
        };
        if let Ok(mut sessions) = self.sessions.lock()
            && let Some(old) = sessions.insert(device_id, entry.clone())
        {
            old.kick.notify_one();
        }
        if let Ok(mut owners) = self.inbox_owner.lock() {
            owners.insert(inbox_id, device_id);
        }
        entry
    }

    /// Forgets a session if it is still the current one for the device.
    pub fn close_session(&self, device_id: &Id, session_id: u64) {
        if let Ok(mut sessions) = self.sessions.lock()
            && sessions
                .get(device_id)
                .is_some_and(|s| s.session_id == session_id)
        {
            let removed = sessions.remove(device_id);
            if let (Some(s), Ok(mut owners)) = (removed, self.inbox_owner.lock()) {
                owners.remove(&s.inbox_id);
            }
        }
    }

    /// Whether a device currently has a live session.
    pub fn is_online(&self, device_id: &Id) -> bool {
        self.sessions
            .lock()
            .map(|s| s.contains_key(device_id))
            .unwrap_or(false)
    }

    /// Wakes the sessions owning these inboxes so they deliver new envelopes immediately.
    pub fn wake_inboxes(&self, inboxes: &[Id]) {
        let (Ok(owners), Ok(sessions)) = (self.inbox_owner.lock(), self.sessions.lock()) else {
            return;
        };
        for inbox in inboxes {
            if let Some(s) = owners.get(inbox).and_then(|d| sessions.get(d)) {
                s.wake.notify_one();
            }
        }
    }

    /// Ends a device's session and invalidates its tokens (revocation).
    pub fn kick(&self, device_id: &Id) {
        if let Ok(sessions) = self.sessions.lock()
            && let Some(s) = sessions.get(device_id)
        {
            s.kick.notify_one();
        }
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.retain(|_, t| t.device_id != *device_id);
        }
    }

    /// Issues a bearer token for HTTP calls made during a session.
    pub fn issue_token(&self, device_id: Id, is_admin: bool) -> [u8; 32] {
        let token: [u8; 32] = random_bytes();
        if let Ok(mut tokens) = self.tokens.lock() {
            let now = Instant::now();
            tokens.retain(|_, t| t.expires > now);
            tokens.insert(
                token,
                TokenEntry {
                    device_id,
                    is_admin,
                    expires: now + TOKEN_TTL,
                },
            );
        }
        token
    }

    /// Resolves a bearer token.
    pub fn token(&self, token: &[u8]) -> Option<TokenEntry> {
        let key: [u8; 32] = token.try_into().ok()?;
        let tokens = self.tokens.lock().ok()?;
        tokens
            .get(&key)
            .copied()
            .filter(|t| t.expires > Instant::now())
    }
}
