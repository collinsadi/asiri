# Setting up Asiri on a friend's Mac

**A guide for an AI coding agent (Claude Code, Codex, Cursor and the like) helping someone join a
friend's Asiri server.** Give your agent this file, or its link, and ask it to follow it.

Asiri is a private, end-to-end encrypted messenger for a small circle of friends (see the
[README](../README.md)). You'll build it from source on this Mac, install it and open it. The last
step, pairing, is done by the people themselves in the app: it's how they make sure nobody is in
the middle.

In this guide, **the user** is the person you're helping, and **the inviter** is the friend who is
already on the Asiri server (usually the person who runs it).

## Rules

1. **Never handle pairing codes or the six emoji.** Codes that start with `ASIRI1:`, the QR codes
   and the emoji the two people compare are only for them. Don't read them from the screen or the
   clipboard, don't ask the user to paste them to you, and don't write them into files, logs,
   chats or commits.
2. **Join, don't host.** The user chooses **Join a friend**. Never choose **Set up a server**, and
   never run `scripts/deploy-vps.sh` or anything under `server/`.
3. **Always go through Tor.** Don't use `ship.sh --local`, `asiri-sim relay` or the direct
   connection developer setting. They skip Tor and exist for development only.
4. **Let the user run anything that needs their password:** `sudo`, the Homebrew installer,
   installing Xcode. Show them the exact command. (In Claude Code they can type it after `!`.)
5. **Don't change Asiri's code or settings to get past an error.** Stop, tell the user what failed
   and which log has the details, and let the inviter look. A patched messenger is worse than
   none.
6. **Stay out of the user's chats.** Don't read Asiri's data. Screenshots won't help either:
   Asiri hides its windows from screenshots and screen sharing, so ask the user what they see.

## What it takes

| | |
|---|---|
| Mac | macOS 14 Sonoma or later. Asiri is built and tested on Apple silicon; Intel Macs build for themselves but are untested |
| Xcode | 16 or later: the full app from the App Store, not just the Command Line Tools |
| Disk | about 15 GB free for the build, or about 5 GB with `--quick` (no tests), plus room for Xcode if it isn't installed |
| Time | 15–30 minutes the first time, mostly compiling |
| Network | GitHub, crates.io and Swift package downloads during the build. Afterwards Asiri only talks to its server, through Tor |
| People | the inviter, in person or on a live voice or video call, for the last step |

## 1. Check the Mac

```bash
sw_vers -productVersion        # 14.0 or later
uname -m                       # arm64 = Apple silicon, x86_64 = Intel
df -h ~ | tail -1              # free space
xcode-select -p                # must end in Xcode.app/Contents/Developer
xcodebuild -version            # Xcode 16 or later
```

If Xcode is missing, or `xcode-select -p` prints `/Library/Developer/CommandLineTools`, ask the
user to install Xcode from the App Store (it needs their Apple ID) and open it once. Then they run:

```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -license accept
sudo xcodebuild -runFirstLaunch
```

## 2. Install the tools

Rust, which needs no password:

```bash
command -v rustup >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"
```

XcodeGen, from Homebrew. If `brew` is missing, the user installs Homebrew from
<https://brew.sh> first (it asks for their password).

```bash
brew install xcodegen
```

## 3. Get the code

```bash
git clone https://github.com/collinsadi/asiri.git ~/Asiri
cd ~/Asiri
git log -1 --format='%h %s (%cr)'
rustup toolchain install "$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)" \
  --profile minimal --component rustfmt --component clippy
```

If the inviter named a version, check it out (`git checkout <commit or tag>`) before you build.

## 4. Build and install

```bash
./scripts/ship.sh --install
```

This runs the Rust and Swift test suites, builds the core and a Release `Asiri.app`, and copies it
into `/Applications`. It ends with `✓ /Applications/Asiri.app` and `All done in …`. It's quiet
while it compiles, so allow up to 45 minutes before you decide it's stuck.

* Short on disk or time: `./scripts/ship.sh --quick --install` skips the test suites.
* If macOS asks whether `codesign` may use a key in the keychain, the user chooses **Always
  Allow**. This only happens on Macs with an Apple developer certificate.
* Without a certificate the app is signed ad hoc, which is fine: it runs on this Mac. Such a build
  keeps Asiri's database key in the login keychain rather than wrapping it with the Secure
  Enclave; **Settings › Security** says which.
* If it fails, the output names the log to read: `build/rust-tests.log`, `build/swift-tests.log`
  or `build/xcodebuild.log`. See [Troubleshooting](#troubleshooting).

## 5. Open Asiri and hand over

```bash
open /Applications/Asiri.app
```

From here on the user does everything. Tell them:

1. Choose **Get started**, then a display name (and a photo if they like). Only friends see it.
2. Choose whether Asiri asks for Touch ID or the Mac's password when it opens.
3. At **Connect to a server**, choose **Join a friend**. Asiri shows a QR code.
4. When macOS asks, allow the camera (for scanning the inviter's code, and for calls) and
   notifications.

## 6. Pairing: for the user and the inviter

**In the same room (best):**

1. The inviter opens Asiri, chooses **Add a Friend in Person…** from the ✎ menu above the chat
   list, stays on **New to Asiri**, and scans the QR code on the user's screen.
2. The inviter's Mac shows a QR code of its own. On the user's Mac, choose **Scan their code** and
   hold it up to the camera. An iPhone works as the camera too (Continuity Camera).
3. Asiri shows **Connecting securely through Tor…**. The first connection can take a minute or
   two.
4. Both Macs show six emoji. If they're the same, in the same order, both choose **They match**.
   If not, both choose **They don't match** and pair again, face to face.

**Not in the same room:**

1. The user chooses **Copy my code** and sends it to the inviter through a channel they both
   already trust, such as an end-to-end encrypted chat.
2. The inviter pastes it under **Add a Friend in Person… › New to Asiri**, chooses **Copy code**,
   and sends that back the same way.
3. The user pastes it into **…or paste their code**.
4. They compare the six emoji **on a live voice or video call where they recognize each other**,
   never by text, and both choose **They match**.

Each code works once and expires after 24 hours; if one fails, the inviter starts again. Only the
server's admins can invite new people unless the server lets everyone (the inviter's
**Settings › Server** shows **Who can invite**).

## 7. Check that it works

Ask the user rather than looking yourself:

* The inviter is in the chat list, marked as verified.
* **Settings › Network** shows **Connected** and the route **Tor onion service**.
* A message to the inviter gets one tick once the server has it, and two once the inviter's Mac
  does.

Then tell the user what you did: the commit you built (`git -C ~/Asiri log -1 --format=%h`),
whether every test passed, and anything you skipped.

## Updating

When the inviter says there's a new version:

```bash
cd ~/Asiri && git pull --ff-only && ./scripts/ship.sh --install
```

Chats and keys are kept: they live in Asiri's container and the keychain, not in the app. A build
from source doesn't update itself, so this is how it stays current.

## Troubleshooting

| What happens | What to do |
|---|---|
| `Xcode is needed (not just the Command Line Tools)` | Step 1 |
| An Xcode license or first-launch error | The user runs `sudo xcodebuild -license accept` and `sudo xcodebuild -runFirstLaunch` |
| `toolchain '…' is not installed` | Run the `rustup toolchain install` line from step 3 inside `~/Asiri` |
| `cargo fmt` or clippy fails on a fresh clone | Check `git status` shows a clean copy; if it does, report it |
| A test fails | Report it with its log. Don't build with `--quick` to get around it unless the user agrees |
| Swift packages fail to download | Usually the network or a proxy: try again, or on another network |
| Copying into `/Applications` is refused | The user's account isn't an administrator: open `build/Asiri.app` from `~/Asiri` instead |
| **Connecting securely through Tor…** for more than three minutes | Tor may be blocked or slow on this network, as on many office and school networks. Try another one, such as a phone hotspot. If that doesn't help, the inviter checks that the server is up |
| The code is refused | Codes work once and for 24 hours: the inviter starts a new invitation |
| Calls connect but there's no sound or picture | The server's firewall must allow 7881/tcp and 7882/udp: the inviter's job |
| Scanning shows no camera | System Settings › Privacy & Security › Camera: allow Asiri. Or paste the codes instead |

## Removing Asiri

In Asiri, **Settings › Security › Erase Asiri Now…** destroys its keys and data on this Mac. Then
delete `/Applications/Asiri.app`. If a Mac with Asiri on it is lost, ask the inviter to revoke it
on the server.
