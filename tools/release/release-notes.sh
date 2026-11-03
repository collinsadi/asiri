#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Prints the GitHub release notes for a version: its section of CHANGELOG.md, then how to
#  install Asiri and check the download. Fails if the changelog has nothing for it.
#
#    tools/release/release-notes.sh 1.2.0
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

version="${1:-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
repo="${GITHUB_REPOSITORY:-collinsadi/asiri}"

# The lines between "## [1.2.0] - date" and the next heading or the link list at the end.
notes="$(awk -v heading="## [$version]" '
  index($0, heading) == 1 { found = 1; next }
  found && (/^## / || /^\[[^]]+\]: /) { exit }
  found { print }
' CHANGELOG.md | sed -e '/./,$!d')"
[ -n "$(printf '%s' "$notes" | tr -d '[:space:]')" ] \
  || { echo "CHANGELOG.md has no notes for $version" >&2; exit 1; }

printf '%s\n' "$notes" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
cat <<EOF

## Install

1. Download **Asiri-$version.dmg** below and open it.
2. Drag **Asiri** onto the **Applications** folder, then open it from there.
3. Joining a friend's server? [docs/agent-setup.md](https://github.com/$repo/blob/v$version/docs/agent-setup.md)
   walks you (or your AI agent) through it.

Asiri needs macOS 14 Sonoma or later, on Apple silicon or Intel. The app and the disk image are
signed with a Developer ID and notarized by Apple.

## Check the download

\`\`\`bash
# The image is the one this release lists…
shasum -a 256 -c SHA256SUMS.txt
# …and this repository's Release workflow built it from the tagged source.
gh attestation verify Asiri-$version.dmg --repo $repo
\`\`\`

> [!WARNING]
> Asiri hasn't had an independent security audit yet. Until it has, use it with test data only.
EOF
