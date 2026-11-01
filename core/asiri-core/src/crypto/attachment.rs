//! Attachment encryption (architecture §7.7).
//!
//! A fresh 256-bit key per file; XChaCha20-Poly1305 in the STREAM construction (big-endian
//! 32-bit counter + last-block flag) over 64 KiB chunks, so tampering, reordering and truncation
//! are all detected; the ciphertext is then padded to a size bucket. Stored layout:
//!
//! ```text
//! nonce prefix (19 B) ‖ chunk₁ ‖ … ‖ chunkₙ (each ≤ 64 KiB + 16 B tag) ‖ zero padding
//! ```
//!
//! The SHA-256 published in the `Attachment` message covers the whole padded blob, i.e. exactly
//! what the relay stores, so receivers verify it before decrypting anything.

use chacha20poly1305::aead::generic_array::GenericArray;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::padding::attachment_bucket;
use crate::error::{CoreError, CoreResult};
use crate::ids::{fill_random, random_bytes};

/// Plaintext bytes per STREAM chunk.
pub const CHUNK: usize = 64 * 1024;
/// STREAM nonce prefix length for XChaCha20-Poly1305 with a 32-bit counter (24 − 5).
pub const NONCE_PREFIX: usize = 19;
const TAG: usize = 16;

/// An encrypted, padded attachment ready to upload.
pub struct EncryptedAttachment {
    /// The per-file key (sent only inside MLS).
    pub key: Zeroizing<[u8; 32]>,
    /// The padded ciphertext blob.
    pub blob: Vec<u8>,
    /// SHA-256 of `blob`.
    pub sha256: [u8; 32],
    /// Plaintext length.
    pub plaintext_len: u64,
}

fn chunk_count(plaintext_len: u64) -> u64 {
    plaintext_len.div_ceil(CHUNK as u64).max(1)
}

/// Exact (unpadded) ciphertext length for a plaintext of `plaintext_len` bytes.
#[must_use]
pub fn ciphertext_len(plaintext_len: u64) -> u64 {
    NONCE_PREFIX as u64 + plaintext_len + TAG as u64 * chunk_count(plaintext_len)
}

/// Padded blob length for a plaintext of `plaintext_len` bytes.
#[must_use]
pub fn blob_len(plaintext_len: u64) -> u64 {
    attachment_bucket(ciphertext_len(plaintext_len))
}

/// Encrypts and pads an attachment with a fresh random key.
pub fn encrypt(plaintext: &[u8]) -> CoreResult<EncryptedAttachment> {
    let key = Zeroizing::new(random_bytes::<32>());
    let prefix: [u8; NONCE_PREFIX] = random_bytes();
    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(key.as_ref()));
    let mut enc = EncryptorBE32::from_aead(cipher, GenericArray::from_slice(&prefix));

    let len = plaintext.len() as u64;
    let padded_len = usize::try_from(blob_len(len)).map_err(CoreError::internal)?;
    let mut blob = Vec::with_capacity(padded_len);
    blob.extend_from_slice(&prefix);
    let chunks: Vec<&[u8]> = if plaintext.is_empty() {
        vec![&[][..]]
    } else {
        plaintext.chunks(CHUNK).collect()
    };
    let (last, body) = chunks.split_last().expect("at least one chunk");
    for chunk in body {
        blob.extend(
            enc.encrypt_next(*chunk)
                .map_err(|_| CoreError::crypto("encrypt"))?,
        );
    }
    blob.extend(
        enc.encrypt_last(*last)
            .map_err(|_| CoreError::crypto("encrypt"))?,
    );
    debug_assert_eq!(blob.len() as u64, ciphertext_len(len));
    blob.resize(padded_len, 0);
    let sha256 = Sha256::digest(&blob).into();
    Ok(EncryptedAttachment {
        key,
        blob,
        sha256,
        plaintext_len: len,
    })
}

/// Verifies the blob's hash, then decrypts it. Fails loudly on any mismatch.
pub fn decrypt(
    key: &[u8],
    blob: &[u8],
    expected_sha256: &[u8],
    plaintext_len: u64,
) -> CoreResult<Zeroizing<Vec<u8>>> {
    let digest: [u8; 32] = Sha256::digest(blob).into();
    if !bool::from(subtle::ConstantTimeEq::ct_eq(&digest[..], expected_sha256)) {
        return Err(CoreError::Crypto("attachment hash does not match".into()));
    }
    let key: &[u8; 32] = key
        .try_into()
        .map_err(|_| CoreError::Crypto("attachment key must be 32 bytes".into()))?;
    let ct_len = usize::try_from(ciphertext_len(plaintext_len)).map_err(CoreError::internal)?;
    if blob.len() < ct_len || blob.len() as u64 != blob_len(plaintext_len) {
        return Err(CoreError::Crypto("attachment has the wrong size".into()));
    }
    if blob[ct_len..].iter().any(|b| *b != 0) {
        return Err(CoreError::Crypto("attachment padding is not zero".into()));
    }
    let (prefix, body) = blob[..ct_len].split_at(NONCE_PREFIX);
    let cipher = XChaCha20Poly1305::new(GenericArray::from_slice(key));
    let mut dec = DecryptorBE32::from_aead(cipher, GenericArray::from_slice(prefix));
    let mut out = Zeroizing::new(Vec::with_capacity(
        usize::try_from(plaintext_len).map_err(CoreError::internal)?,
    ));
    let chunks: Vec<&[u8]> = body.chunks(CHUNK + TAG).collect();
    let (last, rest) = chunks
        .split_last()
        .ok_or_else(|| CoreError::Crypto("attachment is truncated".into()))?;
    for chunk in rest {
        let plain = dec
            .decrypt_next(*chunk)
            .map_err(|_| CoreError::Crypto("attachment was modified".into()))?;
        out.extend_from_slice(&plain);
    }
    let plain = dec
        .decrypt_last(*last)
        .map_err(|_| CoreError::Crypto("attachment was modified".into()))?;
    out.extend_from_slice(&plain);
    if out.len() as u64 != plaintext_len {
        return Err(CoreError::Crypto("attachment is truncated".into()));
    }
    Ok(out)
}

/// Random bytes for tests and placeholders.
#[must_use]
pub fn random_plaintext(len: usize) -> Vec<u8> {
    let mut v = vec![0u8; len];
    fill_random(&mut v);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_across_chunk_boundaries() {
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK + 17] {
            let plain = random_plaintext(len);
            let enc = encrypt(&plain).unwrap();
            assert_eq!(enc.blob.len() as u64, blob_len(len as u64));
            let back = decrypt(enc.key.as_ref(), &enc.blob, &enc.sha256, len as u64).unwrap();
            assert_eq!(back.as_slice(), plain.as_slice(), "len {len}");
        }
    }

    #[test]
    fn detects_tampering_truncation_and_wrong_keys() {
        let plain = random_plaintext(2 * CHUNK + 5);
        let enc = encrypt(&plain).unwrap();
        let len = plain.len() as u64;

        let mut flipped = enc.blob.clone();
        flipped[NONCE_PREFIX + 10] ^= 1;
        let sha = Sha256::digest(&flipped);
        assert!(decrypt(enc.key.as_ref(), &flipped, &sha, len).is_err());

        assert!(
            decrypt(enc.key.as_ref(), &flipped, &enc.sha256, len).is_err(),
            "hash checked first"
        );

        let wrong = [9u8; 32];
        assert!(decrypt(&wrong, &enc.blob, &enc.sha256, len).is_err());

        // Claiming a shorter plaintext (dropping the final chunk) must fail.
        assert!(decrypt(enc.key.as_ref(), &enc.blob, &enc.sha256, CHUNK as u64 * 2).is_err());
    }

    #[test]
    fn fresh_keys_and_nonces_per_file() {
        let a = encrypt(b"same").unwrap();
        let b = encrypt(b"same").unwrap();
        assert_ne!(a.key.as_ref(), b.key.as_ref());
        assert_ne!(a.blob, b.blob);
    }
}
