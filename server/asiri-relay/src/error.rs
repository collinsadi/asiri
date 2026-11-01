//! Relay errors and their wire representation. Error details never echo identifiers (§9.3).

use asiri_proto::relay::v1::{self as wire, ErrorCode};

/// An error that is reported to the client.
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// The request is malformed.
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    /// The session or signature is missing or invalid.
    #[error("unauthenticated")]
    Unauthenticated,
    /// The device is not allowed to do this.
    #[error("forbidden: {0}")]
    Forbidden(&'static str),
    /// The referenced object does not exist.
    #[error("not found")]
    NotFound,
    /// The device exceeded a rate limit.
    #[error("rate limited")]
    RateLimited,
    /// The payload is too large.
    #[error("too large")]
    TooLarge,
    /// A storage quota would be exceeded.
    #[error("quota exceeded")]
    QuotaExceeded,
    /// The request conflicts with the current state.
    #[error("conflict: {0}")]
    Conflict(&'static str),
    /// A server-side failure.
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl RelayError {
    /// The protocol error code.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::BadRequest(_) => ErrorCode::BadRequest,
            Self::Unauthenticated => ErrorCode::Unauthenticated,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::NotFound => ErrorCode::NotFound,
            Self::RateLimited => ErrorCode::RateLimited,
            Self::TooLarge => ErrorCode::TooLarge,
            Self::QuotaExceeded => ErrorCode::QuotaExceeded,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// The matching HTTP status.
    #[must_use]
    pub fn http_status(&self) -> http::StatusCode {
        use http::StatusCode as S;
        match self {
            Self::BadRequest(_) => S::BAD_REQUEST,
            Self::Unauthenticated => S::UNAUTHORIZED,
            Self::Forbidden(_) => S::FORBIDDEN,
            Self::NotFound => S::NOT_FOUND,
            Self::RateLimited => S::TOO_MANY_REQUESTS,
            Self::TooLarge => S::PAYLOAD_TOO_LARGE,
            Self::QuotaExceeded => S::INSUFFICIENT_STORAGE,
            Self::Conflict(_) => S::CONFLICT,
            Self::Internal(_) => S::INTERNAL_SERVER_ERROR,
        }
    }

    /// The protobuf error sent to clients.
    #[must_use]
    pub fn to_wire(&self) -> wire::Error {
        let detail = match self {
            // Never leak internals to clients; the cause is logged at warn level without IDs.
            Self::Internal(_) => "internal error".to_owned(),
            other => other.to_string(),
        };
        wire::Error {
            code: self.code() as i32,
            detail,
        }
    }
}

impl From<rusqlite::Error> for RelayError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl From<std::io::Error> for RelayError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.into())
    }
}

/// Result alias for relay operations.
pub type RelayResult<T> = Result<T, RelayError>;
