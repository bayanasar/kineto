# Native Desktop Boundary

## Scope

Kineto desktop is one local application with Flutter for presentation and Rust for machine-local production work. The desktop boundary is typed FFI, not RPC.

This document describes the active architecture established by ADR-0005.

## Current toolchain

The bootstrap targets the current 2026 native stack:

- Flutter 3.47+
- Dart 3.13+
- Dart `@Native` FFI declarations
- Flutter/Dart build hooks and Code Assets
- Cargo, invoked directly by the build hook
- Rust 1.98.1, edition 2024

The Rust toolchain is pinned rather than floating on `stable` so native builds are reproducible.

## Boundary

```text
Flutter widgets / state
        │
        │ direct typed FFI calls
        ▼
apps/kineto/rust      (thin ABI shim)
        │
        ├──► crates/kineto-core     (domain/runtime code)
        └──► crates/kineto-project  (canonical project IO)
```

There is no desktop:

- JSON-RPC
- stdio protocol
- localhost server
- session token
- engine executable lookup
- duplicate request/response schema

Native ABI v12 keeps the shot-indexed demo control surface and opaque canonical-project session, keeps the panic/version guards introduced in v7, and keeps v8's project-session ownership rules. v10 added the single-writer busy status (`117`), v11 the explicit legacy-hash migration entry point with its refusal status (`118`), and v12 the lock-unavailable status (`119`). Project creation is explicitly a UTF-8 text constructor, and project metadata is copied into caller-owned buffers instead of exposing pointers into Rust `String` storage. TOML, JSON, serde values, and whole project models do not cross FFI.

The demo snapshot remains one packed `u64` containing generated/locked/stale flags, legacy-migration-pending and invalid-hash flags, candidate count, selected candidate, shot direction, generation revision, and superseded count. This compact demo ABI is disposable scaffolding, not the final project model.

## Build and packaging

`apps/kineto/hook/build.dart` runs as part of Flutter `run`, `build`, and native-aware tests. It verifies the pinned `rustc` and the requested target, runs `cargo build --release --locked` for `apps/kineto/rust`, and registers the result as the Code Asset consumed by the Dart native bindings. Cargo intermediates are shared across hook configurations; the reported library is copied into each configuration's own output directory.

The hook verifies Rust but never provisions it. It sets `RUSTUP_AUTO_INSTALL=0`, so a missing toolchain or target fails the build with the install command instead of being installed as a side effect. Flutter runs hooks with a filtered environment that keeps `HOME`, `USERPROFILE`, and `PATH` but drops `RUSTUP_HOME` and `CARGO_HOME`, so the pinned toolchain must live in rustup's default home (`~/.rustup`). CI provisions it there for the same reason.

Dart resolves `@Native` symbols against that Code Asset. No platform-specific `DynamicLibrary.open()` path and no `PATH` lookup is required.

The Rust shim declares both `staticlib` and `cdylib` outputs so the build hook can choose the appropriate artifact for the target platform.

## Ownership

Opaque Rust state crosses the ABI only as pointer-sized handles.

Engine ownership:

```text
kineto_engine_create  -> caller owns one handle
kineto_engine_destroy -> consumes that handle exactly once
```

Canonical project ownership:

```text
kineto_project_create_text/open -> writes one opaque session through an out pointer
kineto_project_destroy          -> consumes that session exactly once
```

A read-write project session is the project's only writer. Opening one takes an OS advisory lock on `.kineto/session.lock` and holds it until `kineto_project_destroy`; a second read-write open, from this process or another, fails with status `117` (`KinetoProjectException.busy`) instead of writing from its own stale copy of the state. The lock fails closed: if the lock file cannot be created or the filesystem cannot lock it, the read-write open fails with status `119` (`KinetoProjectException.lockUnavailable`); there is no unlocked read-write session. Read-only sessions (older project formats) do not take the lock. The lock file is derived state under `.kineto/`; deleting it loses nothing canonical.

Dart wraps these handles in `KinetoEngine` and `KinetoProjectSession`, performs deterministic `close()`, and installs a `NativeFinalizer` as a safety net.

Create/open input buffers are caller-owned and borrowed only for the native call. Rust does not retain those pointers. Project ID and title are returned with caller-buffer copy functions: native first reports the required byte length, then copies UTF-8 bytes into Dart-owned memory. No pointer into mutable Rust string storage escapes the call.

No Rust `String`, `Vec`, trait object, allocator-owned transfer buffer, or serde value crosses ownership domains. Demo indexes/enums/status/snapshots are fixed-width values; project text uses explicit UTF-8 pointer/length pairs with documented lifetimes.

Rust panics must not unwind across the C ABI. Fallible calls return explicit status/error values owned by the bridge. Do not use a panic as a Dart-visible error transport.

Every exported function runs its body inside `guarded`, which catches an unwind, emits a best-effort diagnostic to stderr, and returns an ordinary fallback — a null handle, a zero value, or status code `99` (engine) / `199` (project). The diagnostic is observability only; callers still receive normal ABI values.

## ABI compatibility

`kineto_engine_abi_version()` names the revision of the whole exported surface: symbol set, packed snapshot layout, and status codes. `lib/native/kineto_engine.dart` declares the revision it was written against as `kinetoExpectedNativeAbiVersion`, and every entry point into the bindings — `KinetoEngine.open()`, `KinetoProjectSession.createTextProject`, `KinetoProjectSession.open` — checks the loaded library before doing anything else, throwing `KinetoAbiMismatchException` on a mismatch.

Bump the Rust constant and the Dart constant in the same change whenever any of the following moves: an exported symbol's name or signature, the packed `u64` demo layout, or the meaning of a status code. A committed test asserts the two constants agree against the bundled library, so drift fails the build rather than silently mis-decoding state.

## Text project creation

`kineto_project_create_text` is deliberately narrow. Every textual argument and the source body must be valid UTF-8, and the source is stored as `source/story.txt`. PDF, EPUB, images, and other binary/document ingest must use separate import paths with their own parsing and ownership semantics; they must not be tunneled through the text constructor.

`CanonicalProject::create` remains byte-capable below the FFI shim so future importers can write canonical source bytes without changing the storage primitive.

## Data movement

The bridge is a control/state boundary, not a media pipe.

Preferred forms, in order:

1. IDs, enums, small fixed-width structs, and opaque handles
2. project-relative paths / asset IDs for large persisted media
3. caller-owned buffers with explicit capacity/length contracts for small variable-size metadata
4. native-backed/external typed buffers when a UI operation truly needs bulk memory
5. copying only when ownership/lifetime simplicity is worth the measured cost

Do not base64 media, stringify structured state, or repeatedly parse durable project JSON to satisfy hot UI queries. Rust should keep validated working state in memory and persist canonical files at explicit durability boundaries.

Human-readable TOML/JSON/Markdown remain appropriate for **project storage** because portability and inspectability are goals there; they are not used as the in-process call format.

The two-shot demo keeps its approval state in canonical project files — `scenes/scene_001/shots/shot_00N/shot.json`, `artifacts.json` and `selection.json` — inside the project directory. The earlier machine-level demo sidecars are retired; deleting `.kineto/` loses no approval state.

## Concurrency model

Each open project session is owned by one Rust worker thread (`kineto_core::session::ProjectSession`). The worker is the only code that touches the session's `CanonicalProject` and its cached shot workflows, so project state is never shared between threads and the domain types need no internal locking.

- Commands are queued and run strictly in submission order on the worker. Submitting a command never waits for it to run.
- After every command the worker publishes the affected shot's snapshot. Snapshot and metadata reads return that published state; they never do I/O and never wait for a running command.
- Outcomes reach the caller through a sink invoked on the worker thread, after the snapshot is published.
- The handle is `Send + Sync`: any thread may submit commands or read published state. Only the owner closes it. Closing stops delivery before it returns, discards queued commands that have not started, lets the running command finish, and releases the project and its single-writer lock before returning.
- A panic inside a command is caught. The shot's in-memory state is discarded and reloaded from its canonical files before its next use.

A worker thread, not an async runtime, because every current operation is blocking local filesystem I/O. One thread per session gives ordered execution off the UI thread without an executor. A shared async runtime arrives with network provider I/O, as below, not before.

## Async and events

There is no async runtime yet; the session worker above owns filesystem work.

When provider jobs land:

- one Rust-side async runtime owns IO-bound orchestration
- CPU-bound work is moved off async executor threads into bounded compute work
- Flutter receives coarse state/events, not per-byte/per-frame chatter
- the bridge should expose typed futures/streams/callbacks without serializing through JSON

Before selecting a larger bridge/codegen layer, benchmark the actual API surface and memory/copy behavior. `flutter_rust_bridge` 2.13 is a viable 2026 option for typed async/stream APIs, but it is not required for the current small control surface.

Likewise, `std::thread::available_parallelism()` is not a scheduler policy. A single integer does not model Apple Silicon performance/efficiency cores, QoS, provider/GPU queues, memory bandwidth, or thermal limits. Concurrency is chosen from measured workload behavior rather than surfaced as a generic "use all cores" knob.

## Crash semantics

In-process Rust terminates with the Flutter process on a fatal process crash. That is intentional.

Long-job reliability is implemented through persisted intent and reconciliation, not by assuming a child process survives the UI. Remote provider jobs are reattached by provider job ID; local subprocess work is marked interrupted/recoverable according to capability.

## Future local-web / daemon mode

Do not stretch this FFI ABI into a network protocol.

If a separate service becomes a demonstrated requirement, add a transport adapter over `kineto-core` with a schema-generated binary protocol and explicit local security. That transport has its own versioning and authentication lifecycle.
