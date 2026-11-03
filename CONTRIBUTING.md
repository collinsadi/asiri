# Contributing to Asiri

Thanks for helping keep secrets secret. This document is the source of truth for how code gets
into this repository.

## Commit rules

1. **Conventional Commits.** Every subject line has the form

   ```text
   <type>(<optional scope>): <imperative summary, no trailing period>
   ```

   `type` is one of `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`,
   `style` or `revert`. Common scopes: `core`, `mls`, `store`, `net`, `sync`, `media`, `calls`,
   `pairing`, `ffi`, `relay`, `proto`, `sim`, `macos`, `design`, `deploy`, `release`.
   Keep the subject at 72 characters or fewer. Breaking changes add `!` after the type/scope and a
   `BREAKING CHANGE:` footer.

2. **Small, focused commits, made as you go.** One logical change per commit, each one building
   and passing tests. Never squash a whole feature into a single commit at the end.

3. **Authored as yourself.** Commits are authored with your GitHub identity (the account the
   GitHub CLI is logged in as). Use your GitHub `noreply` address if your email is private.

4. **No AI trailers, ever.** Commit messages and pull-request descriptions must not contain
   `Co-authored-by:` lines for AI tools, “Generated with …” lines, or any other attribution to an
   AI assistant or bot. The `commit-msg` hook in [`.githooks/`](.githooks/) and CI both reject
   them.

5. **Never commit secrets.** No keys, tokens, `.env` files, LiveKit secrets, onion-service keys
   or the release key (only its public half, `tools/release/release-key.pub`, belongs here).
   `.gitignore` covers the usual files, but check `git status` before you commit.

Enable the hooks once per clone:

```bash
git config core.hooksPath .githooks
```

## Development workflow

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/e2e.sh              # simulator suites, plus calls through LiveKit in Docker
./scripts/ship.sh             # everything, including the Mac app (macOS only)
```

CI also runs these, so run the one your change touches before you push:

```bash
cargo clippy -p asiri-core --features tor --all-targets -- -D warnings   # the core with Tor
cargo deny check                                  # advisories, licenses, sources, bans
cargo run -q -p proto-gen && git diff --exit-code core/asiri-proto/src/gen   # after .proto changes
sudo server/deploy/check.sh                       # the deployment kit and every script (Linux)
tools/design/make-icon.py --check                 # after icon changes
```

[docs/security.md](docs/security.md#what-ci-proves-on-every-push) lists everything CI checks.
Releases have their own process: [docs/releasing.md](docs/releasing.md).

* **The Mac app** lives in Swift packages (`apps/macos/Packages`) so most of it builds and
  tests without Xcode projects or Rust: `cd apps/macos/Packages/AsiriKit && swift test`.
  Screens render against `PreviewCore`; every screen has a snapshot test. The Xcode project is
  generated from `apps/macos/project.yml` with XcodeGen and is not committed.
* **The icon** is drawn by `tools/design/make-icon.py`; change the script, not the PNGs.

* **Dependencies** are pinned in the workspace `Cargo.toml` and `Cargo.lock`. Every new crate is
  reviewed before it is added and must pass `cargo deny check`.
* **No third-party SDKs** in the app besides LiveKit: no analytics, crash reporting or ad
  libraries (architecture §14.3).
* **Never log** message content, keys or identifiers (device IDs, inbox IDs, group routes) in
  release builds.
* **No home-made cryptography.** Compose audited primitives; if you think you need a new
  construction, write an ADR first.

## Architecture decision records

Significant decisions — and every deviation from [`docs/architecture.md`](docs/architecture.md) —
are recorded in [`docs/adr/`](docs/adr/). Copy `docs/adr/template.md`, number it, and open it with
your change.
