# ADR-0002: OpenMLS 0.9 with the libcrux provider and the X-Wing suite

* **Status:** Accepted
* **Date:** 2026-11-01
* **Relates to:** architecture §5.1, §7.3 (D3, D4)

## Context

The architecture calls for OpenMLS 0.9+ with `openmls_libcrux_crypto` and the hybrid post-quantum
suite `MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519`, plus `openmls_sqlite_storage` on the same
SQLCipher-backed `rusqlite` connection as the app data.

Current releases: `openmls 0.9.2`, `openmls_libcrux_crypto 0.4.0`, `openmls_sqlite_storage 0.3.0`
(which depends on `rusqlite ^0.37` with `bundled`), all on `openmls_traits 0.6`. The suite is
gated behind the `draft-ietf-mls-pq-ciphersuites` feature and uses provisional code point
`0x004D`.

## Decision

* Pin the four crates above and enable `draft-ietf-mls-pq-ciphersuites` everywhere.
* Use `rusqlite 0.37` with `bundled-sqlcipher`. Cargo unifies features, so the storage crate's
  `bundled` request is satisfied by the SQLCipher build. On Apple targets SQLCipher uses
  CommonCrypto (`SQLCIPHER_CRYPTO_CC`), so the app ships no OpenSSL; on Linux it links the system
  `libcrypto`.
* Asiri owns a composite `OpenMlsProvider`: libcrux for crypto and randomness, and a
  `SqliteStorageProvider` borrowing the core's single connection, constructed per operation while
  the connection lock is held. App data and MLS state therefore change in **one transaction**.
* Every group records its cipher suite so groups can later move with ReInit (§7.3) when final
  code points or fully post-quantum suites ship. Failures to negotiate the suite are hard errors:
  there is no silent fallback (§7.11).

## Consequences

OpenMLS's API changes between minor versions (high likelihood per §20); all OpenMLS usage is kept
behind `asiri_core::mls`, so an upgrade touches one module.
