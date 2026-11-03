#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Builds a release of Asiri (architecture §16.1): the core for Apple silicon and Intel, the
#  app signed with your Developer ID and notarized, the disk image people download, and its
#  checksum. The Release workflow runs this for every version tag (docs/releasing.md); it
#  runs the same way on your Mac.
#
#    tools/release/make-release.sh [--version 1.2.0] [--build 42] [--out dist]
#        [--notary-profile asiri] [--require-notarization]
#        [--key /Volumes/Offline/release.key [--min-build 38] [--notes "…"]]
#
#    --version               default: the workspace version in Cargo.toml
#    --build                 the build number, which only ever goes up (default: the number
#                            of commits, so every build of one commit agrees)
#    --notary-profile NAME   notarize with credentials saved by `xcrun notarytool
#                            store-credentials NAME`
#    --require-notarization  stop rather than make a release Apple hasn't notarized
#    --key FILE              also sign the update manifest (latest.json) with the offline
#                            release key; tools/release/sign-update.sh does it later otherwise
#    --min-build, --notes    for the manifest: builds older than this must update first, and
#                            the notes the app shows
#
#  Signing: the "Developer ID Application" certificate in the keychain (or ASIRI_SIGN_IDENTITY).
#  Notarizing: --notary-profile, or an App Store Connect API key: ASIRI_NOTARY_KEY (the .p8
#  file), ASIRI_NOTARY_KEY_ID and ASIRI_NOTARY_ISSUER. Without either, the release isn't
#  notarized and Gatekeeper warns whoever opens it.
#  Updates: the app trusts the public key in tools/release/release-key.pub. Without that file
#  the build doesn't check for updates.
#
#  Output in dist/: Asiri-<version>.dmg, SHA256SUMS.txt, and with --key, latest.json.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

usage() { sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; }
say() { printf '\033[1;35m▸\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

version="" build="" out=dist key="" min_build="" notes="" profile="" require_notarization=false
while [ $# -gt 0 ]; do
  case "$1" in
    --version|--build|--out|--key|--min-build|--notes|--notary-profile)
      [ $# -ge 2 ] || die "$1 needs a value"
      case "$1" in
        --version) version="$2" ;;
        --build) build="$2" ;;
        --out) out="$2" ;;
        --key) key="$2" ;;
        --min-build) min_build="$2" ;;
        --notes) notes="$2" ;;
        --notary-profile) profile="$2" ;;
      esac
      shift 2 ;;
    --require-notarization) require_notarization=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done

[ "$(uname)" = Darwin ] || die "Releases are built on macOS"
[ -n "$version" ] || version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must look like 1.2.0, not \"$version\""
[ -n "$build" ] || build="$(git rev-list --count HEAD)"
[[ "$build" =~ ^[0-9]+$ ]] || die "--build must be a number"
min_build="${min_build:-$build}"
[[ "$min_build" =~ ^[0-9]+$ ]] || die "--min-build must be a number"
[ -z "$key" ] || [ -f "$key" ] || die "Release key not found: $key"

# ── Credentials ──────────────────────────────────────────────────────────────
identity="${ASIRI_SIGN_IDENTITY:-$(security find-identity -v -p codesigning \
  | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}"
[[ "$identity" == "Developer ID Application: "* ]] \
  || die "No \"Developer ID Application\" certificate in the keychain (docs/releasing.md)"

notary=()
if [ -n "$profile" ]; then
  notary=(--keychain-profile "$profile")
elif [ -n "${ASIRI_NOTARY_KEY:-}" ]; then
  [ -f "$ASIRI_NOTARY_KEY" ] || die "ASIRI_NOTARY_KEY isn't a file: $ASIRI_NOTARY_KEY"
  [ -n "${ASIRI_NOTARY_KEY_ID:-}" ] && [ -n "${ASIRI_NOTARY_ISSUER:-}" ] \
    || die "ASIRI_NOTARY_KEY needs ASIRI_NOTARY_KEY_ID and ASIRI_NOTARY_ISSUER too"
  notary=(--key "$ASIRI_NOTARY_KEY" --key-id "$ASIRI_NOTARY_KEY_ID" --issuer "$ASIRI_NOTARY_ISSUER")
fi
if [ ${#notary[@]} -eq 0 ]; then
  $require_notarization && die "No notarization credentials (docs/releasing.md)"
  warn "Not notarizing: Gatekeeper will warn whoever opens this release"
fi

public_key=""
if [ -f tools/release/release-key.pub ]; then
  public_key="$(tr -d '[:space:]' < tools/release/release-key.pub)"
fi
if [ -n "$key" ]; then
  from_key="$(cargo run -q --release -p asiri-release --locked -- public-key --key "$key")"
  [ -z "$public_key" ] || [ "$public_key" = "$from_key" ] \
    || die "$key isn't the release key in tools/release/release-key.pub"
  public_key="$from_key"
fi
[ -n "$public_key" ] || warn "No tools/release/release-key.pub: this build won't check for updates"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# notarize FILE WHAT: submit to Apple's notary service and wait for its verdict.
notarize() {
  say "Notarizing $2 (Apple scans it for malware; no user data is involved)"
  local result="$work/notary.json" status id
  xcrun notarytool submit "$1" "${notary[@]}" --wait --timeout 2h --output-format json >"$result" \
    || { cat "$result" >&2; die "Couldn't submit $2 for notarization"; }
  status="$(plutil -extract status raw -o - "$result" 2>/dev/null || echo unknown)"
  if [ "$status" != Accepted ]; then
    id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
    [ -z "$id" ] || xcrun notarytool log "$id" "${notary[@]}" >&2 || true
    die "Apple didn't notarize $2 (status: $status)"
  fi
}

say "Signing as $identity"

# ── The app ──────────────────────────────────────────────────────────────────
say "Building the core for Apple silicon and Intel"
tools/release/build-xcframework.sh

say "Building Asiri $version ($build)"
rm -rf build/release
ASIRI_SIGN_IDENTITY="$identity" tools/release/build-app.sh --out build/release --developer-id --timestamp \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$build" ASIRI_RELEASE_PUBLIC_KEY="$public_key"
app="build/release/Asiri.app"
archs="$(lipo -archs "$app/Contents/MacOS/Asiri")"
[[ "$archs" == *arm64* && "$archs" == *x86_64* ]] \
  || die "A release runs on Apple silicon and Intel, but this build has only: $archs"
codesign --verify --deep --strict --verbose=2 "$app"
signature="$(codesign -dv "$app" 2>&1)"
[[ "$signature" == *"flags="*"(runtime)"* ]] \
  || die "The release must have the Hardened Runtime on (architecture §6.9)"

if [ ${#notary[@]} -gt 0 ]; then
  ditto -c -k --keepParent "$app" "$work/Asiri.zip"
  notarize "$work/Asiri.zip" "the app"
  # The ticket travels with the app, so Gatekeeper needn't ask Apple about it on first launch.
  xcrun stapler staple -q "$app"
fi

# ── The disk image ───────────────────────────────────────────────────────────
say "Packaging the disk image"
mkdir -p "$out"
dmg="$out/Asiri-$version.dmg"
tools/release/make-dmg.sh "$app" "$dmg" --version "$version"
codesign --force --sign "$identity" --timestamp "$dmg"
if [ ${#notary[@]} -gt 0 ]; then
  notarize "$dmg" "the disk image"
  xcrun stapler staple -q "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
  spctl --assess --type execute --verbose=2 "$app"
fi
(cd "$out" && shasum -a 256 "$(basename "$dmg")" > SHA256SUMS.txt)

# ── The update manifest ──────────────────────────────────────────────────────
if [ -n "$key" ]; then
  say "Signing the update manifest with the release key"
  cargo run -q --release -p asiri-release --locked -- sign --key "$key" --dmg "$dmg" \
    --version "$version" --build "$build" --min-build "$min_build" --notes "$notes" --out "$out/latest.json"
  cargo run -q --release -p asiri-release --locked -- verify --public-key "$public_key" "$out/latest.json" --dmg "$dmg"
fi

echo
say "Asiri $version ($build) is ready in $out/"
for file in "$out"/*; do printf '  %s (%s)\n' "$file" "$(du -h "$file" | cut -f1 | tr -d ' ')"; done
if [ -n "$key" ]; then
  echo
  echo "Offer it to the apps on your server (they check once a day):"
  echo "  scripts/deploy-vps.sh <user@host> <ssh-key> --publish $out/latest.json $dmg"
fi
