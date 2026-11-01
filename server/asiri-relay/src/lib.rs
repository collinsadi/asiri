//! `asiri-relay`: a deliberately simple store-and-forward relay (architecture §9).
//!
//! It keeps a registry of devices known only by random IDs and public keys, a mailbox per device,
//! a key-package directory, one-commit-per-epoch ordering for each group, short-lived encrypted
//! file storage, and mints LiveKit tokens. It never sees plaintext, names, contact lists or IP
//! addresses (messaging traffic arrives through Tor), and it never writes logs to disk.

#![forbid(unsafe_code)]

pub mod admin;
pub mod authorized;
pub mod config;
pub mod db;
pub mod error;
pub mod gc;
pub mod http_api;
pub mod ids;
pub mod livekit;
pub mod ratelimit;
pub mod server;
pub mod state;
pub mod ws;

pub use config::Config;
pub use server::{serve, spawn};
