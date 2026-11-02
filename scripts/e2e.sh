#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  End-to-end tests: real relays and real cores talking over the network.
#
#    ./scripts/e2e.sh           the simulator suites (messaging, groups, files, calls, and a
#                               malicious relay), plus calls through a real LiveKit server
#                               when Docker is running
#    ./scripts/e2e.sh --demo    also print the scripted three-person demo
#
#  The LiveKit image is the one the server deploys (server/deploy/docker-compose.yml).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
demo=false
for arg in "$@"; do
  case "$arg" in
    --demo) demo=true ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done

violet=$'\033[1;35m'; green=$'\033[1;32m'; yellow=$'\033[1;33m'; off=$'\033[0m'
step() { printf '\n%s▸ %s%s\n' "$violet" "$*" "$off"; }

step "Simulator suites: messaging, groups, files and calls, and a relay that misbehaves"
cargo test -p asiri-sim --locked

livekit=""
cleanup() { [ -n "$livekit" ] && docker rm -f "$livekit" >/dev/null 2>&1 || true; rm -f "${config:-}"; }
trap cleanup EXIT INT TERM

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  step "Calls through a real LiveKit server"
  image=$(grep -oE 'livekit/livekit-server:v[0-9.]+' server/deploy/docker-compose.yml)
  config="$(mktemp)"
  cat > "$config" <<'YAML'
port: 7880
bind_addresses: ["0.0.0.0"]
rtc:
  tcp_port: 7881
  udp_port: 7882
  use_external_ip: false
  node_ip: 127.0.0.1
keys:
  APIsimulator: simulator-secret-0123456789abcdefghijklmnop
logging:
  level: warn
YAML
  livekit="asiri-e2e-livekit-$$"
  docker run -d --name "$livekit" -p 127.0.0.1:7880:7880 -p 127.0.0.1:7881:7881 -p 127.0.0.1:7882:7882/udp \
    -v "$config:/etc/livekit.yaml:ro" "$image" --config /etc/livekit.yaml >/dev/null
  for _ in $(seq 1 60); do curl -fsS http://127.0.0.1:7880/ >/dev/null 2>&1 && break; sleep 1; done
  curl -fsS http://127.0.0.1:7880/ >/dev/null || { docker logs "$livekit" | tail -40; exit 1; }
  echo "LiveKit is up ($image)"
  ASIRI_LIVEKIT=1 cargo test -p asiri-sim --test livekit --locked -- --nocapture
else
  printf '%s! Docker isn'"'"'t running, so the LiveKit test was skipped (CI runs it).%s\n' "$yellow" "$off"
fi

if $demo; then
  step "The scripted demo: three people, a DM and a group"
  cargo run -q -p asiri-sim --locked -- demo
fi

printf '\n%s✓ End-to-end tests passed%s\n' "$green" "$off"
