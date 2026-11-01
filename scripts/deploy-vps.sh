#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Deploy (or update) the Asiri server on a VPS, from your Mac.
#
#   scripts/deploy-vps.sh <user@host> <ssh-key> [install options]
#   scripts/deploy-vps.sh <user@host> <ssh-key> --bootstrap '<onion client key from the app>'
#   scripts/deploy-vps.sh <user@host> <ssh-key> --status
#
# Install options are passed to server/deploy/install.sh: --public-ip IP, --lockdown.
# The committed HEAD of this repository is what gets deployed (git archive), and the relay is
# built on the server from that source. Nothing about the server is stored in the repository.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

if [ $# -lt 2 ]; then
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi

HOST="$1"
KEY="$2"
shift 2
REPO="$(cd "$(dirname "$0")/.." && pwd)"

[ -f "$KEY" ] || { echo "SSH key not found: $KEY" >&2; exit 1; }
perms=$(stat -f '%Lp' "$KEY" 2>/dev/null || stat -c '%a' "$KEY")
case "$perms" in
  400|600) ;;
  *) echo "ssh refuses keys other people can read. Run:  chmod 600 '$KEY'" >&2; exit 1 ;;
esac

SSH_OPTS=(-i "$KEY" -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30 -o ConnectTimeout=20)
remote() { ssh "${SSH_OPTS[@]}" "$HOST" "$@"; }

case "${1:-}" in
  --bootstrap)
    onion_key="${2:?--bootstrap needs the onion client key shown by the app}"
    remote "sudo asiri-admin admin bootstrap --onion-client-key '$onion_key'"
    exit 0
    ;;
  --status)
    remote "sudo docker compose -f /opt/asiri/docker-compose.yml ps; echo; echo relay onion: \$(sudo cat /var/lib/tor/asiri/hostname); sudo asiri-admin devices list"
    exit 0
    ;;
esac

echo "==> Packaging $(git -C "$REPO" rev-parse --short HEAD) from $REPO"
if ! git -C "$REPO" diff --quiet HEAD -- server core Cargo.toml Cargo.lock; then
  echo "    note: uncommitted changes under server/ or core/ are not deployed"
fi
tarball="$(mktemp -t asiri-src).tgz"
trap 'rm -f "$tarball"' EXIT
git -C "$REPO" archive --format=tar.gz -o "$tarball" HEAD Cargo.toml Cargo.lock rust-toolchain.toml core server tools

echo "==> Uploading to $HOST"
scp "${SSH_OPTS[@]}" -q "$tarball" "$HOST:/tmp/asiri-src.tgz"

echo "==> Installing (this takes a few minutes the first time: Docker, tor, and a Rust build)"
remote "sudo bash -c 'set -e; rm -rf /opt/asiri/src.new; mkdir -p /opt/asiri/src.new; \
  tar -xzf /tmp/asiri-src.tgz -C /opt/asiri/src.new; rm -f /tmp/asiri-src.tgz; \
  rm -rf /opt/asiri/src; mv /opt/asiri/src.new /opt/asiri/src; \
  bash /opt/asiri/src/server/deploy/install.sh $*'"
