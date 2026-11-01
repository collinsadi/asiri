//! `asiri-relay`: a deliberately simple store-and-forward relay (architecture §9).
//!
//! It keeps a registry of devices known only by random IDs and public keys, a mailbox per device,
//! a key-package directory, one-commit-per-epoch ordering for each group, short-lived encrypted
//! file storage, and mints LiveKit tokens. It never sees plaintext, names, contact lists or IP
//! addresses (messaging traffic arrives through Tor), and it never writes logs to disk.

#![forbid(unsafe_code)]

pub mod config;
pub mod db;
pub mod error;
pub mod gc;
pub mod ids;

pub use config::Config;
