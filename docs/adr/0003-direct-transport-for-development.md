# ADR-0003: A direct transport alongside Tor, for development and tests

* **Status:** Accepted
* **Date:** 2026-11-01
* **Relates to:** architecture §8, §19 (M1: “Relay MVP (no Tor yet)”)

## Context

Production traffic must reach the relay only through its onion service (§8.2). But CI runners,
local development, simulated clients and Docker end-to-end tests cannot depend on the public Tor
network, and the roadmap itself starts with a relay reached without Tor.

## Decision

`asiri_core::net` defines a `Dialer` trait that returns an `AsyncRead + AsyncWrite` byte stream
for a relay port. Two implementations exist:

* `TorDialer` (cargo feature `tor`, on by default in app builds) — embedded Arti with
  `onion-service-client`, the device's restricted-discovery key in Arti's keystore, and
  `connect_to_onion_services` enabled.
* `DirectDialer` — plain TCP to `host:port`. The Mac app only offers it in Debug builds or behind
  *Settings → Network → Developer: direct connection*, labelled “not anonymous”.

Everything above the dialer (WebSocket, HTTP, auth, sync, blobs, call signaling bridge) is
identical for both, so tests over the direct transport exercise the production code paths.

## Consequences

A release build can be configured to bypass Tor, which is why the option is hidden, labelled, and
logged to the user as a persistent banner while active. Removing it for v1.0 is a one-line change.

## Amendment (2026-11-03)

As built, the Tor dialer is `OnionDialer` (`core/asiri-core/src/net/tor.rs`). There is no
Debug-only gate, *Developer* setting or banner. A device uses the direct transport only when the
server code it joined with names a direct address, which only development relays (`asiri-sim
relay`, `scripts/ship.sh --local`) print. **Settings › Network** then shows the route
**Direct (development only)** and a warning that this Mac reaches its server without Tor. A
server installed by the deployment kit always hands out onion addresses.
