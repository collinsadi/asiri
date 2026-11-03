#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Builds Asiri.app in Release around the Rust core (architecture §16.1, step 2).
#  scripts/ship.sh, tools/release/make-release.sh and CI all build the app with this
#  script, so the build CI checks is the build you get.
#
#    tools/release/build-app.sh [--out DIR] [--developer-id] [--timestamp] [SETTING=value …]
#
#    --out DIR       where Asiri.app, xcodebuild.log and Xcode's DerivedData go (default: build)
#    --developer-id  sign with a "Developer ID Application" certificate, or stop
#    --timestamp     have Apple timestamp the signatures (notarization needs it)
#    SETTING=value   handed to xcodebuild, e.g. MARKETING_VERSION=1.2.0
#
#  Architectures: the ones the core was built for. tools/release/build-xcframework.sh
#  builds both, or only this Mac's with --host-only.
#  Signing: ASIRI_SIGN_IDENTITY when set ("-" means ad hoc); otherwise your "Developer ID
#  Application" or "Apple Development" certificate, with the Hardened Runtime; with neither,
#  ad hoc, which runs on this Mac only.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

out=build developer_id=false timestamp=false
settings=()
while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || { echo "--out needs a folder" >&2; exit 2; }; out="$2"; shift 2 ;;
    --developer-id) developer_id=true; shift ;;
    --timestamp) timestamp=true; shift ;;
    -h|--help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    [A-Z]*=*) settings+=("$1"); shift ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done

die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname)" = Darwin ] || die "Asiri.app builds on macOS"
command -v xcodegen >/dev/null 2>&1 || die "XcodeGen is needed: brew install xcodegen"

# ── Architectures: exactly the core's ────────────────────────────────────────
# A Release build otherwise targets Apple silicon and Intel, and can't link a core built
# for one of them.
core_lib="$(find apps/macos/Frameworks/AsiriCoreFFI.xcframework -name libasiri_ffi.a 2>/dev/null | head -1 || true)"
[ -n "$core_lib" ] || die "The Rust core isn't built yet: tools/release/build-xcframework.sh"
archs="$(lipo -archs "$core_lib" 2>/dev/null || lipo -info "$core_lib" 2>/dev/null | sed 's/.*: //' || true)"
case "$archs" in
  "") die "Couldn't read the architectures of $core_lib" ;;
  *" "*) destination="generic/platform=macOS" ;;
  *) destination="platform=macOS,arch=$archs" ;;
esac
echo "Architectures: $archs"

# ── Signing ──────────────────────────────────────────────────────────────────
certificate() {
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -n "s/.*\"\($1: [^\"]*\)\".*/\1/p" | head -1 || true
}
identity="${ASIRI_SIGN_IDENTITY:-}"
if [ -z "$identity" ]; then
  identity="$(certificate "Developer ID Application")"
  if [ -z "$identity" ] && ! $developer_id; then identity="$(certificate "Apple Development")"; fi
fi
if $developer_id && [[ "$identity" != "Developer ID Application: "* ]]; then
  die "No \"Developer ID Application\" certificate in the keychain (architecture §16.1)"
fi

if [ -n "$identity" ] && [ "$identity" != "-" ]; then
  # The team is the certificate's organizational unit. A Developer ID certificate's name also
  # ends with it in brackets; an Apple Development one's ends with a personal ID instead.
  team="$(security find-certificate -c "$identity" -p 2>/dev/null | openssl x509 -noout -subject 2>/dev/null \
    | sed -n 's/.*OU *= *\([A-Z0-9]\{10\}\).*/\1/p' | head -1 || true)"
  if [ -z "$team" ] && [[ "$identity" == "Developer ID Application: "* ]]; then
    team="$(printf '%s\n' "$identity" | sed -n 's/.*(\([A-Z0-9]\{10\}\))$/\1/p')"
  fi
  [ -n "$team" ] || echo "(couldn't read the team from \"$identity\"; Xcode may ask for one)" >&2
  stamp=--timestamp=none
  $timestamp && stamp=--timestamp
  # Every target signs manually with this one certificate. The Swift packages' own targets
  # (LiveKit's resource bundle, say) default to automatic signing, which Xcode refuses to
  # combine with a certificate named on the command line.
  signing=(CODE_SIGN_STYLE=Manual CODE_SIGN_IDENTITY="$identity" DEVELOPMENT_TEAM="$team"
    ENABLE_HARDENED_RUNTIME=YES OTHER_CODE_SIGN_FLAGS="$stamp")
  echo "Signing as $identity"
else
  # Library validation (part of the Hardened Runtime) refuses ad-hoc-signed frameworks, so an
  # ad-hoc build runs without it. Releases are signed and notarized (§16.1).
  signing=(CODE_SIGN_IDENTITY=- ENABLE_HARDENED_RUNTIME=NO)
  echo "No signing certificate in the keychain: signing ad hoc (runs on this Mac only)"
fi
# Xcode adds com.apple.security.get-task-allow to everything it signs in a `build` (only archives
# drop it). That entitlement lets a debugger attach to Asiri and read its memory, and the notary
# service refuses apps that have it.
signing+=(CODE_SIGN_INJECT_BASE_ENTITLEMENTS=NO)

# ── Build ────────────────────────────────────────────────────────────────────
(cd apps/macos && xcodegen generate --quiet)
mkdir -p "$out"
log="$out/xcodebuild.log"
derived="$out/DerivedData"
if ! xcodebuild -project apps/macos/Asiri.xcodeproj -scheme Asiri -configuration Release \
  -destination "$destination" -derivedDataPath "$derived" \
  -skipPackagePluginValidation -skipMacroValidation \
  ARCHS="$archs" ONLY_ACTIVE_ARCH=NO "${signing[@]}" ${settings[@]+"${settings[@]}"} \
  build >"$log" 2>&1; then
  errors="$(grep -E "error:|Undefined symbol|ld: " "$log" | awk '!seen[$0]++' | head -40 || true)"
  if [ -n "$errors" ]; then printf '%s\n' "$errors" >&2; else tail -30 "$log" >&2; fi
  die "The app didn't build; the full log is in $log"
fi

products="$derived/Build/Products/Release"
app="$out/Asiri.app"
rm -rf "$app" "$app.dSYM"
ditto "$products/Asiri.app" "$app"
if [ -d "$products/Asiri.app.dSYM" ]; then ditto "$products/Asiri.app.dSYM" "$app.dSYM"; fi

if ! check="$(codesign --verify --deep --strict --verbose=2 "$app" 2>&1)"; then
  printf '%s\n' "$check" >&2
  $developer_id && die "Asiri.app's signature doesn't check out"
  echo "(the signature didn't verify strictly; Asiri still runs on this Mac)" >&2
fi
# (Captured, then matched: piped into grep -q, codesign could die of SIGPIPE under pipefail and
# the check would pass without looking.)
entitlements="$(codesign -d --entitlements - --xml "$app" 2>/dev/null || true)"
if [[ "$entitlements" == *get-task-allow* ]]; then
  die "Asiri.app would let a debugger attach (com.apple.security.get-task-allow)"
fi
echo "Built $app ($(lipo -archs "$app/Contents/MacOS/Asiri" 2>/dev/null || echo "$archs"))"
