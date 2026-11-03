# Security engineering notes

How the code meets the threat model in [architecture.md §4](architecture.md#4-threat-model), and
what is checked on every push. For reporting a vulnerability, see [SECURITY.md](../SECURITY.md).

## What CI proves on every push

| Check | Where | What it shows |
|---|---|---|
| Unit tests | `cargo test --workspace` | Padding, attachment encryption, verification strings, MLS (post-quantum suite, admin policy, past epochs, persistence), the encrypted store, the relay's storage and ordering |
| Relay protocol | `server/asiri-relay/tests` | Registration, authentication, ordered delivery, commit ordering, key packages, blobs, revocation, call tokens, over real sockets |
| End to end | `tools/asiri-sim/tests` | Real engines with their own encrypted stores talking to a real relay: pairing, DMs, groups, attachments, calls, offline delivery, restarts |
| Malicious relay | `tools/asiri-sim/tests/evil_relay.rs` | A proxy in front of the relay duplicates, replays, tampers with, drops, reorders, fabricates and withholds envelopes |
| LiveKit | `tools/asiri-sim/tests/livekit.rs` | Relay-minted tokens join the pinned LiveKit server through the call bridge; forged tokens are refused |
| Call identities | `tools/asiri-sim/tests/media_calls.rs` | LiveKit only sees random aliases; MLS-authenticated call signals map them to people, first claim wins |
| Test buddy | `tools/asiri-sim/tests/buddy.rs` | A headless friend pairs like a new Mac and exchanges texts, reactions and files, and declines calls |
| Release signing | `tools/asiri-release` | Manifests verify only with the release key; a changed manifest or disk image is refused |
| Hidden from screen capture | `CaptureShieldTests` | Windows leave screenshots, recordings and screen sharing before they're first shown, and windows that open later follow ([ADR-0007](adr/0007-hidden-from-screen-capture.md)) |
| Mac app | `.github/workflows/macos.yml` | The Swift packages build and pass their tests, every screen renders in light and dark mode, and the app builds with the core and LiveKit, then launches |
| Icon | `tools/design/make-icon.py --check` | The committed icon matches its generator |
| Dependency policy | `cargo deny check` | Known vulnerabilities, licenses, sources and banned crates |
| Deployment kit | `server/deploy/check.sh` | Firewall, Tor and LiveKit configuration, scripts, the relay image |
| Commit rules | `scripts/ci/check-commits.sh` | Conventional Commits, no AI attribution |

## Malicious relay: what clients do

| The relay… | The client… | Test |
|---|---|---|
| delivers an envelope twice | processes it once (envelope IDs are remembered for 31 days) | `duplicates_and_replays_are_ignored` |
| replays old ciphertext under a new ID | rejects it: the MLS ratchet already consumed that key | `duplicates_and_replays_are_ignored` |
| flips bits | rejects the message (AEAD), keeps going, and later says a message is missing | `tampered_messages_are_rejected_and_reported` |
| drops a message for good | says "Some messages from Ada may not have arrived" (per-sender sequence numbers) | `suppressed_messages_are_noticed` |
| reorders messages | shows them in the sender's order, with no false alarm | `reordered_messages_are_shown_in_order` |
| invents envelopes of every kind | rejects them all; nothing is created | `fabricated_envelopes_change_nothing` |
| withholds a commit | keeps later messages until the commit arrives, and if it never does, says the chat is out of sync | `a_withheld_commit_is_reported`, `a_late_commit_unblocks_waiting_messages` |
| offers a key that wasn't verified in person | refuses to create the chat | `fetch_key_packages` in `engine/groups.rs` |

Commits are ordered by the relay (one per epoch per group), but clients enforce the admin policy
themselves ([§7.5](architecture.md#75-group-admin-policy)): a commit that breaks it is rejected by
every receiver no matter what the relay accepted.

## Containing failures

* Every database transaction catches panics (`Store::tx`): the transaction rolls back and the
  engine keeps running. A bug in a dependency can fail one operation, not lock up the app.
* Each incoming message is applied inside a savepoint and its own panic boundary: a message that
  fails to decrypt, breaks the policy or crashes a decoder is rejected without side effects.
* Key packages are checked for the exact X-Wing key sizes before they're used.

## Dependency advisories

`deny.toml` lists every advisory that is ignored and why it cannot affect Asiri. Today:

| Advisory | Crate | Why it doesn't apply |
|---|---|---|
| RUSTSEC-2026-0331 | libcrux-kem 0.0.9 | Panics decoding a short X-Wing key. Only a group member could supply one; key packages are length-checked and panics are contained. Fixed in libcrux-kem 0.0.10, which OpenMLS hasn't adopted yet (openmls_libcrux_crypto 0.4.0 is the latest release). |
| RUSTSEC-2026-0330 | libcrux-kem 0.0.9 | Panics on a short encapsulation seed; seeds come from the local RNG at full length. |
| RUSTSEC-2026-0329 | libcrux-hmac-drbg 0.0.1 | Panics for output lengths divisible by 65,536; OpenMLS asks for a few dozen bytes. |
| RUSTSEC-2026-0387 | crypto-bigint 0.5 | Reached only through P-256/P-384 support in openmls_basic_credential. Asiri uses Ed25519 and X-Wing. |
| RUSTSEC-2023-0071 | rsa 0.9 | Arti verifies Tor relays' RSA identity signatures; the Marvin attack needs private-key operations. |

When OpenMLS releases a provider on hpke-rs-libcrux 0.8, update and delete the libcrux entries.
