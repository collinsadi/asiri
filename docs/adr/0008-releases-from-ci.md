# ADR-0008: Build and publish releases in GitHub Actions

* **Status:** Accepted
* **Date:** 2026-11-03
* **Relates to:** architecture §14.3, §16

## Context

§16.1 describes a release built on the maintainer's Mac: the core for Apple silicon and Intel,
the app signed with a Developer ID and notarized, a disk image, and an update manifest signed
with the offline release key. §16.2 delivers updates only through each server's relay, over Tor.
That leaves no way to get Asiri in the first place except building it from source. Every friend
would need Xcode, Rust, about 15 GB of disk and half an hour, and a build made on a different
Mac with a different toolchain each time.

§14.3 asks for release builds "made on a clean, dedicated machine or a CI runner you control",
and for the release manifest to be signed with an offline key.

## Decision

* Every version tag (`v1.2.0`, made by `scripts/release.sh`) is built by the Release workflow
  (`.github/workflows/release.yml`) on a fresh GitHub-hosted macOS runner. It builds only a tag
  on main whose commit has passed CI, and its actions are pinned by commit.
* The workflow signs with the Developer ID certificate and notarizes with an App Store Connect
  API key. Both are secrets of a `release` environment that only version tags and main can use
  (`tools/release/setup-signing.sh`), held in a keychain that lasts as long as the job.
* The result is a GitHub release with a disk image that opens on an install window, its SHA-256
  checksum, and a build provenance attestation (Sigstore) that ties the image to the workflow
  run and the commit. The release notes come from `CHANGELOG.md`.
* The release key never goes to CI. After a release is published, the maintainer checks the
  image (checksum, attestation, notarization, the key the app trusts) and signs `latest.json`
  on their own Mac (`tools/release/sign-update.sh`). Only then can servers offer the update.

The full process is in [docs/releasing.md](../releasing.md).

## Consequences

* Friends install Asiri by downloading one signed, notarized disk image. Building from source
  still works and is still documented.
* Every release is built the same way from a clean machine, and anyone can check where a disk
  image came from: `gh attestation verify Asiri-1.2.0.dmg --repo collinsadi/asiri`.
* GitHub now holds the Developer ID private key and the notarization key. Whoever takes over the
  maintainer's GitHub account or the workflow could publish a malicious image *on GitHub*, which
  would reach new downloads. They still couldn't push it to installed apps: the updater installs
  only images whose manifest the offline key signed. If that happens, revoke the certificate and
  the API key at Apple, and run `setup-signing.sh` again with new ones.
* The runner is clean and short-lived, but GitHub runs it, not the maintainer. That is weaker
  than §14.3's "a CI runner you control". Reproducible builds of the core (§14.3) would let
  anyone check the binary against the source, and remain the goal.
* Unlike the Tor-only updater, downloads from GitHub show GitHub who is downloading. People for
  whom that matters can download through Tor Browser, or build from source.
* A release takes two steps for in-app updates (tag, then sign the manifest). That's deliberate:
  it keeps the one key that can change every installed Asiri off the internet.
