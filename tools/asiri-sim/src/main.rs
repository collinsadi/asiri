//! `asiri-sim`: a local development relay and a scripted end-to-end demo.
//!
//! ```text
//! asiri-sim relay [--listen 127.0.0.1:8080]   run a relay without Tor and print the admin code
//! asiri-sim demo                              pair three people, chat in a DM and a group
//! ```

#![allow(clippy::print_stdout)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use asiri_core::store::ChatKind;
use asiri_proto::pairing::v1::relay_endpoint::Transport;
use asiri_relay::config::{Config, LiveKitKeys};
use asiri_sim::{Harness, PATIENCE, eventually};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "asiri-sim",
    version,
    about = "Asiri simulator and development relay"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a development relay reached without Tor (docs/adr/0003) and print the code the
    /// first admin pastes into Asiri.
    Relay {
        /// Address to listen on; also the address the app dials.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        /// Where the relay keeps its data (a temporary directory by default).
        #[arg(long)]
        data: Option<PathBuf>,
        /// LiveKit signaling address for calls (`host:port`), when a LiveKit server runs.
        #[arg(long)]
        call_address: Option<String>,
        /// LiveKit API key and secret as `key:secret`.
        #[arg(long, env = "ASIRI_LIVEKIT_KEYS")]
        livekit: Option<String>,
    },
    /// Pair three people, then chat in a DM and a group, printing what each of them sees.
    Demo,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    match Cli::parse().command {
        Command::Relay {
            listen,
            data,
            call_address,
            livekit,
        } => relay(listen, data, call_address, livekit).await,
        Command::Demo => demo().await,
    }
}

async fn relay(
    listen: SocketAddr,
    data: Option<PathBuf>,
    call_address: Option<String>,
    livekit: Option<String>,
) -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let dir = data.unwrap_or_else(|| temp.path().to_path_buf());
    let mut config = Config::for_dev(dir.clone());
    config.listen = listen;
    config.public_address = listen.to_string();
    if let Some(keys) = livekit {
        let (api_key, api_secret) = keys
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("--livekit takes key:secret"))?;
        config.livekit = Some(LiveKitKeys {
            api_key: api_key.to_owned(),
            api_secret: api_secret.to_owned(),
        });
    }
    let (addr, state, task) = asiri_relay::spawn(config).await?;
    let invitation = asiri_relay::admin::bootstrap(
        &state.db,
        &state.config,
        None,
        Transport::Direct,
        &addr.to_string(),
        call_address.as_deref(),
    )?;
    println!("Asiri development relay listening on {addr} (no Tor: development only)");
    println!("Data: {}", dir.display());
    println!();
    println!(
        "First admin: in Asiri choose “Set up a server”, then paste this under “Enter the server's code”:"
    );
    println!();
    println!("{}", asiri_relay::admin::invitation_text(invitation));
    println!();
    println!("The code works once and expires in 24 hours. Press Ctrl-C to stop.");
    tokio::select! {
        _ = task => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

async fn demo() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    println!("relay on {}", h.address);
    let ada = h.admin("Ada").await?;
    println!("Ada registered as the server's admin");
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    println!("Ada paired in person with Ben and Cam");

    let dm = ben.dm_with(&ada).ok_or_else(|| anyhow::anyhow!("no DM"))?;
    ben.core.send_text(&dm, "Hi Ada! Coffee at 10?", None)?;
    ada.wait_for_text(&dm, "Hi Ada! Coffee at 10?").await?;
    ada.core.send_text(&dm, "Perfect, see you there", None)?;
    ben.wait_for_text(&dm, "Perfect, see you there").await?;

    let group = ada
        .core
        .create_chat(
            ChatKind::Group,
            "Weekend trip",
            &[ben.user_id(), cam.user_id()],
        )
        .await?;
    eventually("everyone to join the group", PATIENCE, || {
        ben.core.chat(&group).is_ok() && cam.core.chat(&group).is_ok()
    })
    .await?;
    cam.core.send_text(&group, "I'll bring the tent", None)?;
    ben.wait_for_text(&group, "I'll bring the tent").await?;
    ada.wait_for_text(&group, "I'll bring the tent").await?;
    tokio::time::sleep(Duration::from_millis(300)).await;

    for who in [&ada, &ben, &cam] {
        println!();
        println!("── what {} sees ──", who.name);
        for chat in who.core.chats()? {
            println!("  [{}]", chat.title);
            for m in who.messages(&chat.chat.chat_id) {
                let from = if m.outgoing {
                    "me".to_owned()
                } else {
                    format!("{:?}", m.kind).to_lowercase()
                };
                println!("    {from:>8}: {}  ({:?})", m.body, m.status);
            }
        }
    }
    Ok(())
}
