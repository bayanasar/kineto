# MVP Roadmap

## Cross-cutting product rules

### WabiSabi is the Flutter UI system

All Kineto Flutter UI consumes WabiSabi from `https://github.com/bayanasar/wabisabi.git` for reusable components, themes, and design tokens.

Kineto must not grow a parallel local design system. Product screens may contain Kineto content, state, composition, project data, and feature-specific dimensions; reusable UI primitives and reusable visual rules belong upstream in WabiSabi.

When Kineto exposes a missing reusable widget/token/toolkit capability, implement it in WabiSabi, update WabiSabi's example/showcase and tests, then upgrade the Kineto consumer. See [WabiSabi UI Consumer Playbook](../playbooks/wabisabi-ui-consumer.md).

### Native resources are first-class

Desktop Flutter and Rust are one application. Ordinary Flutter↔Rust calls use typed in-process FFI per ADR-0005; do not add JSON-RPC, localhost, stdio framing, or a daemon to the desktop path without a measured requirement.

Rust owns production compute and machine-local orchestration. Large media stays in files/native memory where possible instead of being copied through Dart. Reliability comes from durable job state and reconciliation rather than keeping an engine process alive at all costs.

## P0 — Foundation

Goal: prove the local application architecture before adding generation.

Deliverables:

- Flutter shell consuming WabiSabi
- Rust `kineto-core` library
- typed native FFI boundary with explicit ownership
- Flutter build hook that compiles/bundles Rust as a Code Asset
- project create/open
- project folder layout
- schema validation
- SQLite local index
- job model
- asset registry
- settings and secret storage abstraction
- deterministic fake provider seam
- at least one committed golden project fixture

Testing strategy:

- `cargo fmt`, clippy with warnings denied, and workspace tests
- actual Flutter FFI smoke test against the bundled Rust CodeAsset
- workflow/artifact-state tests run offline with zero provider spend
- fake adapters cover deterministic success, latency, transient failure, terminal failure, and capability limits
- golden projects cover schema validation, DB reconstruction, invalidation, and migration
- `.kineto/` deletion/rebuild tests assert durable selections, locks, provenance, and dependencies survive

Interactive checkpoint now available on the bootstrap branch:

```text
Local demo project
  → Scene 001 / Shot 001
  → generate 3 deterministic candidates
  → select / reselect
  → lock
  → close and reopen the native engine
  → selection and lock remain
```

This checkpoint uses the real Rust artifact lifecycle, typed FFI, and WabiSabi UI. Its approval state is written as canonical project files, but the deterministic candidates are demo scaffolding with no generated media and no provider behind them; they are not a provider implementation.

Exit criteria:

- desktop app loads the Rust library in-process without an external engine executable
- no JSON/RPC/string-dispatch layer exists on the desktop native boundary
- Rust can create/read/update canonical project state
- app restart can reconstruct durable state and reconcile unfinished jobs
- deleting derived local index/cache does not change durable production truth
- Flutter UI uses WabiSabi for theme/tokens/reusable components

## P0.5 — Narrow vertical slice

Goal: validate abstract schemas against one real generation path before generalizing further.

Build one deliberately narrow path from a short source/scene through one provider chain to one generated video shot. Task-specific code is acceptable when it exposes real shot intent, provider capability, artifact dependency, approval, cost, and memory/copy semantics.

The P0 interactive checkpoint proves local approval-state ergonomics before provider spend. P0.5 replaces the deterministic demo candidates with one real provider chain while preserving the same select/reselect/lock UX and Rust-owned state boundary.

Profile the slice on Apple Silicon and at least one x86_64 desktop before adding extra process boundaries, worker pools, bulk bridge copies, or caches.

Status: the slice runs end to end with MiniMax as the one provider chain — `image-01` keyframes (three takes per generation, invoked in parallel) and a Hailuo image-to-video clip from the locked keyframe, generated in the background and attached when it finishes. Both are write-ahead jobs that survive restarts without paying twice, select/lock UX is unchanged, and the provider is chosen explicitly with `KINETO_IMAGE_PROVIDER=minimax` (development credentials from `KINETO_MINIMAX_API_KEY`). The app opens a finished clip in the system player; in-app playback is not built. Apple Silicon profiling of the provider path is still open.

## P1 — Source to screenplay

Deliverables:

- source import
- LLM provider abstraction
- one cloud provider
- one OpenAI-compatible/local provider
- story analysis schema
- screenplay generation
- screenplay review/editor
- revision and lock model
- write-ahead job intent + idempotency key
- provider retry/rate-limit/error-classification policy
- generation cost preflight/dry-run

Exit criteria:

- user imports source text
- generates screenplay
- edits/reviews it
- locks a screenplay revision

## P2 — Style and characters

Deliverables:

- world/character bible
- image provider abstraction
- explicit image capability model
- style preset system
- style candidate generation
- character candidate generation
- character reference packs
- style and character locking
- prompt compilation result with applied/dropped constraints and degradation level

Exit criteria:

- user can lock a visual style and canonical cast

## P3 — Scene planning and keyframes

Deliverables:

- scene breakdown
- shot schema
- declarative workflow recipe/step schema
- default recipe reproducing the documented production pipeline
- shot planner
- continuity context builder
- keyframe generation
- candidate compare/select/refine UI
- semantic input hashing and explainable staleness at sub-artifact granularity

Exit criteria:

- locked screenplay can produce a reviewed shot list and selected keyframes

## P4 — Video generation and production handoff

Deliverables:

- video provider abstraction
- explicit video capability model
- provider-specific prompt compiler
- candidate generation
- refine/branch flow
- local asset download/storage
- selected-shot workflow
- retention classes and dependency-aware media GC with dry-run
- content-addressed generated-media dedupe
- per-shot export sidecars
- project-level JSON export manifest with shot ordering and scene grouping

Exit criteria:

- user can produce and lock scene video shots into the project folder
- user can export a self-describing production package without reopening Kineto
- project disk use can be controlled without collecting locked artifacts or required ancestry

This is the first useful alpha boundary.

## P5 — Voice and preferences

Deliverables:

- TTS abstraction
- explicit TTS capability model
- voice casting
- dialogue duration metadata
- voice generation
- director preference profile
- preference updates from candidate choices

Exit criteria:

- characters have stable voices
- future shot proposals reflect user preferences

## P6 — Ecosystem and advanced interchange

Deliverables:

- OTIO/EDL export
- custom provider plugins
- MCP tool connections
- optional local-web/daemon transport **only if a demonstrated use case requires it**
- schema-generated binary transport for that mode; desktop FFI remains unchanged
- external asset integrations

## Explicitly deferred

Do not build before the core production workflow proves useful:

- timeline editor
- transitions
- color grading
- compositing
- subtitle editor
- music editor
- cloud project hosting
- collaboration server
- model marketplace
- training/ranking models from user preference data
