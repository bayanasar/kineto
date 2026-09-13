# Workflow State Machine

## Artifact lifecycle

The core object is an artifact with explicit review state.

Recommended states:

```text
draft
  ↓
candidate
  ↓
selected
  ↓
locked
  ↓
superseded
```

Additional terminal/runtime states may include:

- failed
- canceled
- archived

Lifecycle state is canonical project metadata, not database-only state and not a filename convention.

## Why this matters

A boolean such as `generated = true` is insufficient. The system must distinguish generated/rejected, selected, locked, superseded, inherited, referenced, evicted, and collectable artifacts.

## Selection and lock semantics

Selection stores a pointer to a stable artifact ID. It never renames candidate media.

Locking means downstream artifacts may rely on the selected artifact as stable production truth. Changing a locked artifact creates a new revision and a new dependency/invalidation result; accepted media is never silently overwritten.

`reset` is the explicit destructive exception. It is a direct user action that may supersede the active locked selection and clear the shot's active approval state without a separate unlock step. The locked artifact remains in canonical lineage as `superseded`; it is not overwritten or deleted. The typed shot snapshot exposes whether the active selection is locked before reset. Kineto does not require a modal confirmation protocol for this operation; the destructive meaning belongs to the reset action itself rather than to an implicit upstream edit.

## Canonical publication ordering

Canonical shot state spans `shot.json`, `artifacts.json`, and `selection.json`. Each file is replaced atomically, but the three files are not a cross-file transaction. Writers therefore publish in an order where every completed file boundary is recoverable after a process or power failure:

1. publish the monotonic shot `generation_revision` in `shot.json`
2. publish artifact records in `artifacts.json`
3. publish the human selection pointer in `selection.json`

A crash may leave the revision ahead of the artifacts or the artifacts ahead of the selection pointer. The loader recognizes those intermediate states and reconciles them in memory. It never relies on reusing an already-published generation ID, so the next generation remains possible without hand-editing project files.

For selection changes, artifact status is published before the selection pointer. `selection.json` remains the authority for the human choice; if a crash leaves an unmatched `Selected` status, loading reconciles active candidate statuses to the still-published selection pointer before validating the state.

Recovery does not turn the files into an ACID transaction and does not silently rewrite them on open. A later explicit mutation republishes a coherent canonical state.

## Dependency graph

Artifacts have explicit dependencies at the smallest meaningful production unit.

```text
screenplay_scene_003
   ↓
shot_003_01
   ↓
keyframe_01J_SELECTED
   ↓
video_01J_CANDIDATE
```

Cross-cutting dependencies include character, style, location, props, and continuity state. Do not attach every shot to an entire screenplay revision when a scene-level semantic dependency is sufficient.

## Computed staleness

`current` / `stale` / `incompatible` are computed properties, not hand-maintained flags.

Each generated artifact records an `input_hash` derived from semantic parents, structured intent, provider/model/params, and compiler version. The engine recomputes expected inputs and explains which semantic parent/field changed. Cosmetic presentation edits do not invalidate production work.

Changing a parent never automatically deletes children.

## Human approval checkpoints

Required checkpoints for MVP:

1. screenplay
2. visual style
3. character identity
4. scene/shot plan
5. keyframe
6. video candidate

Voice approval may be independent per character. A declarative workflow recipe may skip inapplicable stages or insert approvals while preserving explicit artifact boundaries.

## Paid job intent and reconciliation

Before a provider invocation, the job system persists a write-ahead intent keyed by a deterministic idempotency key derived from semantic input and operation identity.

Startup reconciliation runs before rescheduling unfinished paid work:

1. load unfinished intent records
2. if a provider job handle exists, query/reconcile remote state
3. attach completed results without reissuing the request
4. retry only according to provider error classification/backoff policy
5. require policy approval before expensive fallback

Runtime state may live in `.kineto/`; resulting durable artifact/provenance state is written to canonical project files.

## Engine/UI events

The Flutter UI observes typed engine/job state through the in-process native boundary. It does not poll the filesystem and it does not receive JSON-RPC frames.

Events are coarse production state transitions, for example:

```text
screenplay.generated
screenplay.locked
character.locked
shot.planned
keyframe.selected
video.generation_started
video.generated
video.selected
artifact.superseded
artifact.became_stale
```

Do not stream bulk media or high-frequency internal telemetry to Dart merely because an event mechanism exists.

## Resumability

A workflow step is restartable from canonical project state plus retained runtime job state.

On application restart:

- unfinished external jobs are reconciled before retry
- remote provider jobs are reattached where possible
- local process jobs are marked interrupted unless independently recoverable
- selected/locked artifacts remain untouched
- incomplete temp files remain isolated from canonical assets

The Rust engine is in-process on desktop; process survival is not the durability mechanism. Deleting `.kineto/` intentionally discards runtime bookkeeping but must not discard durable selections, locks, provenance, or dependencies.
