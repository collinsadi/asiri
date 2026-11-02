#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# Asiri server installer (architecture §15). Idempotent: run it again to update.
#
#   sudo bash install.sh [--public-ip IP] [--lockdown]
#
# Installs and configures, on Debian 13, Ubuntu 24.04 or Ubuntu 26.04:
#   * tor (Tor Project repository) with two onion services in restricted-discovery mode:
#     "asiri" (relay :80 → 127.0.0.1:8080, call signaling :7880 → 127.0.0.1:7880) and
#     "admin" (SSH :22 → 127.0.0.1:22)
#   * Docker Engine, then builds asiri-relay from this source tree and runs it with LiveKit
#   * asiri-authsync (systemd path unit) that installs authorized onion client keys into tor
#   * an nftables table that only admits LiveKit media (7881/tcp, 7882/udp) — and SSH on 22/tcp
#     unless --lockdown is given
#   * volatile journald, unattended security upgrades
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

LOCKDOWN=0
PUBLIC_IP=""
while [ $# -gt 0 ]; do
  case "$1" in
    --lockdown) LOCKDOWN=1 ;;
    --public-ip) PUBLIC_IP="${2:?--public-ip needs an address}"; shift ;;
    -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

[ "$(id -u)" = 0 ] || { echo "install.sh must run as root (use sudo)" >&2; exit 1; }

ASIRI=/opt/asiri
SRC=$(cd "$(dirname "$0")/../.." && pwd)
DEPLOY="$SRC/server/deploy"
# The containers' accounts (docker-compose.yml): the relay, LiveKit, and the group that may
# read the LiveKit keys.
RELAY_UID=10001
LIVEKIT_UID=10002
SECRETS_GID=10010

log()  { printf '\n\033[1;35m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m!! %s\033[0m\n' "$*" >&2; }

# shellcheck disable=SC1091
. /etc/os-release
case "${ID}:${VERSION_ID}" in
  debian:13|ubuntu:24.04|ubuntu:26.04) ;;
  *) warn "untested operating system: ${PRETTY_NAME}. Continuing anyway." ;;
esac
CODENAME="${VERSION_CODENAME}"
export DEBIAN_FRONTEND=noninteractive

log "Installing base packages"
apt-get update -q
apt-get install -y -q ca-certificates curl gnupg nftables unattended-upgrades openssl rsync iproute2

mem_kb=$(awk '/MemTotal/ {print $2}' /proc/meminfo)
if [ "${mem_kb:-0}" -lt 3500000 ] && ! swapon --show | grep -q .; then
  log "Adding a 2 GB swap file (this machine has less than 4 GB of RAM; the Rust build needs it)"
  fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile >/dev/null && swapon /swapfile
  grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

log "Enabling automatic security updates"
cat > /etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF

log "Installing Docker Engine"
if ! command -v docker >/dev/null 2>&1; then
  install -m 0755 -d /etc/apt/keyrings
  curl -fsSL "https://download.docker.com/linux/${ID}/gpg" -o /etc/apt/keyrings/docker.asc
  chmod a+r /etc/apt/keyrings/docker.asc
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/${ID} ${CODENAME} stable" \
    > /etc/apt/sources.list.d/docker.list
  apt-get update -q
  apt-get install -y -q docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
fi
systemctl enable --now docker >/dev/null

log "Installing tor"
if ! command -v tor >/dev/null 2>&1; then
  if curl -fsSL https://deb.torproject.org/torproject.org/A3C4F0F979CAA22CDBA8F512EE8CBC9E886DDD89.asc \
      | gpg --dearmor --yes -o /usr/share/keyrings/deb.torproject.org-keyring.gpg; then
    echo "deb [signed-by=/usr/share/keyrings/deb.torproject.org-keyring.gpg] https://deb.torproject.org/torproject.org ${CODENAME} main" \
      > /etc/apt/sources.list.d/tor.list
    apt-get update -q || true
  fi
  apt-get install -y -q tor deb.torproject.org-keyring 2>/dev/null || apt-get install -y -q tor
fi

log "Laying out ${ASIRI}"
# Host accounts with the containers' IDs give their files and processes a name on the host.
# Ubuntu 26.04 needs them too: its install and chown (uutils) refuse owners they can't look
# up, numeric or not.
getent group "$SECRETS_GID" >/dev/null || groupadd --system --gid "$SECRETS_GID" asiri-secrets
for account in "$RELAY_UID:asiri-relay" "$LIVEKIT_UID:asiri-livekit"; do
  getent passwd "${account%%:*}" >/dev/null || useradd --uid "${account%%:*}" --gid "$SECRETS_GID" \
    --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin "${account#*:}"
done
relay_user=$(getent passwd "$RELAY_UID" | cut -d: -f1)
secrets_group=$(getent group "$SECRETS_GID" | cut -d: -f1)
install -d -m 755 "$ASIRI" "$ASIRI/data"
install -d -m 750 "$ASIRI/data/relay" "$ASIRI/data/authorized" "$ASIRI/secrets"
chown "$relay_user:$secrets_group" "$ASIRI/data/relay" "$ASIRI/data/authorized"
chown "root:$secrets_group" "$ASIRI/secrets"
chmod 750 "$ASIRI/data/relay" "$ASIRI/data/authorized" "$ASIRI/secrets"
if [ "$SRC" != "$ASIRI/src" ]; then
  rsync -a --delete --exclude target --exclude .git --exclude apps "$SRC/" "$ASIRI/src/"
  SRC="$ASIRI/src"; DEPLOY="$SRC/server/deploy"
fi
install -m 644 "$DEPLOY/docker-compose.yml" "$ASIRI/docker-compose.yml"

log "LiveKit API keys"
if [ ! -s "$ASIRI/secrets/livekit_keys" ]; then
  key="API$(openssl rand -hex 6)"
  secret=$(openssl rand -base64 64 | tr -d '\n/+=' | head -c 48)
  printf '%s: %s\n' "$key" "$secret" > "$ASIRI/secrets/livekit_keys"
fi
chown "root:$secrets_group" "$ASIRI/secrets/livekit_keys"
chmod 640 "$ASIRI/secrets/livekit_keys"

log "Public IPv4 for call media"
if [ -z "$PUBLIC_IP" ]; then
  # AWS EC2 (IMDSv2); other providers usually put the public address on the interface.
  token=$(curl -fsS -m 3 -X PUT http://169.254.169.254/latest/api/token \
    -H "X-aws-ec2-metadata-token-ttl-seconds: 60" 2>/dev/null || true)
  if [ -n "$token" ]; then
    PUBLIC_IP=$(curl -fsS -m 3 -H "X-aws-ec2-metadata-token: $token" \
      http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null || true)
  fi
fi
if [ -z "$PUBLIC_IP" ]; then
  PUBLIC_IP=$(ip -4 route get 1.1.1.1 | awk '{for (i=1;i<=NF;i++) if ($i=="src") print $(i+1)}')
  warn "using ${PUBLIC_IP} for call media; pass --public-ip if that is a private address"
fi
sed "s/@NODE_IP@/${PUBLIC_IP}/" "$DEPLOY/livekit.yaml.template" > "$ASIRI/livekit.yaml"
chmod 644 "$ASIRI/livekit.yaml"

log "Configuring the onion services (restricted discovery)"
install -m 644 "$DEPLOY/torrc.template" /etc/tor/torrc
for svc in asiri admin; do
  install -d -o debian-tor -g debian-tor -m 700 "/var/lib/tor/$svc" "/var/lib/tor/$svc/authorized_clients"
  placeholder="/var/lib/tor/$svc/authorized_clients/placeholder.auth"
  if [ ! -s "$placeholder" ]; then
    # A key nobody holds keeps the service undiscoverable from its very first second online.
    pub=$(openssl genpkey -algorithm x25519 | openssl pkey -pubout -outform DER | tail -c 32 | base32 | tr -d '=\n')
    echo "descriptor:x25519:${pub}" > "$placeholder"
    chown debian-tor:debian-tor "$placeholder"
    chmod 600 "$placeholder"
  fi
done
systemctl enable tor >/dev/null 2>&1 || true
systemctl restart tor
for _ in $(seq 1 90); do [ -s /var/lib/tor/asiri/hostname ] && break; sleep 1; done
ONION=$(cat /var/lib/tor/asiri/hostname)
ADMIN_ONION=$(cat /var/lib/tor/admin/hostname 2>/dev/null || echo "(starting)")

log "Installing asiri-authsync"
install -m 755 "$DEPLOY/asiri-authsync" /usr/local/sbin/asiri-authsync
install -m 644 "$DEPLOY/systemd/asiri-authsync.path" "$DEPLOY/systemd/asiri-authsync.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now asiri-authsync.path >/dev/null

log "Logs in RAM only"
install -d /etc/systemd/journald.conf.d
install -m 644 "$DEPLOY/journald-volatile.conf" /etc/systemd/journald.conf.d/asiri.conf
systemctl restart systemd-journald

log "Firewall"
install -d /etc/asiri
if [ "$LOCKDOWN" = 1 ]; then
  ssh_rule="# SSH is reachable only through the admin onion service"
  warn "--lockdown: SSH on port 22 will be closed. Make sure SSH over the admin onion works first."
else
  ssh_rule='tcp dport 22 accept   # SSH (closed by install.sh --lockdown)'
fi
sed "s|@SSH_RULE@|${ssh_rule}|" "$DEPLOY/nftables-asiri.nft" > /etc/asiri/nftables-asiri.nft
nft -c -f /etc/asiri/nftables-asiri.nft
install -m 644 "$DEPLOY/systemd/asiri-firewall.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable asiri-firewall.service >/dev/null
systemctl restart asiri-firewall.service

log "Building asiri-relay and starting it with LiveKit"
printf 'ASIRI_ONION_ADDRESS=%s\n' "$ONION" > "$ASIRI/.env"
chmod 600 "$ASIRI/.env"
cd "$ASIRI"
if ! docker compose build relay; then
  warn "build with the pinned Rust image failed; retrying with the current stable image"
  docker compose build --build-arg RUST_IMAGE=rust:1-slim-trixie relay
fi
docker compose up -d --remove-orphans

cat > /usr/local/bin/asiri-admin <<'EOF'
#!/bin/bash
# Runs asiri-relay commands inside the running relay container, e.g.
#   sudo asiri-admin admin bootstrap --onion-client-key '<key from the app>'
#   sudo asiri-admin devices list
exec docker compose -f /opt/asiri/docker-compose.yml exec -T relay asiri-relay "$@"
EOF
chmod 755 /usr/local/bin/asiri-admin

log "Health checks"
relay_ok=no; livekit_ok=no
for _ in $(seq 1 60); do
  code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/v1/health || true)
  [ "$code" = 401 ] && { relay_ok=yes; break; }   # up, and refusing unauthenticated callers
  sleep 2
done
for _ in $(seq 1 30); do
  curl -fsS -o /dev/null http://127.0.0.1:7880/ && { livekit_ok=yes; break; }
  sleep 2
done

if [ "$relay_ok" = yes ]; then
  status="up"
else
  status="NOT responding (docker compose -f /opt/asiri/docker-compose.yml logs relay)"
fi

cat <<EOF

────────────────────────────────────────────────────────────────────────
 Asiri server is ${status}.

   Relay onion address   ${ONION}
   LiveKit signaling     ${livekit_ok} (media on ${PUBLIC_IP}: 7881/tcp, 7882/udp)
   Admin SSH onion       ${ADMIN_ONION}

 Open 7881/tcp and 7882/udp in your cloud firewall (on AWS: the instance's
 security group). Nothing else needs to be public.

 Next, in Asiri on your Mac choose "Set up a server", copy the onion client
 key it shows, and bootstrap your admin device:

   scripts/deploy-vps.sh <user@host> <ssh-key> --bootstrap '<key>'
   (or on this server: sudo asiri-admin admin bootstrap --onion-client-key '<key>')
────────────────────────────────────────────────────────────────────────
EOF
