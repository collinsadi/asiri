# Changelog

All notable changes to Asiri are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). When a version is released,
`scripts/release.sh` dates its section, and the section becomes the GitHub release's notes.

## [Unreleased]

## [0.1.0] - 2026-11-03

The first release of Asiri, for macOS 14 Sonoma or later on Apple silicon and Intel.

### Messaging

- Chats and groups encrypted end to end with MLS (RFC 9420), using a hybrid post-quantum
  cipher suite (X25519 with ML-KEM-768). Every message is padded and sent as an MLS
  `PrivateMessage`.
- Texts with replies, edits and reactions, plus photos, files, voice notes and disappearing
  messages.
- Voice and video calls with screen sharing. Their media is encrypted end to end with keys
  from the chat's MLS group.
- Pairing in person: scan each other's QR codes (or paste the codes) and compare six emoji.
  A safety number lets you check a friend from afar.

### Privacy and security

- The relay stores and forwards only ciphertext and random identifiers. It's reachable only
  as a Tor onion service in restricted-discovery mode, through Tor built into the app.
- An encrypted database (SQLCipher), an app lock (Touch ID, your Mac's password or a PIN), a
  duress PIN, and Erase Asiri Now.
- Windows are left out of screenshots, screen recordings and screen sharing.
- Updates come from your own server over Tor, and the app installs only disk images signed
  with the offline release key.

### Servers

- `scripts/deploy-vps.sh` installs the relay, LiveKit and Tor on an Ubuntu server in one
  command. It locks the server down, or with `--shared` installs alongside the other
  services the server already runs.

### Releases

- Asiri comes as a signed and notarized disk image that opens on an install window.
- A Release workflow can build, attest and publish one for every version tag
  ([docs/releasing.md](https://github.com/collinsadi/asiri/blob/main/docs/releasing.md)).

[Unreleased]: https://github.com/collinsadi/asiri/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/collinsadi/asiri/releases/tag/v0.1.0
