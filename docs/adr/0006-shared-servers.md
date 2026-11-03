# ADR-0006: Running the server alongside other services

* **Status:** Accepted
* **Date:** 2026-11-03
* **Relates to:** architecture §14.2, §15

## Context

The architecture gives Asiri's server a VPS of its own: an nftables policy that drops everything
but call media (§15.5), SSH only through an onion service, and logs kept in RAM (§14.2). The
installer applied all of it unconditionally. On a machine that already serves websites or other
containers, the default-drop firewall would take those services offline the moment it loaded,
and volatile journald would change logging for every program on the machine.

Such machines are common, and Asiri still works on them: the relay and LiveKit's signaling listen
only on loopback, everything users send is end-to-end encrypted, and the design already treats
the server as untrusted (§3, §11.8).

## Decision

`install.sh` inspects the machine before it changes anything:

* If other services listen on public TCP ports, it stops and names them, unless `--shared` is
  given.
* With `--shared` it installs tor, the relay and LiveKit as usual but leaves the firewall,
  journald and unattended-upgrades configuration to the machine's owner. If an earlier install
  added Asiri's firewall or volatile journald, it removes them. `--lockdown` and `--shared` can't
  be combined, because lockdown works through Asiri's firewall.
* In both modes it refuses to replace a torrc that something else configured, and to start when
  another program holds one of Asiri's ports (8080, 7880, 7881/tcp, 7882/udp).

## Consequences

* Asiri can be tried on a server someone already has, without breaking what runs there.
* A shared server doesn't meet §14.2. Anyone who gains root through another service on it sees
  what the relay sees (random device and inbox IDs, group routes, sizes, timing, and the IP
  addresses of call participants) and can withhold or delay messages, which clients detect and
  report (`docs/security.md`). Message content, attachments and call media stay end-to-end
  encrypted.
* The cloud firewall is the only network filter left in front of Asiri's ports, so it must stay
  narrow: 7881/tcp and 7882/udp for calls, nothing else for Asiri.
* Before Asiri carries real conversations (§19), it should move to a server of its own. Until the
  automated move of §11.8 exists, that means everyone pairs again in person.
