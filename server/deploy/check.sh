#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Validates the deployment kit without touching a server: shell syntax and lint, the firewall
# rules with and without --lockdown, the Tor configuration, LiveKit's configuration and the
# compose file. CI runs it on every push; run it before deploying if you changed the kit.
#
#   server/deploy/check.sh
#
# Checks whose tool is missing (nft, tor, docker, shellcheck) are skipped with a note; nft and
# tor checks need root, so run with sudo to cover them.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
skip() { printf '  – %s (skipped: %s)\n' "$1" "$2"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$*"; failures=$((failures + 1)); }
have() { command -v "$1" >/dev/null 2>&1; }

echo "Deployment kit"

scripts=(
  "$here/install.sh"
  "$here/asiri-authsync"
  "$here/check.sh"
  "$root/scripts/deploy-vps.sh"
  "$root/scripts/check-commit-message.sh"
  "$root/scripts/ci/check-commits.sh"
  "$root/.githooks/commit-msg"
)
syntax=1
for s in "${scripts[@]}"; do
  bash -n "$s" || { fail "syntax error in ${s#"$root"/}"; syntax=0; }
done
[ "$syntax" = 1 ] && ok "shell syntax (${#scripts[@]} scripts)"
if have shellcheck; then
  if shellcheck -S warning "${scripts[@]}"; then ok "shellcheck"; else fail "shellcheck"; fi
else
  skip "shellcheck" "not installed"
fi

# The firewall exactly as install.sh renders it, in both modes.
for mode in open lockdown; do
  if [ "$mode" = lockdown ]; then
    rule="# SSH is reachable only through the admin onion service"
  else
    rule='tcp dport 22 accept   # SSH (closed by install.sh --lockdown)'
  fi
  sed "s|@SSH_RULE@|${rule}|" "$here/nftables-asiri.nft" > "$work/firewall-$mode.nft"
  if grep -q '@[A-Z_]*@' "$work/firewall-$mode.nft"; then
    fail "firewall ($mode): unrendered placeholder"
  elif ! grep -q 'udp dport 7882 accept' "$work/firewall-$mode.nft"; then
    fail "firewall ($mode): LiveKit media port is not open"
  elif [ "$mode" = lockdown ] && grep -q 'dport 22' "$work/firewall-$mode.nft"; then
    fail "firewall (lockdown): SSH is still open"
  elif have nft && [ "$(id -u)" = 0 ]; then
    if nft -c -f "$work/firewall-$mode.nft"; then ok "firewall ($mode) passes nft -c"; else fail "firewall ($mode) rejected by nft"; fi
  else
    ok "firewall ($mode) rendered"; skip "nft -c ($mode)" "needs nft and root"
  fi
done

# Tor: the two onion services, with directories like install.sh creates them.
if have tor && [ "$(id -u)" = 0 ]; then
  for svc in asiri admin; do
    install -d -m 700 "$work/tor/$svc" "$work/tor/$svc/authorized_clients"
  done
  sed "s|/var/lib/tor/|$work/tor/|g" "$here/torrc.template" > "$work/torrc"
  echo "DataDirectory $work/tor/data" >> "$work/torrc"
  install -d -m 700 "$work/tor/data"
  if tor --verify-config -f "$work/torrc" >"$work/tor.log" 2>&1; then
    ok "torrc passes tor --verify-config"
  else
    fail "torrc rejected by tor"; sed 's/^/      /' "$work/tor.log"
  fi
else
  skip "tor --verify-config" "needs tor and root"
fi
for needle in 'HiddenServicePort 80 127.0.0.1:8080' 'HiddenServicePort 7880 127.0.0.1:7880' 'SocksPort 0'; do
  grep -qF "$needle" "$here/torrc.template" || fail "torrc lacks: $needle"
done

# LiveKit: rendered with a documentation address, keys outside the file, media ports matching
# the firewall.
sed "s/@NODE_IP@/203.0.113.7/" "$here/livekit.yaml.template" > "$work/livekit.yaml"
if grep -q '@[A-Z_]*@' "$work/livekit.yaml"; then
  fail "livekit.yaml: unrendered placeholder"
elif ! grep -qE '^\s*udp_port:\s*7882' "$work/livekit.yaml" || ! grep -qE '^\s*tcp_port:\s*7881' "$work/livekit.yaml"; then
  fail "livekit.yaml: media ports don't match the firewall (7881/tcp, 7882/udp)"
elif grep -qiE 'api_?secret|^keys:' "$work/livekit.yaml"; then
  fail "livekit.yaml must not contain keys (they live in secrets/livekit_keys)"
else
  ok "livekit.yaml rendered"
fi
if have python3 && python3 -c 'import yaml' 2>/dev/null; then
  if python3 -c 'import sys, yaml; yaml.safe_load(open(sys.argv[1]))' "$work/livekit.yaml"; then
    ok "livekit.yaml is valid YAML"
  else
    fail "livekit.yaml is not valid YAML"
  fi
fi

# Compose: the file parses and pins LiveKit.
if have docker && docker compose version >/dev/null 2>&1; then
  if ASIRI_ONION_ADDRESS=example.onion docker compose -f "$here/docker-compose.yml" config -q; then
    ok "docker-compose.yml"
  else
    fail "docker-compose.yml rejected by docker compose"
  fi
else
  skip "docker compose config" "docker not available"
fi
grep -qE 'image: livekit/livekit-server:v[0-9]+\.[0-9]+\.[0-9]+' "$here/docker-compose.yml" \
  || fail "LiveKit image must be pinned to an exact version"

# Nothing about any real server belongs in the repository.
if grep -rIlE '(ec2-[0-9]+-[0-9]+-[0-9]+-[0-9]+\.|BEGIN [A-Z ]*PRIVATE KEY)' "$root" \
  --exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules >"$work/leaks" 2>/dev/null; then
  fail "server details or private keys found in: $(tr '\n' ' ' < "$work/leaks")"
else
  ok "no server addresses or private keys in the repository"
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all checks passed"
