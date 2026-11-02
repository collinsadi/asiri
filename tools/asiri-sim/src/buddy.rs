//! `asiri-sim buddy`: a headless friend for trying Asiri with one Mac (architecture §18).
//!
//! It joins the server the way a new Mac does. It prints its pairing code; you add it in Asiri
//! with "Add a Friend in Person…" and paste Asiri's reply back. From then on it behaves like a
//! patient friend: it reads what you send, shows that it's typing, answers, reacts, downloads
//! your files and greets new groups. Nothing is faked: it has its own keys and encrypted
//! database and talks MLS through the relay, exactly like the app.

#![allow(clippy::print_stdout)]

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use asiri_core::ids::Id16;
use asiri_core::store::{ChatKind, Message, MessageKind};
use asiri_core::{Core, CoreConfig, CoreEvent, EventSink};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::PATIENCE;

/// Hands the core's events to the buddy's loop.
struct Forward(UnboundedSender<CoreEvent>);

impl EventSink for Forward {
    fn emit(&self, event: CoreEvent) {
        let _ = self.0.send(event);
    }
}

/// The database key: random for a throwaway buddy, kept next to its data with `--data` so it
/// can come back as the same person. (A development tool: the app keeps its key in the
/// Secure Enclave instead.)
fn database_key(dir: &Path, persist: bool) -> anyhow::Result<[u8; 32]> {
    let path = dir.join("buddy.key");
    if persist && let Ok(bytes) = std::fs::read(&path) {
        return bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("{} is damaged", path.display()));
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    if persist {
        std::fs::write(&path, key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(key)
}

async fn read_line(prompt: &str) -> anyhow::Result<String> {
    println!("{prompt}");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        match lines.next_line().await? {
            Some(line) if !line.trim().is_empty() => return Ok(line.trim().to_owned()),
            Some(_) => {}
            None => anyhow::bail!("no input"),
        }
    }
}

/// A buddy: its engine, and the events the engine emits.
pub struct Buddy {
    /// The engine (a real core with its own keys and database).
    pub core: Core,
    name: String,
    events: UnboundedReceiver<CoreEvent>,
}

impl Buddy {
    /// Opens (or creates) a buddy whose database lives in `dir`.
    pub fn open(name: &str, dir: &Path, key: &[u8; 32]) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let (tx, events) = unbounded_channel();
        let mut config = CoreConfig::new(dir);
        config.runtime = Some(tokio::runtime::Handle::current());
        let core = Core::open(config, key, Arc::new(Forward(tx)) as Arc<dyn EventSink>)?;
        if !core.profile()?.registered {
            core.set_profile(name, None)?;
        }
        Ok(Self {
            core,
            name: name.to_owned(),
            events,
        })
    }

    /// Answers until `stop` completes: greets every chat it hasn't spoken in yet, replies to
    /// texts, collects files and declines calls.
    pub async fn serve(mut self, stop: impl Future<Output = ()>) -> anyhow::Result<()> {
        let me = self.core.profile()?.user_id;
        let mut greeted = HashSet::new();
        greet_new_chats(&self.core, &self.name, &mut greeted).await?;
        tokio::pin!(stop);
        loop {
            tokio::select! {
                event = self.events.recv() => {
                    let Some(event) = event else { break };
                    if let Err(e) = handle(&self.core, &self.name, &me, &mut greeted, event).await {
                        eprintln!("({} hit a snag: {e})", self.name);
                    }
                }
                () = &mut stop => break,
            }
        }
        self.core.disconnect().await;
        Ok(())
    }
}

/// Runs a buddy from the command line until Ctrl-C, pairing it first if it's new.
pub async fn run(name: &str, data: Option<PathBuf>) -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let persist = data.is_some();
    let dir = data.unwrap_or_else(|| temp.path().join("buddy"));
    std::fs::create_dir_all(&dir)?;
    let key = database_key(&dir, persist)?;
    let buddy = Buddy::open(name, &dir, &key)?;
    let core = &buddy.core;

    if core.profile()?.registered {
        println!("{name} is back (data in {}).", dir.display());
    } else {
        let offer = core.device_offer_code()?;
        println!();
        println!(
            "1. In Asiri: “Add a Friend in Person…” → “New to Asiri”, and paste {name}'s code:"
        );
        println!();
        println!("{offer}");
        println!();
        let reply =
            read_line("2. Click “Copy code” in Asiri, paste it here and press Return:").await?;
        let step = core.redeem_invitation(&reply).await?;
        let emoji: Vec<String> = step
            .emoji
            .iter()
            .map(|e| format!("{} {}", e.emoji, e.name))
            .collect();
        println!();
        println!(
            "3. Asiri should show the same six emoji: {}",
            emoji.join("  ")
        );
        println!("   If they match, confirm in Asiri. {name} trusts you from here.");
        core.confirm_verification(&step.peer_user_id, true)?;
    }
    core.connect()?;
    core.wait_until_connected(PATIENCE).await?;
    println!();
    println!("{name} is online. Message them from Asiri; Ctrl-C to stop.");
    buddy
        .serve(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    println!();
    println!("{name} went offline.");
    Ok(())
}

/// Says hello in every chat the buddy hasn't written in yet.
async fn greet_new_chats(
    core: &Core,
    name: &str,
    greeted: &mut HashSet<Vec<u8>>,
) -> anyhow::Result<()> {
    for chat in core.chats()? {
        if !greeted.insert(chat.chat.chat_id.clone()) {
            continue;
        }
        let spoken = core
            .messages(&chat.chat.chat_id, None, 200)?
            .iter()
            .any(|m| m.outgoing && m.kind == MessageKind::Text);
        if spoken {
            continue;
        }
        let hello = match chat.chat.kind {
            ChatKind::Direct => format!(
                "Hi! I'm {name}, a test buddy. I answer everything you send, so try texts, \
                 replies, photos, files and voice notes. All of it is end-to-end encrypted."
            ),
            ChatKind::Group => format!("Hello, {}! 👋 {name} here.", chat.title),
        };
        pause(400).await;
        core.send_text(&chat.chat.chat_id, &hello, None)?;
        println!("→ [{}] {hello}", chat.title);
    }
    Ok(())
}

async fn handle(
    core: &Core,
    name: &str,
    me: &Id16,
    greeted: &mut HashSet<Vec<u8>>,
    event: CoreEvent,
) -> anyhow::Result<()> {
    match event {
        CoreEvent::ChatsChanged => greet_new_chats(core, name, greeted).await?,
        CoreEvent::MessageAdded {
            chat_id,
            message_id,
        } => {
            let message = core.message(&message_id)?;
            if message.outgoing || message.sender_user == *me {
                return Ok(());
            }
            match message.kind {
                MessageKind::Text => reply_to_text(core, &chat_id, &message).await?,
                MessageKind::Attachment => reply_to_file(core, &chat_id, &message).await?,
                _ => {}
            }
        }
        CoreEvent::IncomingCall(call) => {
            println!("☎ declining a call (a buddy has no camera or microphone)");
            core.decline_call(&call.call_id)?;
            core.send_text(
                &call.chat_id,
                "I'm a test buddy, so I can't take calls. Try calling a friend!",
                None,
            )?;
        }
        _ => {}
    }
    Ok(())
}

async fn reply_to_text(core: &Core, chat_id: &[u8], message: &Message) -> anyhow::Result<()> {
    println!("← {}", message.body);
    pause(500).await;
    core.mark_read(chat_id)?;
    let text = message.body.trim();
    let lower = text.to_lowercase();
    let answer = if ["hi", "hello", "hey", "yo"]
        .iter()
        .any(|w| lower.split_whitespace().next() == Some(w))
    {
        "Hey! 👋 Good to hear from you.".to_owned()
    } else if lower == "ping" {
        "pong 🏓".to_owned()
    } else if lower.ends_with('?') {
        "Good question! I'm only a test buddy, but I read it on my side of the encryption. 🔐"
            .to_owned()
    } else {
        core.react(chat_id, &message.message_id, "👍", false)?;
        format!("You said: “{text}”")
    };
    core.set_typing(chat_id, true)?;
    pause(400 + 25 * u64::try_from(answer.chars().count().min(80)).unwrap_or(80)).await;
    core.send_text(chat_id, &answer, Some(message.message_id))?;
    println!("→ {answer}");
    Ok(())
}

async fn reply_to_file(core: &Core, chat_id: &[u8], message: &Message) -> anyhow::Result<()> {
    let Some(file) = &message.attachment else {
        return Ok(());
    };
    println!("← {} ({} bytes)", file.file_name, file.size);
    core.mark_read(chat_id)?;
    core.download_attachment(&message.message_id).await?;
    let bytes = core.attachment_data(&message.message_id)?;
    anyhow::ensure!(
        bytes.len() as u64 == file.size,
        "the file arrived with the wrong size"
    );
    let what = if file.voice_note {
        "voice message".to_owned()
    } else if file.mime.starts_with("image/") {
        format!("photo ({}×{})", file.width, file.height)
    } else {
        format!("“{}”", file.file_name)
    };
    core.react(chat_id, &message.message_id, "❤️", false)?;
    let answer = format!(
        "Got your {what}: {} decrypted fine on my side.",
        human_size(file.size)
    );
    core.send_text(chat_id, &answer, Some(message.message_id))?;
    println!("→ {answer}");
    Ok(())
}

fn human_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if b >= 1_048_576.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

async fn pause(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
