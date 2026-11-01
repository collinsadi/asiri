//! `asiri-relay` command line (architecture §9.10).
//!
//! ```text
//! asiri-relay serve                                       # run the relay
//! asiri-relay admin bootstrap --onion-client-key <key>    # first device; prints a registration QR
//! asiri-relay devices list                                # IDs, state, admin flag
//! asiri-relay devices revoke <device_id>
//! asiri-relay policy set invite admins|members
//! asiri-relay gc                                          # apply the retention rules now
//! asiri-relay release publish --manifest latest.json [--file Asiri-1.2.0.dmg]
//! ```

#![allow(clippy::print_stdout)]

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context as _;
use asiri_proto::pairing::v1::relay_endpoint::Transport;
use asiri_relay::config::{Config, Limits, LiveKitKeys};
use asiri_relay::db::{self, Db, InvitePolicy};
use asiri_relay::{admin, authorized, gc, ids};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "asiri-relay", version, about = "Asiri's ciphertext-only relay")]
struct Cli {
    /// Database, encrypted files and releases.
    #[arg(
        long,
        env = "ASIRI_DATA_DIR",
        default_value = "/var/lib/asiri",
        global = true
    )]
    data_dir: PathBuf,
    /// Directory where desired onion client keys are published for asiri-authsync.
    #[arg(long, env = "ASIRI_AUTHORIZED_DIR", global = true)]
    authorized_dir: Option<PathBuf>,
    /// The address devices dial (e.g. "<56 chars>.onion"); bound into auth signatures.
    #[arg(
        long,
        env = "ASIRI_PUBLIC_ADDRESS",
        default_value = "127.0.0.1",
        global = true
    )]
    public_address: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the relay.
    Serve {
        /// Listen address; behind tor this stays on loopback.
        #[arg(long, env = "ASIRI_LISTEN", default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        /// LiveKit keys file (`APIKEY: secret`), used to mint call tokens.
        #[arg(long, env = "ASIRI_LIVEKIT_KEYS_FILE")]
        livekit_keys_file: Option<PathBuf>,
        /// Log filter (warnings and errors only by default; never logs identifiers).
        #[arg(long, env = "ASIRI_LOG", default_value = "warn")]
        log: String,
    },
    /// Administrative commands.
    Admin {
        #[command(subcommand)]
        cmd: AdminCmd,
    },
    /// Inspect or revoke devices.
    Devices {
        #[command(subcommand)]
        cmd: DevicesCmd,
    },
    /// Relay policy.
    Policy {
        #[command(subcommand)]
        cmd: PolicyCmd,
    },
    /// Apply the retention rules now.
    Gc,
    /// Publish a signed release for the in-app updater.
    Release {
        #[command(subcommand)]
        cmd: ReleaseCmd,
    },
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Create the first admin device and print its registration invitation.
    Bootstrap {
        /// The admin Mac's onion client key (shown under "Set up a server"). Omit for a relay
        /// reached without Tor (development only).
        #[arg(long)]
        onion_client_key: Option<String>,
        /// How devices reach this relay. Defaults to onion for `.onion` addresses.
        #[arg(long, value_enum)]
        transport: Option<TransportArg>,
        /// Address placed in the invitation (defaults to --public-address).
        #[arg(long)]
        address: Option<String>,
        /// Address for call signaling, if not the relay address with port 7880.
        #[arg(long)]
        call_address: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum TransportArg {
    Onion,
    Direct,
}

#[derive(Subcommand)]
enum DevicesCmd {
    /// List devices: ID, state, admin flag.
    List,
    /// Revoke a device; it can no longer reach the relay.
    Revoke { device_id: String },
}

#[derive(Subcommand)]
enum PolicyCmd {
    /// Set a policy value.
    Set {
        #[command(subcommand)]
        what: PolicySet,
    },
}

#[derive(Subcommand)]
enum PolicySet {
    /// Who may authorize new devices.
    Invite {
        #[arg(value_parser = ["admins", "members"])]
        policy: String,
    },
}

#[derive(Subcommand)]
enum ReleaseCmd {
    /// Copy a signed manifest (and optionally the installer) into the releases directory.
    Publish {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

fn config(cli: &Cli) -> Config {
    Config {
        listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
        data_dir: cli.data_dir.clone(),
        authorized_dir: cli.authorized_dir.clone(),
        public_address: cli.public_address.clone(),
        livekit: None,
        limits: Limits::default(),
    }
}

fn print_qr(text: &str) {
    match qrcode::QrCode::new(text.as_bytes()) {
        Ok(code) => {
            let image = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .dark_color(qrcode::render::unicode::Dense1x2::Light)
                .light_color(qrcode::render::unicode::Dense1x2::Dark)
                .quiet_zone(true)
                .build();
            println!("{image}");
        }
        Err(e) => println!("(could not render a QR code: {e})"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut cfg = config(&cli);
    match &cli.cmd {
        Cmd::Serve {
            listen,
            livekit_keys_file,
            log,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(log))
                .with_target(false)
                .init();
            cfg.listen = *listen;
            if let Some(path) = livekit_keys_file {
                cfg.livekit = Some(LiveKitKeys::from_file(path).context("reading LiveKit keys")?);
            }
            asiri_relay::serve(cfg).await
        }
        Cmd::Admin {
            cmd:
                AdminCmd::Bootstrap {
                    onion_client_key,
                    transport,
                    address,
                    call_address,
                },
        } => {
            let db = Db::open(&cfg.db_path())?;
            let key = onion_client_key
                .as_deref()
                .map(admin::parse_onion_client_key)
                .transpose()?;
            let address = address
                .clone()
                .unwrap_or_else(|| cfg.public_address.clone());
            let transport = match transport {
                Some(TransportArg::Onion) => Transport::Onion,
                Some(TransportArg::Direct) => Transport::Direct,
                None if address.ends_with(".onion") => Transport::Onion,
                None => Transport::Direct,
            };
            if transport == Transport::Onion && key.is_none() {
                anyhow::bail!("an onion relay needs --onion-client-key from the admin's Mac");
            }
            let invitation =
                admin::bootstrap(&db, &cfg, key, transport, &address, call_address.as_deref())?;
            let text = admin::invitation_text(invitation);
            println!("Scan this with Asiri on the admin's Mac (Set up a server → Scan),");
            println!("or paste the code below. It works once and expires in 24 hours.\n");
            print_qr(&text);
            println!("{text}");
            Ok(())
        }
        Cmd::Devices { cmd } => {
            let db = Db::open(&cfg.db_path())?;
            match cmd {
                DevicesCmd::List => {
                    let devices = db.call_blocking(|c| db::list_devices(c))?;
                    println!("{:<34} {:<8} admin", "device_id", "state");
                    for d in devices {
                        println!(
                            "{:<34} {:<8} {}",
                            hex::encode(d.device_id),
                            format!("{:?}", d.state).to_lowercase(),
                            if d.is_admin { "yes" } else { "" }
                        );
                    }
                    Ok(())
                }
                DevicesCmd::Revoke { device_id } => {
                    let id =
                        ids::parse_hex_id(device_id).context("device_id must be 32 hex chars")?;
                    db.call_blocking(|c| db::revoke_device(c, &id, ids::now()))?;
                    if let Some(dir) = &cfg.authorized_dir {
                        authorized::remove_key(dir, &id)?;
                    }
                    println!("revoked; a running relay drops the device on its next request");
                    Ok(())
                }
            }
        }
        Cmd::Policy {
            cmd: PolicyCmd::Set {
                what: PolicySet::Invite { policy },
            },
        } => {
            let db = Db::open(&cfg.db_path())?;
            let p = InvitePolicy::parse(policy).context("policy")?;
            db.call_blocking(|c| db::set_invite_policy(c, p))?;
            println!("invite policy: {}", p.as_str());
            Ok(())
        }
        Cmd::Gc => {
            let db = Db::open(&cfg.db_path())?;
            let blob_dir = cfg.blob_dir();
            let report = db.call_blocking(|c| gc::run(c, &blob_dir, ids::now()))?;
            println!("{report:?}");
            Ok(())
        }
        Cmd::Release {
            cmd: ReleaseCmd::Publish { manifest, file },
        } => {
            let dir = cfg.releases_dir();
            std::fs::create_dir_all(&dir)?;
            if let Some(f) = file {
                let name = f.file_name().context("installer file name")?;
                std::fs::copy(f, dir.join(name))?;
            }
            std::fs::copy(manifest, dir.join("latest.json"))?;
            println!("published to {}", dir.display());
            Ok(())
        }
    }
}
