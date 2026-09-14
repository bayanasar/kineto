# Architecture Overview

## High-level architecture

```text
┌──────────────────────────────────────────────┐
│                 Flutter UI                   │
│         WabiSabi presentation layer          │
└──────────────────────┬───────────────────────┘
                       │
                 typed Dart FFI
                       │
┌──────────────────────▼───────────────────────┐
│              Rust application core           │
│                                              │
│  project/artifact state                      │
│  prompt compilation contracts                │
│  provider capability routing                 │
│  paid-job intent/reconciliation               │
│  future workflow/asset/process runtime        │
└───────┬───────────┬───────────┬──────────────┘
        │           │           │
     LLMs       Image/Video     TTS
        │           │           │
 Cloud/local    Cloud/local  Cloud/local
```

The desktop split is a **language/ownership boundary**, not a client/server topology. ADR-0005 supersedes the original separate-process desktop design.

## Ownership

Flutter owns:

- navigation and presentation
- project browser and review screens
- candidate comparison/selection UI
- provider settings and cost-preflight presentation
- lightweight media previews

Rust owns:

- canonical project IO and validation
- artifact identity/lifecycle/dependency semantics
- hashing/indexing/cache management
- provider/model clients and capability routing
- job execution, idempotency, retry and reconciliation
- prompt compilation
- ffmpeg/local model subprocesses
- CPU/GPU-oriented production work

The UI does not own production truth and the Rust core does not own visual design.

## Repository boundaries

```text
kineto/
├── apps/
│   └── kineto/
│       ├── lib/                 # Flutter product code
│       ├── hook/build.dart      # Rust Code Asset build hook
│       └── rust/                # thin app-specific FFI shim
├── crates/
│   ├── kineto-core/             # engine composition/runtime root
│   ├── kineto-project/          # artifact identity/lifecycle/dependency model
│   ├── kineto-prompts/          # prompt degradation/compiler contract
│   ├── kineto-jobs/             # paid-job intent/cost/retry policy model
│   ├── kineto-job-runtime/      # write-ahead persistence/executor/reconciliation
│   └── kineto-providers/        # provider capability/routing model
├── schemas/                     # canonical wire/storage contracts
├── fixtures/projects/           # deterministic golden projects
├── docs/
├── Cargo.lock
├── Cargo.toml
└── rust-toolchain.toml
```

Additional crates appear only when executable, tested subsystem boundaries justify them. Do not add empty crates because a design diagram names a future subsystem.

## Native desktop boundary

Desktop uses Dart FFI and bundled Code Assets:

```text
Flutter
  ↓ direct typed call
opaque native handle / fixed-width ABI
  ↓
kineto-native thin shim
  ↓ normal Rust calls
kineto-core/domain crates
```

There is no desktop JSON-RPC, stdio protocol, localhost server, session token, or engine executable lookup. See [Native Desktop Boundary](native-boundary.md).

Performance rules:

- no JSON/HTTP/stdio serialization for ordinary desktop calls
- no base64 or large media copies through FFI when an asset/path/native buffer suffices
- no duplicate daemon heap without a demonstrated isolation requirement
- no worker pool per feature; execution policy comes from measured workloads
- canonical JSON/TOML is durable storage, not the hot in-memory representation

A future local-web/daemon adapter is separate from desktop FFI and gets its own generated binary transport and security boundary.

## Project/artifact model

Durable production truth is ordinary project state, not `.kineto/project.db`.

Typed invariants in `kineto-project` include:

- stable artifact IDs
- lifecycle states: draft → candidate → selected → locked → superseded
- selection manifests point to IDs instead of renaming media
- semantic vs cosmetic dependency edges
- computed `current` / `stale` / `incompatible` state from recorded dependency hashes
- explanation data showing which parent/field changed

The committed schemas live under `schemas/`; the minimal golden project under `fixtures/projects/minimal` intentionally carries no `.kineto/` dependency.

## Prompt/provider model

`kineto-prompts` returns a structured compilation result rather than only a string:

- compiled provider input
- compiler version
- applied constraints
- dropped constraints with reasons
- derived degradation severity

Identity-affecting degradation can be blocked by policy.

`kineto-providers` describes modality capabilities for LLM/image/video/TTS. Router evaluation refuses incompatible changes such as silently turning image-to-video into text-to-video or dropping a required identity reference. Lesser optional capability loss remains visible as a degradation/warning.

## Job model

Long-running generation is job-oriented, but durability does not depend on an immortal process.

```text
persist Prepared intent + attempt count
        ↓
provider invocation with deterministic idempotency key
        ↓
persist provider_job_id immediately when available
        ↓
attach provider result to canonical project state
        ↓
acknowledge intent as Reconciled
```

`kineto-jobs` owns the pure domain model:

- deterministic idempotency key and canonical input hash
- prepared → invoked → reconciled intent lifecycle
- remote provider job handle
- integer cost estimates (no floating-point money)
- validated concurrency/backoff/error classification policy
- explicit paid fallback policy and spend ceiling

`kineto-job-runtime` owns the executable paid-job safety boundary:

- atomic intent persistence under `.kineto/jobs/intents/`
- write-ahead `Prepared` records before provider invocation
- persisted attempt counters before each provider-call window
- provider adapters that receive the idempotency key
- startup reconciliation by remote handle or idempotency key before retry
- reattachment of pending/completed remote work without duplicate invocation
- retry disposition from provider error classification and backoff policy
- per-provider runtime concurrency limiting
- side-effect-free batch cost/duration dry run
- canonical-result acknowledgement before `Reconciled`

An `Invoked` remote job that later cannot be found is not silently converted into a second paid request. It is surfaced for operator/user resolution. Likewise, a `Prepared` record with evidence of an earlier attempt must be reconciled by idempotency key before the scheduler decides another invocation is safe.

Provider networking remains adapter-specific. The runtime contract deliberately does not turn vendor HTTP payloads into the workflow/domain model.

## Storage durability classes

### Canonical project files

Human-readable TOML/JSON/Markdown plus media own all durable production decisions: identity, lifecycle state, selections/locks, provenance, dependencies, continuity, and accepted generation metadata.

### `.kineto/` derived/runtime state

`.kineto/` may contain derived indexes, caches, search data, provider metadata cache, and in-flight job bookkeeping. Paid-job intents are stored at `.kineto/jobs/intents/<job_id>.json` so process restart can reconcile a provider call that may already have happened.

Deleting `.kineto/` may lose active runtime bookkeeping and therefore the ability to reattach/deduplicate in-flight provider work. It must not change canonical selections, locks, provenance, dependencies, or accepted artifacts.

## Testing seam

- Rust domain crates remain pure/headless.
- `kineto-job-runtime` tests persistence ordering, crash-window reconciliation, retry policy, cost dry-run, paid fallback policy, and provider concurrency with deterministic fake adapters and no provider spend.
- the FFI shim has native ownership tests; Flutter native tests load the real Code Asset.
- `fixtures/projects/minimal` is the committed golden canonical project.
- fake provider capability descriptors exercise provider portability offline.
- real provider adapters must satisfy the same paid-job runtime contract before they are allowed to issue production spend.
