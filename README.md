<p align="center">
  <img src="apps/macos/Asiri/Resources/Assets.xcassets/AppIcon.appiconset/icon_256x256.png" width="128" alt="Asiri app icon">
</p>

<h1 align="center">Asiri</h1>

<p align="center"><b>Àṣírí</b> (Yoruba, <i>ah-shee-ree</i>): “secret”.<br>
A private, end-to-end encrypted messenger for macOS, built for a small circle of friends,<br>
with no third-party service in the middle.</p>

---

Asiri lets a group of friends chat, share images and files, send voice notes, and make voice
calls, video calls and screen shares from their Macs. Everything is end-to-end encrypted with
**Messaging Layer Security** (MLS, RFC 9420) using a **hybrid post-quantum** cipher suite.
The only server is one you rent and control: a deliberately “dumb” relay that stores and
forwards ciphertext, reachable only as a **Tor onion service in restricted-discovery mode**,
plus a LiveKit media server for calls whose frames are end-to-end encrypted with keys derived
from each chat’s MLS group.

The full design lives in [`docs/architecture.md`](docs/architecture.md).

## Highlights

| | |
|---|---|
| **One protocol for everything** | Every chat is an MLS group (a DM is a two-person group), suite `MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519` (X25519 + ML-KEM-768). Handshake and application messages are always `PrivateMessage`s, padded to 256 bytes. |
| **A relay that knows nothing** | Random device/inbox IDs and a random `group_route` are all it sees. One commit per epoch per group, in a single `BEGIN IMMEDIATE` transaction, keeps every member in lock-step. |
| **No metadata leaks** | Messaging goes over embedded Tor (Arti) to an onion service that outsiders can’t even discover. No DNS, no CA, no push service, no analytics. |
| **Verified in person** | Friends pair by scanning each other’s QR codes and comparing six emoji. A 60-digit safety number covers remote verification. |
| **Encrypted at rest** | SQLCipher database whose key is wrapped by a Secure Enclave key; optional Touch ID app lock, duress PIN and panic wipe (crypto-shredding). |
| **Calls** | LiveKit SFU with AES-GCM frame encryption; call keys come from the MLS exporter and rotate with every epoch. |
| **A chat UI you’ll want to live in** | Telegram-style timeline with tailed bubbles, patterned wallpaper, reactions with bursts, swipe-to-reply, voice notes with waveforms, and fast spring animations that respect Reduce Motion. |

## Repository layout

```text
asiri/
├── Cargo.toml                 # Rust workspace: core, relay, tools
├── proto/                     # Protocol Buffers: asiri.relay.v1, asiri.app.v1
├── core/
│   ├── asiri-proto/           # generated protobuf code, shared by client and server
│   ├── asiri-core/            # identity, mls, store, net, sync, media, calls, pairing, updater
│   └── asiri-ffi/             # UniFFI bridge to Swift
├── server/
│   ├── asiri-relay/           # axum relay + admin CLI
│   └── deploy/                # docker compose, LiveKit, tor, nftables, systemd, installer
├── apps/macos/                # the native Mac app (SwiftUI) and its Swift packages
├── tools/
│   ├── asiri-sim/             # headless clients, scenario runner, malicious relay
│   ├── uniffi-bindgen/        # pinned bindings generator
│   └── release/               # XCFramework build, manifest signing, notarization helpers
├── scripts/                   # dev, e2e and ship scripts
└── docs/                      # architecture, threat model, ADRs, operations
```

## Getting started

### Prerequisites

* macOS 14 Sonoma or later (Apple silicon recommended) with Xcode 16 or later
* Rust (stable, see `rust-toolchain.toml`) — `rustup` installs the right toolchain automatically
* Homebrew packages: `xcodegen` (the ship script installs it if missing)

### One command

```bash
./scripts/ship.sh            # build the Rust core, the Mac app, run every test suite
./scripts/ship.sh --run      # …and launch Asiri against a local relay
```

### Pieces individually

```bash
cargo test --workspace                     # Rust unit, property and integration tests
cargo run -p asiri-relay -- serve --dev    # a local relay on 127.0.0.1:8080 (no Tor)
cargo run -p asiri-sim -- scenario all     # headless end-to-end scenarios against it
./scripts/e2e.sh                           # relay + LiveKit in Docker + every scenario
./tools/release/build-xcframework.sh       # Rust core → AsiriCoreFFI.xcframework
```

## Status

Asiri follows the milestones in [§19 of the architecture](docs/architecture.md#19-roadmap).
Until an independent security audit has been completed, **use it with test data only**.

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) for the commit rules and development workflow, and
[`SECURITY.md`](SECURITY.md) for how to report a vulnerability.

## License

Copyright 2026 Collins Adi. Licensed under the [Apache License, Version 2.0](LICENSE).
