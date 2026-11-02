#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Builds a release of Asiri (architecture §16.1): the core for Apple silicon and Intel, the
#  app signed with your Developer ID and notarized, a disk image, and an update manifest
#  signed with the offline release key.
#
#    tools/release/make-release.sh --version 1.2.0 --build 42 --key /Volumes/Offline/release.key \
#        [--min-build 38] [--notes "Group calls show who is speaking."] [--notary-profile asiri]
#
#  Needs: a "Developer ID Application" certificate in the keychain. To notarize, store your
#  credentials once:  xcrun notarytool store-credentials asiri --apple-id … --team-id …
#  Creates the release key on first use:  cargo run -p asiri-release -- keygen --out release.key
#
#  Output in dist/: Asiri-<version>.dmg and latest.json. Publish both with
#    scripts/deploy-vps.sh <user@host> <ssh-key> --publish dist/latest.json dist/Asiri-<version>.dmg
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

version="" build="" key="" min_build="" notes="" profile=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="$2"; shift 2 ;;
    --build) build="$2"; shift 2 ;;
    --key) key="$2"; shift 2 ;;
    --min-build) min_build="$2"; shift 2 ;;
    --notes) notes="$2"; shift 2 ;;
    --notary-profile) profile="$2"; shift 2 ;;
    -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done
[ -n "$version" ] && [ -n "$build" ] && [ -n "$key" ] || { sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "--version must look like 1.2.0" >&2; exit 2; }
[[ "$build" =~ ^[0-9]+$ ]] || { echo "--build must be a number" >&2; exit 2; }
[ -f "$key" ] || { echo "release key not found: $key" >&2; exit 1; }
[ "$(uname)" = Darwin ] || { echo "releases are built on macOS" >&2; exit 1; }
min_build="${min_build:-$build}"

say() { printf '\033[1;35m▸\033[0m %s\n' "$*"; }

identity="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
[ -n "$identity" ] || { echo "No \"Developer ID Application\" certificate in the keychain (§16.1)." >&2; exit 1; }
team="$(security find-certificate -c "$identity" -p | openssl x509 -noout -subject | sed -n 's/.*OU *= *\([A-Z0-9]\{10\}\).*/\1/p')"
say "Signing as $identity"

public_key="$(cargo run -q --release -p asiri-release --locked -- public-key --key "$key")"

say "Building the core for Apple silicon and Intel"
tools/release/build-xcframework.sh

say "Building Asiri $version ($build)"
(cd apps/macos && xcodegen generate --quiet)
rm -rf build/release
xcodebuild -project apps/macos/Asiri.xcodeproj -scheme Asiri -configuration Release \
  -destination 'generic/platform=macOS' -derivedDataPath build/release/DerivedData \
  -skipPackagePluginValidation -skipMacroValidation \
  ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$build" ASIRI_RELEASE_PUBLIC_KEY="$public_key" \
  CODE_SIGN_IDENTITY="$identity" DEVELOPMENT_TEAM="$team" ENABLE_HARDENED_RUNTIME=YES \
  OTHER_CODE_SIGN_FLAGS="--timestamp" build >build/release/xcodebuild.log 2>&1 \
  || { grep -E "error:" build/release/xcodebuild.log | head -30 >&2; echo "see build/release/xcodebuild.log" >&2; exit 1; }
app="build/release/DerivedData/Build/Products/Release/Asiri.app"
codesign --verify --deep --strict --verbose=2 "$app"

say "Packaging the disk image"
mkdir -p dist build/release/dmg
dmg="dist/Asiri-$version.dmg"
rm -f "$dmg"
ditto "$app" build/release/dmg/Asiri.app
ln -sfn /Applications build/release/dmg/Applications
hdiutil create -quiet -volname "Asiri $version" -srcfolder build/release/dmg -ov -format UDZO "$dmg"
codesign --sign "$identity" --timestamp "$dmg"

if [ -n "$profile" ]; then
  say "Notarizing (Apple checks the app for malware; no user data is involved)"
  xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
  xcrun stapler staple "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose "$dmg"
else
  echo "  (not notarized: pass --notary-profile to notarize; Gatekeeper will warn without it)"
fi

say "Signing the update manifest with the release key"
cargo run -q --release -p asiri-release --locked -- sign --key "$key" --dmg "$dmg" \
  --version "$version" --build "$build" --min-build "$min_build" --notes "$notes" --out dist/latest.json
cargo run -q --release -p asiri-release --locked -- verify --public-key "$public_key" dist/latest.json --dmg "$dmg"

cat <<EOF

Release ready:
  $dmg
  dist/latest.json

Publish it to your server (Asiri checks once a day and offers the update):
  scripts/deploy-vps.sh <user@host> <ssh-key> --publish dist/latest.json $dmg
EOF
