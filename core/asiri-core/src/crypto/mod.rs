//! Cryptographic building blocks that sit outside MLS. Nothing here invents a primitive: it
//! composes XChaCha20-Poly1305 (STREAM), SHA-2, Ed25519 and X25519 (architecture §7.1).

pub mod attachment;
pub mod padding;
pub mod sas;
