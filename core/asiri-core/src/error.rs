//! Errors surfaced by the core. Messages are written for people ("You're offline"), never leak
//! key material, and never fall back silently to weaker behaviour (architecture §7.11).

use std::fmt;

/// Every fallible core operation returns this.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// No connection to the relay right now.
    #[error("You're offline. Asiri will retry when the connection is back.")]
    Offline,
    /// The relay refused or failed a request.
    #[error("The server refused the request: {0}")]
    Relay(String),
    /// A pairing code could not be read or used.
    #[error("{0}")]
    Pairing(String),
    /// The input is invalid (for example an empty message or an unknown chat).
    #[error("{0}")]
    Invalid(String),
    /// The object does not exist.
    #[error("Not found")]
    NotFound,
    /// The device is not registered with a relay yet.
    #[error("This Mac isn't connected to a server yet")]
    NotRegistered,
    /// The local user is not allowed to do this (for example, only admins may add members).
    #[error("{0}")]
    NotAllowed(String),
    /// Cryptographic failure (decryption, signature, MLS). Always loud.
    #[error("Security check failed: {0}")]
    Crypto(String),
    /// Encrypted storage failure.
    #[error("Storage error: {0}")]
    Storage(String),
    /// Network or I/O failure.
    #[error("Network error: {0}")]
    Network(String),
    /// A file could not be read or written.
    #[error("File error: {0}")]
    Io(String),
    /// Something that should never happen.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl CoreError {
    /// Wraps any displayable error as a crypto failure.
    pub fn crypto(e: impl fmt::Display) -> Self {
        Self::Crypto(e.to_string())
    }

    /// Wraps any displayable error as an internal failure.
    pub fn internal(e: impl fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }

    /// Wraps any displayable error as a network failure.
    pub fn network(e: impl fmt::Display) -> Self {
        Self::Network(e.to_string())
    }

    /// Whether retrying later could succeed (used by the outbox).
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Offline | Self::Network(_))
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Self::NotFound,
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<prost::DecodeError> for CoreError {
    fn from(e: prost::DecodeError) -> Self {
        Self::Crypto(format!("malformed message: {e}"))
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

/// Result alias for core operations.
pub type CoreResult<T> = Result<T, CoreError>;
