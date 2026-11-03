# Releasing Asiri

How a version goes from `main` to a download on GitHub, and from there to the apps already
installed. Why it works this way is in [ADR-0008](adr/0008-releases-from-ci.md).

```text
 scripts/release.sh 1.2.0                 Release workflow (GitHub Actions)             your Mac
 ───────────────────────────    tag    ────────────────────────────────────────    ─────────────────────
 version, changelog, commit  ───────▶  waits for CI ▸ builds ▸ signs ▸ notarizes ──▶ sign-update.sh signs
 tag v1.2.0, push                      ▸ packs the DMG ▸ attests ▸ publishes       latest.json offline ▸
                                                                                    deploy-vps.sh --publish
```

## Once: set up signing

1. **A Developer ID certificate.** With a paid Apple Developer account, create a "Developer ID
   Application" certificate (Xcode › Settings › Accounts › Manage Certificates) so it's in your
   login keychain.
2. **An App Store Connect API key for notarization.** In App Store Connect, go to Users and
   Access › Integrations › App Store Connect API › Team Keys, add a key with **Developer**
   access, and download it (`AuthKey_….p8`; Apple offers it only once). Note its key ID and the
   issuer ID shown above the keys.
3. **Hand both to the Release workflow:**

   ```bash
   tools/release/setup-signing.sh --api-key ~/Downloads/AuthKey_ABC123DEFG.p8 \
       --key-id ABC123DEFG --issuer 01234567-89ab-cdef-0123-456789abcdef
   ```

   macOS asks before the certificate leaves the keychain: enter your login password and choose
   **Allow**. The script stores the certificate (under a one-time password) and the API key as
   secrets of the repository's `release` environment, which only version tags and `main` can
   use. It also saves the key as the notarytool profile `asiri` on this Mac.
4. **The release key, for in-app updates.** Asiri installs an update only if its manifest is
   signed with this Ed25519 key, so keep it offline (a USB drive, or a hardware-encrypted disk
   image) and backed up:

   ```bash
   cargo run -q --release -p asiri-release -- keygen --out /Volumes/Offline/release.key
   cargo run -q --release -p asiri-release -- public-key --key /Volumes/Offline/release.key \
     > tools/release/release-key.pub
   ```

   Commit `release-key.pub`. Builds made after that trust the key; builds without it don't check
   for updates at all. Anyone with the private key can ship an "update" to every Asiri, so it
   never goes into the repository or CI.

## Every release

1. **Write the notes.** Under `## [Unreleased]` in [CHANGELOG.md](../CHANGELOG.md), in the
   [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) sections (Added, Changed, Fixed,
   Security…). Commit and push them like any change.
2. **Release:**

   ```bash
   scripts/release.sh 1.2.0            # --dry-run first, if you like
   ```

   On an up-to-date, clean `main`, it sets the version in `Cargo.toml` and
   `apps/macos/project.yml`, dates the changelog section, commits `chore(release): 1.2.0`, tags
   `v1.2.0` (signed, if git has a signing key) and pushes both.
3. **The Release workflow takes over.** It checks that the tag matches the code's version and
   the changelog and is on `main`, and waits for CI to pass on the commit. Then, on a fresh macOS
   runner:
   * builds the core for Apple silicon and Intel, and the app around it
     (`tools/release/make-release.sh`);
   * signs everything with your Developer ID, with the Hardened Runtime on;
   * notarizes the app and staples the ticket to it;
   * packs it into the disk image, which opens on the install window
     (`tools/release/make-dmg.sh`), then signs, notarizes and staples that too;
   * writes `SHA256SUMS.txt` and a provenance attestation for the image;
   * publishes the GitHub release: the changelog section, how to install and check it, the disk
     image and the checksums.

   Watch it with `gh run watch`, or in the repository's Actions tab. It takes about 40 minutes,
   most of it compiling the core twice.
4. **Offer it to installed apps.** On the Mac that has the release key:

   ```bash
   tools/release/sign-update.sh 1.2.0 --key /Volumes/Offline/release.key
   scripts/deploy-vps.sh ubuntu@your-server your-key.pem --publish dist/latest.json dist/Asiri-1.2.0.dmg
   ```

   `sign-update.sh` downloads the image from the release and checks its checksum, its
   attestation, Apple's notarization and the key the app trusts. Then it signs `latest.json`
   and attaches it to the release. Apps on your server see the update within a day.
   `--min-build N` makes builds older than `N` update before they can be used (for security
   fixes), and `--notes "…"` sets the text the app shows. Friends who run their own servers
   download `latest.json` and the disk image from the release and publish them the same way.

## What a release contains

| File | What it is |
|---|---|
| `Asiri-1.2.0.dmg` | The app for Apple silicon and Intel, signed, notarized and stapled, in a signed and notarized disk image |
| `SHA256SUMS.txt` | The disk image's SHA-256 checksum |
| `latest.json` | The update manifest signed with the release key (once `sign-update.sh` has run) |

Anyone can check a download:

```bash
shasum -a 256 -c SHA256SUMS.txt
gh attestation verify Asiri-1.2.0.dmg --repo collinsadi/asiri
spctl --assess --type open --context context:primary-signature -v Asiri-1.2.0.dmg
```

## When something goes wrong

| What happened | What to do |
|---|---|
| CI failed on the release commit | Fix it on `main`, then release the next patch version. Never move a published tag |
| The workflow failed after CI passed (Apple's notary service timed out, say) | Re-run the failed job from the Actions tab, or build the tag again with the workflow on `main`: `gh workflow run release.yml -f tag=v1.2.0` |
| "No signing secrets in the release environment" | Run `tools/release/setup-signing.sh` (step 3 above) |
| Apple rejected the notarization | The job prints Apple's log; it names the file and the problem |
| `sign-update.sh` says the app doesn't trust the key | The release was built without `release-key.pub`, or with another key. Release again with the right one |
| The Developer ID certificate or API key leaked | Revoke it at Apple, make a new one, and run `setup-signing.sh` again |

## Releasing from your Mac

If GitHub Actions is unavailable, the same script builds a release locally into `dist/`, signed
with your certificate and notarized with the `asiri` profile:

```bash
tools/release/make-release.sh --notary-profile asiri --require-notarization \
  [--key /Volumes/Offline/release.key]
gh release create v1.2.0 dist/Asiri-1.2.0.dmg dist/SHA256SUMS.txt --verify-tag \
  --title "Asiri 1.2.0" --notes-file <(tools/release/release-notes.sh 1.2.0)
```

A local release has no provenance attestation, so say so in its notes.

## The install window

The disk image opens on one window: the app, the Applications folder, and a path of dots
between them. `tools/design/make-dmg-background.swift` draws its picture from code, in the app's
colors, and `tools/release/make-dmg.sh` lays out the window with dmgbuild (pinned by hash in
`tools/release/dmg-requirements.txt`). The macOS workflow packs every build of the app and keeps a
picture of the window with its artifacts (`ci-app-*`). Look at it there after changing either
file.
