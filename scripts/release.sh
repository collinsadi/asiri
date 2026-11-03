#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Releases a version of Asiri: sets the version, dates its section of CHANGELOG.md, commits
#  "chore(release): <version>", tags v<version> and pushes both. The tag starts the Release
#  workflow, which waits for CI to pass on that commit, then builds, signs, notarizes and
#  publishes the release (docs/releasing.md).
#
#    scripts/release.sh 1.2.0 [--dry-run | --no-push]
#
#  Write the release's notes under "## [Unreleased]" in CHANGELOG.md first, and run it on an
#  up-to-date main with nothing uncommitted. --dry-run shows the changes and makes none.
#  --no-push commits and tags but pushes nothing, for a release built on your Mac
#  (docs/releasing.md, "Releasing from your Mac").
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

usage() { sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; }
say() { printf '\033[1;35m▸\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

version="" dry_run=false push=true
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=true ;;
    --no-push) push=false ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
    *) [ -z "$version" ] || { usage >&2; exit 2; }; version="$arg" ;;
  esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { usage >&2; exit 2; }
tag="v$version"

# ── Is this the moment? ──────────────────────────────────────────────────────
[ "$(git branch --show-current)" = main ] || die "Release from main"
[ -z "$(git status --porcelain)" ] || die "Commit or stash your changes first"
git fetch --quiet --tags origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main isn't the same as origin/main: pull or push first"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && die "$tag already exists"

# A version only goes up: past the last release, and never below what the files say.
newest() { printf '%s\n' "$@" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1; }
previous="$(git tag -l 'v*' | sed -n 's/^v\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
if [ -n "$previous" ] && [ "$(newest "$previous" "$version")" != "$version" ]; then
  die "$version isn't newer than the last release, $previous"
fi
current="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
app="$(sed -n 's/^ *MARKETING_VERSION: "\(.*\)"/\1/p' apps/macos/project.yml)"
[ "$current" = "$app" ] || die "Cargo.toml says $current but apps/macos/project.yml says $app"
[ "$(newest "$current" "$version")" = "$version" ] || die "The code is already at $current, past $version"

unreleased="$(awk '/^## \[Unreleased\]/ {on = 1; next} on && (/^## / || /^\[[^]]+\]: /) {exit} on' CHANGELOG.md)"
[ -n "$(printf '%s' "$unreleased" | tr -d '[:space:]')" ] \
  || die "CHANGELOG.md has nothing under \"## [Unreleased]\": write the release's notes there first"

# ── The release commit ───────────────────────────────────────────────────────
say "Releasing Asiri $version"
if [ "$current" != "$version" ]; then
  # The workspace version (every crate inherits it) and the app's.
  awk -v v="$version" '!done && /^version = "/ {print "version = \"" v "\""; done = 1; next} {print}' \
    Cargo.toml > Cargo.toml.new && mv Cargo.toml.new Cargo.toml
  sed -i '' -E "s/^( *MARKETING_VERSION: )\"[^\"]*\"/\1\"$version\"/" apps/macos/project.yml
  cargo update --workspace --offline --quiet
fi

repo_url="https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null || echo collinsadi/asiri)"
if [ -n "$previous" ]; then link="$repo_url/compare/v$previous...$tag"; else link="$repo_url/releases/tag/$tag"; fi
awk -v v="$version" -v date="$(date +%Y-%m-%d)" -v unreleased="$repo_url/compare/$tag...HEAD" -v link="$link" '
  /^## \[Unreleased\]/ { print; print ""; print "## [" v "] - " date; next }
  /^\[Unreleased\]: / { print "[Unreleased]: " unreleased; print "[" v "]: " link; linked = 1; next }
  { print }
  END { if (!linked) { print ""; print "[Unreleased]: " unreleased; print "[" v "]: " link } }
' CHANGELOG.md > CHANGELOG.md.new && mv CHANGELOG.md.new CHANGELOG.md
tools/release/release-notes.sh "$version" >/dev/null

git --no-pager diff --stat
if $dry_run; then
  git --no-pager diff
  git checkout -- .
  echo
  echo "(dry run: nothing was committed, tagged or pushed)"
  exit 0
fi

git commit --quiet -am "chore(release): $version"
if [ -n "$(git config --get user.signingkey || true)" ]; then
  git tag -s "$tag" -m "Asiri $version"
else
  git tag -a "$tag" -m "Asiri $version"
fi
if ! $push; then
  cat <<EOF

Committed and tagged $tag here; nothing is pushed. Build it, then publish it:
  tools/release/make-release.sh --notary-profile asiri --require-notarization --key …
  git push origin main                 # and wait for CI to pass on it
  git push origin $tag
  gh release create $tag dist/Asiri-$version.dmg dist/SHA256SUMS.txt dist/latest.json --verify-tag \\
    --title "Asiri $version" --notes-file <(tools/release/release-notes.sh $version --built-locally)
EOF
  exit 0
fi
say "Pushing main and $tag"
git push --quiet --atomic origin main "$tag"

cat <<EOF

The Release workflow is building $tag: it waits for CI on this commit, then signs, notarizes
and publishes it.
  gh run watch \$(gh run list --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')
  $repo_url/releases/tag/$tag (when it's done)

For in-app updates, sign its manifest afterwards: tools/release/sign-update.sh $version --key …
EOF
