# Security policy

Asiri is security software. Reports are taken seriously and handled privately.

## Reporting a vulnerability

* **Do not open a public issue.** Use GitHub’s private vulnerability reporting
  (“Report a vulnerability” on the repository’s *Security* tab).
* Include the affected component (Mac app, Rust core, relay, deployment), a description of the
  impact, and steps to reproduce. Proof-of-concept code is welcome.
* You will get an acknowledgement within 72 hours and a status update at least weekly until the
  issue is resolved.

## Scope

In scope: the cryptographic protocol design, `asiri-core`, `asiri-relay`, the Mac app, the
deployment kit in `server/deploy`, and the release/update pipeline.

Out of scope (see the threat model in [`docs/architecture.md` §3](docs/architecture.md#3-threat-model)):
a fully compromised Mac, a group member who screenshots or forwards messages, and traffic
correlation by a global passive adversary.

## Status

Asiri has **not** yet had an independent security audit. Until it has, use it with test data only.
