#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Signs the update manifest for a published release with the offline release key, so the
#  apps on your server offer the update (architecture §16.2). CI builds and publishes the
#  release but never holds this key: run this afterwards on the Mac that does.
#
#    tools/release/sign-update.sh 1.2.0 --key /Volumes/Offline/release.key \
#        [--min-build 38] [--notes "Group calls show who is speaking."] [--out dist]
#
#  It downloads Asiri-1.2.0.dmg from the GitHub release and checks it against the release's
#  checksum, its provenance attestation, Apple's notarization, and the release key built into
#  the app. Then it writes and verifies latest.json and attaches it to the release. Publish
#  both to your server with:
#
#    scripts/deploy-vps.sh <user@host> <ssh-key> --publish dist/latest.json dist/Asiri-1.2.0.dmg
#
#  Needs the GitHub CLI (gh), signed in.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

usage() { sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; }
say() { printf '\033[1;35m▸\033[0m %s\n' "$*"; }
ok() { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

version="" key="" min_build="" notes="" out=dist
while [ $# -gt 0 ]; do
  case "$1" in
    --key|--min-build|--notes|--out)
      [ $# -ge 2 ] || die "$1 needs a value"
      case "$1" in
        --key) key="$2" ;;
        --min-build) min_build="$2" ;;
        --notes) notes="$2" ;;
        --out) out="$2" ;;
      esac
      shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    *) [ -z "$version" ] || { usage >&2; exit 2; }; version="$1"; shift ;;
  esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && [ -n "$key" ] || { usage >&2; exit 2; }
[ -f "$key" ] || die "Release key not found: $key"
[ -z "$min_build" ] || [[ "$min_build" =~ ^[0-9]+$ ]] || die "--min-build must be a number"
command -v gh >/dev/null 2>&1 || die "The GitHub CLI is needed: brew install gh"
repo="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
tag="v$version"
dmg="$out/Asiri-$version.dmg"

say "Downloading Asiri $version from github.com/$repo"
mkdir -p "$out"
gh release download "$tag" --repo "$repo" --dir "$out" --clobber \
  --pattern "Asiri-$version.dmg" --pattern SHA256SUMS.txt
(cd "$out" && shasum -a 256 --check --status SHA256SUMS.txt) || die "$dmg doesn't match the release's SHA256SUMS.txt"
ok "It matches the release's checksum"
gh attestation verify "$dmg" --repo "$repo" >/dev/null || die "$dmg has no valid provenance attestation from $repo"
ok "GitHub attests that $repo's Release workflow built it"
xcrun stapler validate -q "$dmg" || die "$dmg has no notarization ticket stapled to it"
spctl --assess --type open --context context:primary-signature "$dmg" 2>/dev/null \
  || die "Gatekeeper rejects $dmg"
ok "It's signed with a Developer ID and notarized by Apple"

public_key="$(cargo run -q --release -p asiri-release --locked -- public-key --key "$key")"
mount="$(mktemp -d)"
trap 'hdiutil detach -quiet "$mount" 2>/dev/null || true; rmdir "$mount" 2>/dev/null || true' EXIT
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$mount" "$dmg"
info="$mount/Asiri.app/Contents/Info.plist"
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$info" 2>/dev/null || true; }
[ "$(plist CFBundleShortVersionString)" = "$version" ] || die "The app in $dmg says it's $(plist CFBundleShortVersionString)"
build="$(plist CFBundleVersion)"
[[ "$build" =~ ^[0-9]+$ ]] || die "The app in $dmg has no build number"
[ "$(plist AsiriReleasePublicKey)" = "$public_key" ] \
  || die "The app in $dmg doesn't trust this release key, so it couldn't take later updates either"
codesign --verify --deep --strict "$mount/Asiri.app" || die "The app's signature doesn't verify"
hdiutil detach -quiet "$mount"
ok "The app is $version ($build) and trusts this release key"

say "Signing the update manifest"
cargo run -q --release -p asiri-release --locked -- sign --key "$key" --dmg "$dmg" \
  --version "$version" --build "$build" --min-build "${min_build:-$build}" --notes "$notes" --out "$out/latest.json"
cargo run -q --release -p asiri-release --locked -- verify --public-key "$public_key" "$out/latest.json" --dmg "$dmg"
gh release upload "$tag" "$out/latest.json" --repo "$repo" --clobber
ok "Attached latest.json to the $tag release"

cat <<EOF

Offer it to the apps on your server (they check once a day):
  scripts/deploy-vps.sh <user@host> <ssh-key> --publish $out/latest.json $dmg
EOF
