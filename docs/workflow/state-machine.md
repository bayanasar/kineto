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

Paid/asynchronous provider work uses a durable write-ahead intent in `.kineto/jobs/intents/<job_id>.json`. Every paid intent targets a concrete `artifact_id`, and its deterministic idempotency key is `{operation}:{artifact_id}:{input_hash}`. The `input_hash` may deliberately remain stable across regenerations for staleness semantics; candidate identity is therefore the discriminator between a crash retry and a fresh paid generation. Retrying the same candidate reuses the same key, while another candidate or regeneration must use a new artifact ID and therefore receives a new key.

The runtime keeps a small direct-lookup reservation sidecar under `.kineto/jobs/idempotency/` for each idempotency key. Reservation files contain and re-validate the complete provider/key pair; their filename locator is not authoritative. `prepare` reads only that reservation and, when present, its single referenced intent instead of parsing the entire intent directory. Reservation publication is write-ahead too: `Reserved` is written before the intent, then promoted to `Active`. An interrupted `Reserved` entry with no intent is safe to clear because provider invocation could not yet have happened.

The runtime intent lifecycle is:

```text
Prepared
   ↓ provider call may have happened
Invoked
   ↓ canonical result/provenance attached
Reconciled
```

`Prepared` is written before the first provider call. Before every call attempt the attempt counter is atomically persisted while the intent is still `Prepared`; therefore a crash in the provider-call window leaves evidence that a paid request may already have happened. A returned remote `provider_job_id` is persisted by moving the intent to `Invoked` as soon as the adapter supplies it. A synchronous result also moves to `Invoked`, but it is not marked `Reconciled` until the caller has durably attached the result to canonical project state.

Startup reconciliation runs before unfinished paid work is rescheduled:

1. load and validate runtime intent records from `.kineto/jobs/intents/`
2. for `Prepared`, reconcile by deterministic idempotency key before deciding that another call is safe
3. for `Invoked` with a remote handle, reconcile that handle; without a handle, reconcile by idempotency key
4. if reconciliation finds a completed or pending remote job for a `Prepared` record, persist it as `Invoked` without reissuing the request
5. if a `Prepared` record is not found remotely, only expose retry according to the adapter's retry classification and configured backoff/attempt ceiling
6. if an already-`Invoked` remote handle is reported missing, surface `NeedsAttention`; do not silently convert it into another paid invocation
7. when a completed result is returned, write canonical artifact/provenance state first, then acknowledge the runtime intent as `Reconciled`

Provider adapters expose side-effect-free cost estimation, invocation with the deterministic idempotency key, remote-handle reconciliation, idempotency-key reconciliation, and retryable/terminal error classification. Batch dry-run aggregates call count, integer money micros, currency, and a conservative serial-sum duration upper bound without creating intent records or invoking a provider. The duration intentionally does not model provider concurrency and must not be presented as an exact ETA.

Per-provider execution policy supplies a concurrency ceiling and validated exponential-backoff policy. Paid fallback is configuration, not an implicit router behavior: both the spend ceiling and any explicit-user-approval requirement must pass before an expensive fallback is allowed.

Reconciled intent files are diagnostic history, not recovery evidence. `IntentRetentionPolicy` bounds the number retained per provider (256 by default); pruning runs after canonical acknowledgement and never removes `Prepared` or `Invoked` records. Pruning leaves a compact idempotency tombstone so an exact already-paid candidate cannot become payable again merely because its diagnostic intent record aged out. Unknown JSON fields in stored intents are preserved across load/save cycles, matching the schema's `additionalProperties` contract.

Runtime bookkeeping lives under `.kineto/`; durable artifact/provenance state lives in canonical project files. Deleting `.kineto/` intentionally discards in-flight recovery/idempotency bookkeeping, but it must not change what the user selected, locked, or otherwise accepted as canonical production truth.

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
