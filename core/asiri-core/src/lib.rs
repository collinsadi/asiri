//! `asiri-core`: everything security-critical in Asiri (architecture §6.3).
//!
//! The Mac app owns what people see and every Apple-specific service; this crate owns keys,
//! encryption, storage, networking and sync. It is exposed to Swift through `asiri-ffi`.

#![forbid(unsafe_code)]

pub mod crypto;
pub mod error;
pub mod ids;
pub mod mls;
pub mod store;

pub use error::{CoreError, CoreResult};
