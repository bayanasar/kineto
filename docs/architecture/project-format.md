# Project Format

## Goal

A Kineto project is a normal directory that remains understandable and recoverable without the application.

Canonical production truth lives in ordinary project files. `.kineto/` is a rebuildable local index/cache plus runtime state; it is never the only copy of a durable production decision.

Example:

```text
my-film/
├── project.toml
├── source/
│   └── novel.txt
├── screenplay/
│   ├── screenplay.md
│   └── screenplay.json
├── bible/
│   ├── world.json
│   ├── style.json
│   └── continuity.json
├── characters/
│   └── alice/
│       ├── character.json
│       ├── candidates/
│       │   ├── 01J7X....png
│       │   └── 01J7Y....png
│       ├── refs/
│       └── voice.json
├── locations/
├── props/
├── scenes/
│   └── scene_001/
│       ├── scene.json
│       ├── shots/
│       │   └── shot_001/
│       │       ├── shot.json
│       │       ├── keyframes/
│       │       ├── videos/
│       │       └── artifacts.json
│       └── voice/
├── prompts/
└── .kineto/
    ├── project.db
    ├── cache/
    └── logs/
```

Candidate filenames use stable artifact IDs or content-addressed names. Selection is metadata; there is no canonical `selected.png` or `selected.mp4` rename step.

## `project.toml`

Minimum fields:

```toml
format_version = 1
project_id = "01J..."
title = "Untitled"
created_at = "2026-09-07T00:00:00Z"

[source]
kind = "text"
path = "source/novel.txt"

[defaults]
language = "zh"

[workflow]
recipe_id = "default-film"
recipe_version = 1
```

Provider secrets must never be stored in `project.toml`.

## Canonical ownership

Durable production state must survive deletion of `.kineto/`.

## Durability of canonical writes

A canonical write is not finished when `write` returns. Project creation stages the whole project in a sibling directory, fsyncs each file and each staged directory, publishes it with one rename, and then fsyncs the parent directory so the rename itself is durable. A crash therefore leaves either no project or a complete one — never a directory whose bytes exist under a name nothing points at.

Creation refuses an existing target rather than merging into it. A directory that appears between the existence check and the rename is a residual race; on Unix the rename can only consume an empty directory, so no authored content can be lost to it.

Canonical files own:

- artifact IDs and per-artifact schema versions
- lifecycle state: draft / candidate / selected / locked / superseded
- selected/locked pointers
- provenance and dependency edges
- content and input hashes
- production memory and continuity state
- generation metadata needed to explain accepted output

`.kineto/project.db` may cache/index this state and may retain in-flight runtime job bookkeeping for crash recovery. Manual deletion of `.kineto/` can discard active runtime bookkeeping, but must not change which screenplay, character, style, keyframe, or video is selected or locked.

A full canonical project scan must be sufficient to rebuild all durable production state in `project.db`.

## Selection manifests

Selection points to stable artifact IDs rather than renaming files.

```json
{
  "selected_artifact_id": "01J7X...",
  "candidate_artifact_ids": ["01J7X...", "01J7Y..."]
}
```

Changing selection changes this metadata only. Candidate bytes, paths, and content hashes remain stable.

## Versioning and forward compatibility

Project format version and desktop native ABI version are independent. A project can outlive many application/ABI revisions, so storage compatibility must not be inferred from the FFI version.

Every typed artifact record carries its own `schema_version` so artifact schemas can evolve independently.

Compatibility is asymmetric:

- `format_version > supported`: refuse to open/write; this build cannot safely interpret future storage semantics
- `format_version == supported`: open read-write
- `0 < format_version < supported`: open read-only until an explicit migration is selected and completed
- `format_version == 0`: invalid

Hard rules:

- serialization and canonical mutation require the current writable format
- migrations are explicit, versioned, testable, opt-in, and create a backup before mutation
- an older readable project must never be silently rewritten as the current format
- unknown fields encountered in a supported schema must be preserved and written back unchanged
- round-trip tests over golden projects guard against silent field loss

## Executable schema contracts

Committed JSON Schemas are the source of truth for canonical structured storage. Rust types and serializers must satisfy those schemas; the schemas are not generated from Rust.

`kineto-schema-contracts` is a test-only workspace package that executes the subset of JSON Schema draft 2020-12 used by this repository. It validates the committed golden project and real persistence output from `CanonicalProject`, `ShotWorkflow`, and `JobRuntime`. The harness fails closed on schema keywords, formats, or regular-expression patterns it does not implement, so extending a schema cannot silently weaken validation.

When adding or changing a canonical structured file:

- add or update its committed schema under `schemas/`
- add the file to the golden-project contract inventory when it belongs in the minimal project
- ensure production serialization is exercised against the same schema
- extend the contract harness before using a JSON Schema keyword, format, or pattern it does not already support

Plain source payloads such as `source/story.txt` are not JSON-schema documents; the golden-project contract still inventories them and verifies the declared source is readable, valid UTF-8 text, and non-empty.

## Production memory

Do not treat "memory" as only embeddings.

Kineto has two distinct memory systems.

### Semantic memory

Used for retrieval:

- source text
- screenplay
- world notes
- director notes
- prior scene context

### Production memory

Explicit deterministic state:

```text
Character Alice
├── canonical identity
├── approved reference images
├── wardrobe state
├── hair/makeup
├── voice
├── provider reference IDs
├── forbidden changes
└── continuity notes
```

The second category is more important for visual continuity than vector search.

## Artifact identity and provenance

Every generated or authored production item has a stable artifact ID.

Representative record:

```json
{
  "artifact_id": "01J...",
  "schema_version": 1,
  "type": "video_candidate",
  "created_at": "...",
  "status": "candidate",
  "source_artifacts": ["01J_PARENT..."],
  "content_hash": "sha256:...",
  "input_hash": "sha256:...",
  "provider": "example-video",
  "model": "model-x",
  "compiler_version": "video-prompt/1",
  "compiled_prompt": "verbatim provider input used for this generation",
  "prompt_hash": "sha256:...",
  "dropped_constraints": [],
  "degradation": "none",
  "media_state": "present",
  "retention_class": "recent"
}
```

`prompt_hash` covers the structured intent, compiler version, and compiled provider input. The compiled prompt itself is stored verbatim so reproduction and diagnosis do not depend on a newer compiler implementation.

## Input hash and staleness

Staleness is computed, not manually toggled.

Conceptually:

```text
input_hash = H(
  semantic parent artifact IDs,
  semantic parent content hashes,
  structured intent,
  provider,
  model,
  generation params,
  compiler version
)
```

Dependency edges attach at the smallest meaningful production unit: scene, character, style, location, prop, shot, keyframe, and so on. A cosmetic change to screenplay formatting must not invalidate unrelated shots.

The UI must be able to explain which semantic parent or field caused a stale result.

## Retention and media presence

Artifact metadata and artifact bytes are separate concerns.

Retention classes:

- `pinned` — locked artifacts and required ancestry; never automatically collected
- `recent` — retained candidate window per shot/entity
- `archived` — canonical metadata retained while regenerable media bytes may be evicted
- `collectable` — unreferenced/superseded and outside the retention window

`media_state` must represent at least `present` and `evicted`.

Future GC must be dependency-aware, provide dry-run reporting, and never collect an ancestor required by a pinned artifact. Generated media should use content-addressed deduplication where practical.

## Scene schema

Representative shape:

```json
{
  "scene_id": "S003",
  "schema_version": 1,
  "location": "alice_apartment",
  "time": "night",
  "characters": ["alice", "bob"],
  "dramatic_goal": "Alice confronts Bob",
  "shots": ["S003_SH01", "S003_SH02"]
}
```

## Shot schema

Representative shape:

```json
{
  "shot_id": "S003_SH01",
  "schema_version": 1,
  "duration_target": 5.5,
  "characters": ["alice"],
  "framing": "medium_close_up",
  "lens": "50mm",
  "camera_motion": "slow_dolly_in",
  "action": "...",
  "dialogue": "...",
  "emotion": "...",
  "continuity_from": "S002_SH05"
}
```

The intent stays expressive even when a provider cannot represent every field. Provider compilation must report dropped constraints rather than silently deleting them.

LLM output should be schema-validated structured data. Markdown is a presentation format, not the canonical machine state.
