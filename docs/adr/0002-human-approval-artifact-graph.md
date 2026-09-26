# ADR-0002: Human Approval Artifact Graph

- Status: Accepted
- Date: 2026-09-07

## Context

AI film generation is expensive, probabilistic, and iterative.

A linear pipeline that overwrites intermediate outputs makes it difficult to:

- compare candidates
- preserve accepted work
- revise upstream decisions
- understand provenance
- recover from failures
- reuse character/style assets

## Decision

Represent production output as an artifact dependency graph with explicit lifecycle states.

Core states:

- draft
- candidate
- selected
- locked
- superseded

Every generated artifact records provenance and dependencies.

Downstream generation references locked or explicitly selected artifacts.

## Consequences

### Positive

- human approval is a first-class system concept
- no accepted media is silently overwritten
- revision history is natural
- stale downstream work can be detected
- provider/model experiments remain reproducible
- UI can present branches and candidate comparison cleanly

### Negative

- more metadata than a simple pipeline
- garbage collection requires dependency awareness
- changes to locked parents require invalidation logic

## Rule

Never model important generation state only as `generated: true`.

A full scan of canonical project files must be sufficient to reconstruct all durable artifact lifecycle state in `.kineto/project.db`. The database may contain only derived indexes/caches and runtime or in-flight job bookkeeping; it must never be the sole copy of artifact identity, lifecycle status, selection/lock decisions, provenance, dependencies, or other durable production truth.
