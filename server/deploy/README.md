# Deploying the Asiri server

One VPS runs everything (architecture §15): **tor** hosting two onion services in
restricted-discovery mode, **asiri-relay** (built on the server from this source) and the
**LiveKit** SFU for calls, both in hardened containers.

```text
VPS
├─ tor (Tor Project repository)
│   ├─ onion "asiri"  :80 → 127.0.0.1:8080 (relay)   :7880 → 127.0.0.1:7880 (call signaling)
│   └─ onion "admin"  :22 → 127.0.0.1:22 (SSH)
├─ docker compose (host networking, read-only, non-root, no capabilities)
│   ├─ asiri-relay   127.0.0.1:8080
│   └─ livekit       127.0.0.1:7880, public 7881/tcp + 7882/udp (media)
├─ asiri-authsync.path  → installs authorized onion client keys into tor, reloads it
└─ nftables (table inet asiri): only 7881/tcp, 7882/udp — and 22/tcp until --lockdown
```

## Requirements

* Debian 13, Ubuntu 24.04 or Ubuntu 26.04, 2 vCPUs, 4 GB RAM, 40 GB disk, a public IPv4
  address (§5.2).
* Ideally nothing else running on it. A server that already runs other services needs
  `--shared` ([below](#sharing-a-server-with-other-services---shared)).
* SSH access with `sudo`.
* Your cloud firewall (on AWS: the instance's **security group**) must allow inbound
  **7881/tcp** and **7882/udp** from anywhere, plus **22/tcp** from your own IP while you still
  manage the server over regular SSH. Nothing else needs to be open: messaging never touches a
  public port.

## Install or update

From a clone of this repository on your Mac:

```bash
scripts/deploy-vps.sh ubuntu@<host> ~/path/to/key.pem
```

The script uploads the committed source (`git archive HEAD`), then runs
[`install.sh`](install.sh) as root. The first run takes a few minutes (Docker, tor, a Rust
build). It is idempotent; run it again after pulling new commits to update. Before it changes
anything, it stops if other services listen publicly (see `--shared` below), if another program
holds one of Asiri's ports (8080, 7880, 7881/tcp, 7882/udp), or if tor is already configured for
something else. It prints:

* the **relay onion address** (also in `/var/lib/tor/asiri/hostname`),
* the public IP LiveKit advertises for media,
* the **admin SSH onion address**.

Both onion services start with a placeholder client key that nobody holds, so they are never
discoverable, even before the first device is authorized.

## Register the first (admin) device

1. In Asiri on your Mac choose **Set up a server**. It shows your device's onion client key.
2. Run:

   ```bash
   scripts/deploy-vps.sh ubuntu@<host> ~/path/to/key.pem --bootstrap '<onion client key>'
   ```

3. Paste the `ASIRI1:…` code it prints into the app (or scan the QR code). The code works once
   and expires after 24 hours.

Friends are then added in person from the app (architecture §11.2); the relay authorizes their
onion keys and `asiri-authsync` installs them into tor within a second.

## Day-to-day

```bash
scripts/deploy-vps.sh ubuntu@<host> key.pem --status      # containers, onion address, devices
ssh … sudo asiri-admin devices list                        # IDs, state, admin flag
ssh … sudo asiri-admin devices revoke <device_id>          # a lost Mac (§11.7)
ssh … sudo asiri-admin policy set invite members           # let any member add friends
ssh … sudo asiri-admin gc                                  # apply retention now
```

## Sharing a server with other services (`--shared`)

Asiri is meant to have a server to itself (architecture §15). On one that already runs a
website or other containers, the installer refuses to go ahead and lists them, because Asiri's
firewall admits only SSH and call media and would cut them off. To install alongside them, and
for every update after that:

```bash
scripts/deploy-vps.sh ubuntu@<host> ~/path/to/key.pem --shared
```

`--shared` installs tor, the relay and LiveKit as usual, but leaves the machine's firewall,
system logging and automatic-update settings as they are (and removes Asiri's firewall if an
earlier install added one). [ADR-0006](../../docs/adr/0006-shared-servers.md) has the trade-offs;
in short:

* Your cloud firewall is the only filter in front of Asiri, so keep it narrow: Asiri itself needs
  just **7881/tcp** and **7882/udp**.
* Whoever gains root through another service on the machine sees what the relay sees (random IDs,
  sizes, timing, the IP addresses of people in calls) and can hold messages back. They can't read
  them: messages, files and calls stay end-to-end encrypted.
* `--lockdown` isn't available; close 22/tcp in your cloud firewall instead if you want SSH only
  over the admin onion.
* The relay is compiled on the server. Its crates and build stay in Docker's build cache so
  updates are quick; if the disk is tight, `sudo docker builder prune` frees them (the next update
  then compiles from scratch).

## Onion-only SSH (`--lockdown`)

The architecture wants SSH reachable only through the admin onion service (§14.2). Do this only
after you have confirmed it works, or keep your provider's web console as a break-glass option:

1. On your Mac: `brew install tor`, then generate a client key pair:

   ```bash
   openssl genpkey -algorithm x25519 -out admin_onion.pem
   openssl pkey -in admin_onion.pem -pubout -outform DER | tail -c 32 | base32 | tr -d '='   # public
   openssl pkey -in admin_onion.pem -outform DER | tail -c 32 | base32 | tr -d '='          # private
   ```

2. On the server, put `descriptor:x25519:<PUBLIC>` into
   `/var/lib/tor/admin/authorized_clients/me.auth` (owner `debian-tor`, mode 600) and
   `sudo systemctl reload tor`.
3. On your Mac, add `ClientOnionAuthDir /usr/local/etc/tor/onion_auth` to your torrc and create
   `onion_auth/admin.auth_private` containing `<admin onion without .onion>:descriptor:x25519:<PRIVATE>`.
4. Test: `ssh -o ProxyCommand='nc -X 5 -x 127.0.0.1:9050 %h %p' ubuntu@<admin onion address>`.
5. Only then: `scripts/deploy-vps.sh ubuntu@<host> key.pem --lockdown`, and remove 22/tcp from the
   security group.

## What lives where

| Path | Contents | Backed up? |
|---|---|---|
| `/var/lib/tor/asiri/`, `/var/lib/tor/admin/` | onion service keys | **Yes**, encrypted and offline (§14.2) |
| `/opt/asiri/secrets/livekit_keys` | LiveKit API key and secret (root:asiri-secrets, 640) | Optional (regenerated on loss) |
| `/opt/asiri/data/relay/` | relay database and encrypted files — ciphertext only | **Never** |
| `/opt/asiri/data/authorized/` | desired onion client keys, maintained by the relay | No |

Logs are kept in RAM only (`journald Storage=volatile`), tor runs with `SafeLogging 1`, LiveKit
logs warnings only, and the relay never logs identifiers.
