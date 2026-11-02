#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Asiri ship script: checks and builds everything, then hands you Asiri.app.
#
#    ./scripts/ship.sh                  Rust format, lint and tests (with the end-to-end
#                                       simulator), the core XCFramework, the Swift package
#                                       tests with a snapshot of every screen, and a Release
#                                       build of Asiri.app
#    ./scripts/ship.sh --run            …then open Asiri
#    ./scripts/ship.sh --run --demo     …open it on demo data (no server needed)
#    ./scripts/ship.sh --run --local    …also start a development relay on this Mac (no Tor)
#                                       and print the code to paste into Asiri
#    ./scripts/ship.sh --install        …and copy Asiri.app into /Applications
#    ./scripts/ship.sh --quick          skip the test suites (build only)
#    ./scripts/ship.sh --universal      build the core for Apple silicon and Intel
#
#  Output: build/Asiri.app, and build/snapshots/ with a PNG of every screen.
#  Signing: uses your "Developer ID Application" or "Apple Development" identity when the
#  keychain has one (Hardened Runtime on); otherwise signs ad hoc for this Mac only.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

run=false demo=false local_relay=false install=false quick=false universal=false
for arg in "$@"; do
  case "$arg" in
    --run) run=true ;;
    --demo) demo=true ;;
    --local) local_relay=true ;;
    --install) install=true ;;
    --quick) quick=true ;;
    --universal) universal=true ;;
    -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done

bold=$'\033[1m'; violet=$'\033[1;35m'; green=$'\033[1;32m'; red=$'\033[1;31m'; dim=$'\033[2m'; off=$'\033[0m'
step() { printf '\n%s▸ %s%s\n' "$violet" "$*" "$off"; }
ok() { printf '%s✓%s %s\n' "$green" "$off" "$*"; }
die() { printf '%s✗ %s%s\n' "$red" "$*" "$off" >&2; exit 1; }
started=$(date +%s)

# ── Preflight ────────────────────────────────────────────────────────────────
step "Checking the tools"
[ "$(uname)" = Darwin ] || die "Asiri's app builds on macOS. On Linux, run: cargo test --workspace"
xcode_path="$(xcode-select -p 2>/dev/null || true)"
if [[ "$xcode_path" != *.app/* ]]; then
  die "Xcode is needed (not just the Command Line Tools). Install it from the App Store, then:
    sudo xcode-select -s /Applications/Xcode.app"
fi
xcodebuild -version | head -1
if ! command -v cargo >/dev/null 2>&1; then
  [ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 || die "Rust is needed: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
cargo --version
if ! command -v xcodegen >/dev/null 2>&1; then
  command -v brew >/dev/null 2>&1 || die "XcodeGen is needed: install Homebrew (https://brew.sh), then: brew install xcodegen"
  echo "Installing XcodeGen with Homebrew…"
  brew install xcodegen >/dev/null
fi
echo "XcodeGen $(xcodegen --version 2>/dev/null | awk '{print $NF}')"
mkdir -p build

# ── Rust ─────────────────────────────────────────────────────────────────────
if ! $quick; then
  step "Rust: format, lint, and every test (unit, relay, end-to-end simulator)"
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --locked -q -- -D warnings
  if cargo test --workspace --locked >build/rust-tests.log 2>&1; then
    passed=$(grep -E "^test result: ok" build/rust-tests.log | awk '{n += $4} END {print n + 0}')
    ok "Rust: $passed tests passed (log: build/rust-tests.log)"
  else
    grep -E "FAILED|panicked|^error" build/rust-tests.log | head -30 >&2 || true
    die "Rust tests failed; the full log is in build/rust-tests.log"
  fi
fi

# ── The core for Swift ───────────────────────────────────────────────────────
step "Rust core → AsiriCoreFFI.xcframework and its Swift bindings"
if $universal; then
  tools/release/build-xcframework.sh
else
  tools/release/build-xcframework.sh --host-only
fi

# ── Swift packages ───────────────────────────────────────────────────────────
if ! $quick; then
  step "Swift: AsiriKit tests and a snapshot of every screen"
  rm -rf build/snapshots
  if (cd apps/macos/Packages/AsiriKit && SNAPSHOT_DIR="$root/build/snapshots" swift test) >build/swift-tests.log 2>&1; then
    ok "Swift: $(grep -Eo "Executed [0-9]+ tests" build/swift-tests.log | tail -1 | awk '{print $2}') tests passed; every screen is in build/snapshots/"
  else
    grep -E "error:|failed" build/swift-tests.log | head -30 >&2 || true
    die "Swift tests failed; the full log is in build/swift-tests.log"
  fi
fi

# ── The app ──────────────────────────────────────────────────────────────────
step "Generating the Xcode project"
(cd apps/macos && xcodegen generate --quiet)

identity="$(security find-identity -v -p codesigning 2>/dev/null \
  | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
[ -n "$identity" ] || identity="$(security find-identity -v -p codesigning 2>/dev/null \
  | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
signing=()
if [ -n "$identity" ]; then
  team="$(security find-certificate -c "$identity" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null \
    | sed -n 's/.*OU *= *\([A-Z0-9]\{10\}\).*/\1/p')"
  signing=(CODE_SIGN_IDENTITY="$identity" DEVELOPMENT_TEAM="${team:-}" ENABLE_HARDENED_RUNTIME=YES OTHER_CODE_SIGN_FLAGS=--timestamp=none)
  echo "Signing as: $identity"
else
  # Library validation (part of the Hardened Runtime) refuses ad-hoc-signed frameworks, so an
  # ad-hoc build for this Mac runs without it. Release builds are signed and notarized (§16.1).
  signing=(CODE_SIGN_IDENTITY=- ENABLE_HARDENED_RUNTIME=NO)
  echo "No signing identity in the keychain: signing ad hoc (runs on this Mac only)"
fi

step "Building Asiri.app (Release)"
log="build/xcodebuild.log"
if ! xcodebuild -project apps/macos/Asiri.xcodeproj -scheme Asiri -configuration Release \
  -destination 'platform=macOS' -derivedDataPath build/DerivedData \
  -skipPackagePluginValidation -skipMacroValidation "${signing[@]}" build >"$log" 2>&1; then
  grep -E "error:|Undefined symbol|ld: " "$log" | head -40 >&2 || true
  die "The app didn't build; the full log is in $log"
fi
rm -rf build/Asiri.app
ditto build/DerivedData/Build/Products/Release/Asiri.app build/Asiri.app
codesign --verify --deep --strict build/Asiri.app 2>/dev/null && ok "build/Asiri.app is built and its signature checks out" \
  || ok "build/Asiri.app is built"

if $install; then
  step "Installing into /Applications"
  osascript -e 'tell application "Asiri" to quit' >/dev/null 2>&1 || true
  rm -rf /Applications/Asiri.app
  ditto build/Asiri.app /Applications/Asiri.app
  ok "/Applications/Asiri.app"
fi
app="build/Asiri.app"; $install && app="/Applications/Asiri.app"

elapsed=$(( $(date +%s) - started ))
printf '\n%sAll done in %dm %02ds.%s\n' "$bold" $((elapsed / 60)) $((elapsed % 60)) "$off"

# ── Run ──────────────────────────────────────────────────────────────────────
relay_pid=""
cleanup() { [ -n "$relay_pid" ] && kill "$relay_pid" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

if $local_relay; then
  step "Starting a development relay on 127.0.0.1:8080 (no Tor: development only)"
  cargo build -q --release -p asiri-sim --locked
  mkdir -p .asiri-dev
  target/release/asiri-sim relay --listen 127.0.0.1:8080 --data .asiri-dev/relay > .asiri-dev/relay.log 2>&1 &
  relay_pid=$!
  for _ in $(seq 1 50); do grep -q "ASIRI1:" .asiri-dev/relay.log 2>/dev/null && break; sleep 0.2; done
  cat .asiri-dev/relay.log
fi

if $run; then
  step "Opening Asiri"
  if $demo; then open -n "$app" --args --demo; else open "$app"; fi
  $demo && echo "${dim}Demo mode: seeded chats, no server, nothing saved.${off}"
fi

if [ -n "$relay_pid" ]; then
  echo
  echo "In Asiri choose 'Set up a server' and paste the code above under 'Enter the server's code'."
  echo "${dim}The relay keeps running until you press Ctrl-C.${off}"
  wait "$relay_pid"
fi
