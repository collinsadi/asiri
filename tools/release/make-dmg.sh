#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Packages Asiri.app into the disk image people download (architecture §16.1, step 5). It
#  opens as one window: the app on the left, the Applications folder to drag it onto on the
#  right, over a picture drawn by tools/design/make-dmg-background.swift.
#
#    tools/release/make-dmg.sh <Asiri.app> <out.dmg> [--version 1.2.0]
#
#  --version  printed on the window and in the volume's name (default: the app's own)
#
#  Needs Python 3.10 or later. dmgbuild, which writes the window's layout without driving
#  Finder, is installed from tools/release/dmg-requirements.txt (pinned by hash) into
#  build/dmgbuild/. Signing and notarizing the image is make-release.sh's job.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"

usage() { sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

app="" dmg="" version=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    *) if [ -z "$app" ]; then app="$1"; elif [ -z "$dmg" ]; then dmg="$1"; else usage >&2; exit 2; fi; shift ;;
  esac
done
[ -n "$app" ] && [ -n "$dmg" ] || { usage >&2; exit 2; }
[ "$(uname)" = Darwin ] || die "Disk images are made on macOS"
[ -d "$app/Contents/MacOS" ] || die "Not an app: $app"
app="$(cd "$app" && pwd)"
mkdir -p "$(dirname "$dmg")"
dmg="$(cd "$(dirname "$dmg")" && pwd)/$(basename "$dmg")"
[ -n "$version" ] || version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
icon="$app/Contents/Resources/AppIcon.icns"
[ -f "$icon" ] || die "The app has no AppIcon.icns to give the disk"

# ── The window ───────────────────────────────────────────────────────────────
# Points from the top-left corner of the window's content. Icons are placed by their centers;
# the picture is drawn around these same numbers.
width=660 height=420 icon_size=128 text_size=13
app_at=170,214 applications_at=490,214
# Finder's window size includes the title bar (28 points on macOS 15, 32 on macOS 27), and
# the picture starts below it. The window allows for the taller one, and the picture runs on
# past its design (the bleed), so a shorter title bar never leaves an empty strip.
title_bar=32 bleed=20

cd "$root"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

swift tools/design/make-dmg-background.swift --out "$work" --version "$version" \
  --size "${width}x${height}" --bleed "$bleed" --icon-size "$icon_size" --text-size "$text_size" \
  --app "$app_at" --applications "$applications_at" >/dev/null

# ── dmgbuild, pinned ─────────────────────────────────────────────────────────
python=""
for candidate in python3 python3.14 python3.13 python3.12 python3.11 python3.10; do
  if command -v "$candidate" >/dev/null 2>&1 \
    && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 10))' 2>/dev/null; then
    python="$candidate"
    break
  fi
done
[ -n "$python" ] || die "dmgbuild needs Python 3.10 or later: brew install python"
venv="build/dmgbuild"
requirements="tools/release/dmg-requirements.txt"
if ! cmp -s "$requirements" "$venv/requirements.txt" 2>/dev/null; then
  rm -rf "$venv"
  "$python" -m venv "$venv"
  "$venv/bin/python" -m pip install --quiet --disable-pip-version-check --no-deps --only-binary :all: \
    --require-hashes -r "$requirements"
  cp "$requirements" "$venv/requirements.txt"
fi

# ── The image ────────────────────────────────────────────────────────────────
rm -f "$dmg"
"$venv/bin/dmgbuild" -s tools/release/dmg-settings.py \
  -D app="$app" -D icon="$icon" -D background="$work/background.png" \
  -D window="${width}x$((height + title_bar))" -D icon_size="$icon_size" -D text_size="$text_size" \
  -D app_at="$app_at" -D applications_at="$applications_at" \
  "Asiri $version" "$dmg"
hdiutil verify -quiet "$dmg" || die "The disk image doesn't verify: $dmg"
echo "Packed $dmg ($(du -h "$dmg" | cut -f1 | tr -d ' '))"
