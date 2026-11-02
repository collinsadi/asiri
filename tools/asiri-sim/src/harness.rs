//! One relay, many headless clients: the end-to-end test harness (architecture §18).
//!
//! Every client is a real [`Core`] with its own encrypted database, talking to a real relay
//! over real sockets through the direct transport. Nothing is mocked.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asiri_core::ids::Id16;
use asiri_core::store::{ChatKind, Message, MessageKind};
use asiri_core::{Core, CoreConfig, CoreEvent, EventSink};
use asiri_proto::pairing::v1::relay_endpoint::Transport;
use asiri_relay::config::{Config, LiveKitKeys};
use asiri_relay::state::AppState;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::evil::Proxy;

/// How long the helpers wait for something to happen across the network.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// LiveKit API key the simulated relay signs call tokens with. A LiveKit server started with
/// the same key pair (as CI does) accepts them.
pub const LIVEKIT_KEY: &str = "APIsimulator";
/// LiveKit API secret matching [`LIVEKIT_KEY`].
pub const LIVEKIT_SECRET: &str = "simulator-secret-0123456789abcdefghijklmnop";

/// Records every event a client's core emits.
#[derive(Default)]
pub struct Recorder {
    events: Mutex<Vec<CoreEvent>>,
}

impl EventSink for Recorder {
    fn emit(&self, event: CoreEvent) {
        if let Ok(mut e) = self.events.lock() {
            e.push(event);
        }
    }
}

impl Recorder {
    /// Every event so far.
    #[must_use]
    pub fn all(&self) -> Vec<CoreEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// How many events match.
    pub fn count(&self, f: impl Fn(&CoreEvent) -> bool) -> usize {
        self.events
            .lock()
            .map(|e| e.iter().filter(|x| f(x)).count())
            .unwrap_or(0)
    }

    /// Forgets recorded events.
    pub fn clear(&self) {
        if let Ok(mut e) = self.events.lock() {
            e.clear();
        }
    }
}

/// Polls `check` until it holds, failing with `what` after `timeout`.
pub async fn eventually(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> bool,
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if check() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("timed out after {timeout:?} waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).expect("system randomness");
    k
}

/// A headless Asiri client.
pub struct Client {
    /// Display name.
    pub name: String,
    /// The engine.
    pub core: Core,
    /// Everything it emitted.
    pub events: Arc<Recorder>,
    data_dir: PathBuf,
    db_key: [u8; 32],
}

impl Client {
    fn config(data_dir: &std::path::Path) -> CoreConfig {
        let mut config = CoreConfig::new(data_dir);
        config.runtime = Some(tokio::runtime::Handle::current());
        config.gap_grace_ms = 1_000;
        config
    }

    fn open(name: &str, data_dir: PathBuf) -> anyhow::Result<Self> {
        let db_key = random_key();
        let events = Arc::new(Recorder::default());
        let core = Core::open(
            Self::config(&data_dir),
            &db_key,
            Arc::clone(&events) as Arc<dyn EventSink>,
        )?;
        core.set_profile(name, None)?;
        Ok(Self {
            name: name.to_owned(),
            core,
            events,
            data_dir,
            db_key,
        })
    }

    /// This person's user ID.
    #[must_use]
    pub fn user_id(&self) -> Id16 {
        self.core.profile().map(|p| p.user_id).unwrap_or_default()
    }

    /// This device's relay inbox.
    #[must_use]
    pub fn inbox_id(&self) -> Id16 {
        self.core
            .profile()
            .ok()
            .and_then(|p| p.inbox_id)
            .unwrap_or_default()
    }

    /// The DM with `other`, if it exists.
    #[must_use]
    pub fn dm_with(&self, other: &Self) -> Option<Vec<u8>> {
        let peer = other.user_id();
        self.core
            .chats()
            .ok()?
            .into_iter()
            .find(|c| c.chat.kind == ChatKind::Direct && c.chat.dm_user == Some(peer))
            .map(|c| c.chat.chat_id)
    }

    /// Every message in a chat, oldest first.
    #[must_use]
    pub fn messages(&self, chat_id: &[u8]) -> Vec<Message> {
        self.core.messages(chat_id, None, 500).unwrap_or_default()
    }

    /// The bodies of text and attachment messages in a chat (no system lines), oldest first.
    #[must_use]
    pub fn texts(&self, chat_id: &[u8]) -> Vec<String> {
        self.messages(chat_id)
            .into_iter()
            .filter(|m| matches!(m.kind, MessageKind::Text | MessageKind::Attachment) && !m.deleted)
            .map(|m| m.body)
            .collect()
    }

    /// The system lines in a chat, oldest first.
    #[must_use]
    pub fn system_lines(&self, chat_id: &[u8]) -> Vec<String> {
        self.messages(chat_id)
            .into_iter()
            .filter(|m| m.kind == MessageKind::System)
            .map(|m| m.body)
            .collect()
    }

    /// Waits until a chat shows `text` from someone else.
    pub async fn wait_for_text(&self, chat_id: &[u8], text: &str) -> anyhow::Result<()> {
        eventually(
            &format!("{} to see “{text}”", self.name),
            PATIENCE,
            || self.texts(chat_id).iter().any(|t| t == text),
        )
        .await
    }

    /// Simulates quitting and relaunching the app: the engine closes and reopens from disk.
    pub async fn restart(&mut self) -> anyhow::Result<()> {
        self.core.disconnect().await;
        let events = Arc::new(Recorder::default());
        let core = Core::open(
            Self::config(&self.data_dir),
            &self.db_key,
            Arc::clone(&events) as Arc<dyn EventSink>,
        )?;
        self.core = core;
        self.events = events;
        if self.core.profile()?.registered {
            self.core.connect()?;
            self.core.wait_until_connected(PATIENCE).await?;
        }
        Ok(())
    }
}

/// Options for [`Harness::start_with`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Put the malicious-relay proxy between the clients and the relay.
    pub evil: bool,
}

/// A relay plus the clients that use it.
pub struct Harness {
    /// Where clients dial: the relay, or the evil proxy in front of it.
    pub address: SocketAddr,
    /// The relay's state (for inspecting what it stores).
    pub relay: Arc<AppState>,
    /// The malicious-relay proxy, when enabled.
    pub evil: Option<Proxy>,
    dir: tempfile::TempDir,
    clients: AtomicU32,
    task: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Harness {
    /// An honest relay.
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with(Options::default()).await
    }

    /// A relay with the malicious-relay proxy in front of it.
    pub async fn start_evil() -> anyhow::Result<Self> {
        Self::start_with(Options { evil: true }).await
    }

    /// A relay with the given options.
    pub async fn start_with(options: Options) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let relay_addr = listener.local_addr()?;
        let evil = if options.evil {
            Some(Proxy::start(relay_addr).await?)
        } else {
            None
        };
        let address = evil.as_ref().map_or(relay_addr, |p| p.addr);
        let mut config = Config::for_dev(dir.path().join("relay"));
        config.listen = relay_addr;
        config.public_address = address.to_string();
        config.livekit = Some(LiveKitKeys {
            api_key: LIVEKIT_KEY.into(),
            api_secret: LIVEKIT_SECRET.into(),
        });
        let (_, relay, task) = asiri_relay::spawn_on(listener, config).await?;
        Ok(Self {
            address,
            relay,
            evil,
            dir,
            clients: AtomicU32::new(0),
            task,
        })
    }

    /// The malicious-relay proxy (panics if the harness was started without it).
    #[must_use]
    pub fn evil(&self) -> &Proxy {
        self.evil
            .as_ref()
            .expect("start the harness with Harness::start_evil()")
    }

    /// The code the first admin scans (what `asiri-relay admin bootstrap` prints).
    pub fn bootstrap_code(&self) -> anyhow::Result<String> {
        let invitation = asiri_relay::admin::bootstrap(
            &self.relay.db,
            &self.relay.config,
            None,
            Transport::Direct,
            &self.address.to_string(),
            None,
        )?;
        Ok(asiri_relay::admin::invitation_text(invitation))
    }

    /// A Mac that just installed Asiri: keys exist, nothing is registered yet.
    pub fn new_client(&self, name: &str) -> anyhow::Result<Client> {
        let n = self.clients.fetch_add(1, Ordering::Relaxed);
        let data_dir = self
            .dir
            .path()
            .join(format!("client-{n}-{}", name.to_lowercase()));
        Client::open(name, data_dir)
    }

    /// The relay's first admin, registered with the bootstrap code.
    pub async fn admin(&self, name: &str) -> anyhow::Result<Client> {
        let client = self.new_client(name)?;
        client
            .core
            .redeem_invitation(&self.bootstrap_code()?)
            .await?;
        Ok(client)
    }

    /// Someone `inviter` pairs with in person. Returns once both have the DM and the six
    /// emoji matched.
    pub async fn invite(&self, inviter: &Client, name: &str) -> anyhow::Result<Client> {
        let joiner = self.new_client(name)?;
        let offer = joiner.core.device_offer_code()?;
        let step_a = inviter.core.invite_device(&offer).await?;
        let reply = step_a
            .reply_code
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no invitation code"))?;
        let step_b = joiner.core.redeem_invitation(&reply).await?;
        anyhow::ensure!(
            step_a.emoji == step_b.emoji,
            "the two screens show different emoji"
        );
        anyhow::ensure!(
            step_a.peer_user_id == joiner.user_id(),
            "the inviter saw the wrong person"
        );
        anyhow::ensure!(
            step_b.peer_user_id == inviter.user_id(),
            "the joiner saw the wrong person"
        );
        eventually(
            &format!("{} to get the DM from {name}", inviter.name),
            PATIENCE,
            || inviter.dm_with(&joiner).is_some(),
        )
        .await?;
        Ok(joiner)
    }
}
