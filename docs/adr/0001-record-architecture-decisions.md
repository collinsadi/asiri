# ADR-0001: Record architecture decisions

* **Status:** Accepted
* **Date:** 2026-11-01

## Context

`docs/architecture.md` describes the intended system. Implementation inevitably meets details the
document could not anticipate: library versions, API shapes, platform limits. Silent deviations
erode trust in the document; for security software that is unacceptable.

## Decision

Every significant implementation decision, and every deviation from the architecture document, is
recorded as a numbered ADR in `docs/adr/` using `template.md`. ADRs are immutable once accepted;
a later ADR supersedes an earlier one rather than editing it.

## Consequences

Reviewers and auditors can see *why* the code looks the way it does, and which parts of the design
are still aspirational.
