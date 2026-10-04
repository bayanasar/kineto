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
│   ├── revisions.json
│   ├── screenplay_r0001.json
│   └── screenplay.md
├── bible/
│   ├── revisions.json
│   ├── bible_r0001.json
│   └── style.json
├── references/
│   └── references.json
├── shot_plan/
│   ├── revisions.json
│   └── shot_plan_r0001.json
├── style/
│   ├── samples.json
│   └── samples/
│       └── style_s0001.jpg
├── characters/
│   └── alice/
│       ├── casting.json
│       ├── character.json
│       ├── candidates/
│       │   ├── cast.alice.0001.jpg
│       │   └── cast.alice.0002.jpg
│       ├── refs/
│       │   └── cast.alice.0003.jpg
│       ├── voice_auditions.json
│       ├── voice/
│       │   └── voice.alice.0001.mp3
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
│           ├── lines.json
│           └── line.scene_001.01.0001.mp3
├── prompts/
└── .kineto/
    ├── project.db
    ├── cache/
    └── logs/
```

Candidate filenames use stable artifact IDs or content-addressed names. Selection is metadata; there is no canonical `selected.png` or `selected.mp4` rename step.

## Document revisions

The story analysis (`story/analysis/`), the screenplay (`screenplay/`) and the world and character bible (`bible/`) are document artifacts kept as revisions. Each revision is a JSON file named by its artifact ID (`story_analysis_r0001.json`, `screenplay_r0003.json`); `revisions.json` in the same directory (`revisions.schema.json`) records each revision's status (`candidate`, `locked`, `superseded`), origin (`generated` by a model or `edited` by the director, with `based_on`), content hash, input hash, provider, model, prompt provenance and dependencies. A new revision supersedes the earlier candidates; locking one supersedes the previously locked revision; nothing is deleted. A revision whose file no longer matches its hash is refused rather than read. The story analysis body follows `story-analysis.schema.json`, which is also the schema the model is asked to satisfy; a revision depends on the source revision it read (`source_rNNNN`).

A screenplay revision follows `screenplay.schema.json`: a title, a cast whose IDs are story analysis character IDs, and scenes. Each scene has a stable ID, a heading as printed, a setting, a story analysis location ID, a time, the cast present, a dramatic goal and blocks of action, dialogue (with its speaker and an optional parenthetical) and transitions. A generated screenplay depends on the story analysis revision and the source revision it was written from. One scene can be written again on its own (`screenplay-scene.schema.json`, the same shape as a scene in the screenplay): the result is a new revision `based_on` the one it reworked, with every other scene carried over unchanged. `screenplay.md` is rendered by Rust from the current revision (the locked one, otherwise the latest candidate); it is derived, never read back, and deleting it loses nothing, since rendering again reproduces it byte for byte.

The director's edits never overwrite: saving an edit writes the next revision (`origin: edited`, `based_on` the revision edited, carrying that revision's analysis and source dependencies), and is refused when the edit did not start from the latest revision, so a revision the director has not seen is never replaced; saving without a change writes nothing. Locking a revision makes it the screenplay production follows, while newer candidates remain the director's working copy. All of this lives in `screenplay/`, so deleting `.kineto/` changes no revision and no lock.

Downstream work depends on a scene, not on a screenplay revision: the dependency names `screenplay_scene.<scene_id>` and the scene's content hash. That hash covers what the scene says and ignores layout — file formatting, the scene's position, and whitespace at line ends or around paragraphs — so a new revision leaves work on unchanged scenes fresh. Against the current screenplay, such a dependency is fresh, changed, or removed.

## World and character bible

The bible (`bible/bible_rNNNN.json`, `bible.schema.json`) is the production memory visual work follows, written from the locked screenplay. It describes the world (setting, period, visual rules and continuity rules for the whole film), and has an entry for every cast member who appears in a scene and every location a scene is set in, plus props:

- a character has a description, an appearance concrete enough to draw the same person every time, a usual wardrobe with wardrobe changes by scene, forbidden changes (what must never change between shots) and continuity notes;
- a location has a description, an appearance, forbidden changes and continuity notes;
- a prop has a description, an appearance, the characters who handle it and the scenes it appears in.

Character IDs are screenplay cast IDs; location IDs are story analysis location IDs, so a place that is seen without a scene being set in it can be described too; scene IDs are screenplay scene IDs. A model's bible is checked against all of that before it is kept, and asked for once more with the reason when it fails. Entry IDs are at most 112 characters, so an entry can later be depended on as `bible_character.<id>` and the like.

A bible depends on the story analysis and source the locked screenplay was made from, on each of that screenplay's scenes (`screenplay_scene.<id>`, semantic), and on the screenplay revision itself as a cosmetic dependency that only records where the scenes were read. It is stale when the locked screenplay changes a scene it read, removes one, adds one, or is made from another analysis or source; formatting changes and locking a revision whose scenes say the same thing leave it fresh. Staleness names each cause. Edits and locks follow the screenplay's rules (a new revision, `based_on`, refused when not started from the latest; one locked revision), and both hold the bible to the locked screenplay and its analysis exactly as a model's bible is held, so no revision that names a character, location or scene they lack is saved or locked. A stale bible that names only what they define can still be locked, since the director decides whether it holds, and staleness keeps saying why it may not.

## References

A reference is text the director brought in from an outside source, an MCP tool (ADR-0007), as material for the bible. `references/references.json` (`references.schema.json`) keeps each one as `reference.NNNN` (an ID is never given again after a reference is removed; `next_number` says the next), with the registered server and tool it came from, the arguments it was asked with, its text and the text's hash, and what it is about: `bible_world` or bible entries by the IDs work depends on them by (`bible_character.<id>`, `bible_location.<id>`, `bible_prop.<id>`), which need not exist yet. No time is recorded. The director attaches, detaches, edits or removes references; a tool is called only when the director asks.

When the bible is written, every reference attached to anything goes into its prompt after the screenplay, in a `<references>` block, each as a `<reference>` element naming its ID, source and what it is about, its text escaped so it cannot end its element, under the same rule as the source: material, not instructions. The bible depends on each one it read, semantically, by the hash of that element, so it is stale (cause `reference`, naming it) when the reference's text or what it is about changes, when it is removed, or when one it never read is attached. Without references the request is what it was before references existed.

## Style exploration

A style direction is explored with sample images. Each sample is one paid image call on a scene of the locked screenplay (the director's choice, or the scene with the most people), described from the bible when there is one, in one of a few tested presets compiled by Rust (`style-sample/1`), optionally with the director's note. `style/samples.json` (`style-samples.schema.json`) records every sample as `style_sNNNN` in order:

- its preset, the preset's version and directives verbatim, the note, and the sample it refines;
- the scene it shows, and its image under `style/samples/`;
- its content and input hashes, provider, model and prompt provenance with applied and dropped constraints;
- its dependencies (the scene's `screenplay_scene.<id>` and the bible revision).

A sample is a `candidate`, the one `selected`, or `rejected`; nothing is deleted. Locking the selected sample writes `bible/style.json` (`style.schema.json`): the sample's stable artifact ID and image hash, and the style itself (preset, version, directives and note) verbatim, so later visual work reads the locked style without the preset table of whichever build it runs on. Every change is one atomic file write, and deleting `.kineto/` changes no sample and no lock.

Samples are write-ahead jobs under their own runtime partition, `minimax-style`, with the receipt, refusal and fail-closed rules of keyframes. What the sample record needs beyond the receipt is kept beside it as a plan before the call, so an interrupted sample is completed from disk when the project opens or before the next sample, never by calling again; a sample whose call may have been accepted without its result being kept makes the next one wait for the director to risk paying twice.

## Casting

A character of the bible is cast with paid images in the locked style. Portrait candidates are written from the character's bible entry (appearance, wardrobe, forbidden changes) and kept under `characters/<id>/candidates/`. The director selects one portrait or rejects others. From the selected portrait a reference pack is made under `characters/<id>/refs/`, each image sent with the portrait as the provider's identity reference (MiniMax `subject_reference`):
- a front view and left and right profiles;
- a full-length view and a costume reference;
- two expressions (a smile and a tense close-up).

Which kinds and frames the pack holds follows the provider's declared capabilities: a full-length view wants 9:16 and is made at the nearest frame the provider takes, reported as a dropped constraint; with no identity reference there is no pack, since it would not show the same person. Prompts never truncate; what does not fit is dropped and reported (`cast-portrait/1`, `cast-reference/1`).

A portrait can also be the director's own image: imported from a path they chose (the only read outside the project), copied in by its hash, free, and recorded with provider `import`, compiler `import/1` and the bible entry and locked style it was brought in for as dependencies. It must be a JPEG, PNG or WebP of at most 20 MB, and a format and size the session's image provider takes as an identity reference when it declares one. Importing the same image again records nothing new. From there it is selected, packed and locked like a generated portrait.

`characters/<id>/casting.json` (`casting.schema.json`) records every image as `cast.<id>.NNNN` in order, with its kind, its status (portraits only), the portrait it was made from (references only), its hashes, provider, model, prompt provenance and dependencies. Locking writes `characters/<id>/character.json` (`character.schema.json`):
- the canonical portrait and the newest reference of each kind made from it, by artifact ID, content hash and path;
- the provider reference: provider, the request field it travels in, the role, and the image sent;
- the forbidden changes the bible names;
- the portrait's dependencies.

A casting image depends on the character's bible entry (`bible_character.<id>`, the hash of the entry alone, so editing another character leaves it fresh) and on the locked style (the locked sample's ID and the hash of the lock); a reference also depends on its portrait. A locked cast is stale when either parent changed, went or came, and staleness names each. Images are paid write-ahead jobs in the `minimax-cast` partition with the rules of style samples: no new image of a character is paid for while one of that character's may have been charged without its result, until the director risks paying twice for that character. Images are numbered per character, so the decision retires only that character's unresolved images; another character's neither waits for it nor is retired by it. Asking for portraits or a pack says what should exist, not how much to add: an audition of `n` keeps the unrejected portraits already made from the same entry and style and makes only the rest, and a pack makes only the kinds the selected portrait's pack lacks for them. So a request cut short by a crash or a failure, asked again, pays only for what it had not made (and, on the director's decision, for a call whose answer was lost), while other takes of a kind are an explicit remake.

A character's voice is cast after its look. `characters/<id>/voice_auditions.json` (`voice-auditions.schema.json`) records every audition in order as `voice.<id>.NNNN`, with its audio under `voice/`. Each audition is a voice from the speech provider's list saying a line, and records:

- the voice: provider voice ID, ISO 639-1 language, delivery, rate in thousandths and pitch in semitones;
- the voice's listed name;
- the line;
- its status: candidate, selected (at most one) or rejected;
- its hashes, provider, model and parameters;
- its measured length;
- its dependencies;
- its `media_state`.

Locking the selected audition writes `characters/<id>/voice.json` (`voice.schema.json`):

- the audition's ID;
- its provider, model, voice and voice name;
- its audio as a sample, by content hash and path;
- its dependencies.

An audition depends on the character's bible entry and its locked cast (the canonical portrait's ID and the hash of `character.json`, `field: character.<id>`). A locked voice is stale when either changed, went or came. A selected audition made for the character as it was is not locked (`StaleSelection`).

Rules for asking:

- An audition says the director's line, or the character's first line in the current screenplay, or a neutral line in the voice's language.
- A voice already heard saying that line, and not rejected, is not asked again.
- Auditions are paid speech in their provider's `voice` partition (`<provider>-voice`), numbered per character, with the rules of casting images.
- Auditions are the character's media (`MediaKind::Voice`). The locked one is pinned, and a rejected one outside the recent takes may be collected.

Work spoken in a character's voice records the lock as a dependency: the audition's ID and the hash of `voice.json`, `field: voice.<id>`.

## Director preferences

`preferences/` holds the director's preference profile as document revisions (`preferences_rNNNN.json`, `preferences.schema.json`, kind `preferences` in `revisions.json`).

- Every change is a revision, and the latest is in force; nothing is locked.
- An entry names a dimension, `prefer` or `avoid`, a value in the director's words, an optional strength in thousandths, and its source: the director, or a suggestion with its reason and the candidates that showed it.
- Work made with the profile records its revision as a cosmetic dependency (`field: preferences`). It is excluded from input hashes and from staleness. It shows only that the profile has changed since.

## Dubbing

`scenes/<scene>/voice/lines.json` (`lines.schema.json`) records every take of the scene's lines in the order made, as `line.<scene>.<NN>.<TTTT>`: line `NN` of the scene (its place among the scene's dialogue blocks), take `TTTT` of that line. The audio sits beside it.

A take records:

- the line as the screenplay had it: its speaker, its words and its parenthetical;
- its status: candidate, selected (one per line) or rejected;
- the voice that spoke it, with the delivery and rate it was asked for;
- the director's note;
- its hashes, provider, model, parameters, measured length and sentence timings;
- its provenance (`speech-prompt/1`: what was applied and what was dropped);
- its dependency on the speaker's voice lock;
- its `media_state`.

`locks` names, per line, the take it is locked to.

A take goes stale when:

- the current screenplay changes the line's words (`line.text`) or its direction (`line.parenthetical`);
- the line is gone (`line.missing`);
- the speaker's voice is locked again (`voice.<character>`).

A stale selection is not locked. Takes are paid speech in their provider's `line` partition, numbered per line. Asking for `n` takes keeps the unrejected takes already made the same way and makes the rest. Takes are the scene's media (`MediaKind::Line`, owned by the scene): a locked take is pinned.

A locked line's length is its take's measured length; any other line's length is an estimate.


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

`[workflow]` may also set `identity_policy = "block"`: a keyframe that would show a character without an identity reference then waits for the director's explicit consent instead of being generated and reported.

Provider secrets must never be stored in `project.toml`.

## Source revisions

The source a project was created with is revision 1, at `[source].path`. Importing text again never overwrites it: the import is normalised (UTF-8 only, byte-order mark removed, `\n` line endings, trailing whitespace trimmed, one final newline; at most 8 MiB) and written as `source/revisions/rNNNN.txt` or `.md`, and `source/source.json` (`source.schema.json`) lists every revision with its format, content hash, character count and original file name. The latest revision is current. Importing the text of the current revision again changes nothing. Work derived from the source records the content hash of the revision it read, and a revision whose bytes no longer match its hash is refused rather than read.

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

`kineto-json-schema` executes the subset of JSON Schema draft 2020-12 used by this repository. `kineto-schema-contracts`, a test-only workspace package, uses it to validate the committed golden project and real persistence output from `CanonicalProject`, `ShotWorkflow`, and `JobRuntime`; `llm.complete` uses it to check a model's answer against the schema it was asked to satisfy. It fails closed on schema keywords, formats, or regular-expression patterns it does not implement, so extending a schema cannot silently weaken validation.

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
  "compiler_version": "video-prompt/6",
  "compiled_prompt": "verbatim provider input used for this generation",
  "prompt_hash": "sha256:...",
  "dropped_constraints": [],
  "degradation": "none",
  "media_state": "present",
  "retention_class": "recent"
}
```

`prompt_hash` covers the structured intent, compiler version, and compiled provider input. The compiled prompt itself is stored verbatim so reproduction and diagnosis do not depend on a newer compiler implementation.

Shot keyframe candidates generated by a real provider are stored as `scenes/<scene>/shots/<shot>/keyframes/<artifact_id>.<ext>`. Their records carry `media_path` (project-relative), `media_state`, and a `content_hash` computed from the file's bytes, which attach verifies before publishing. They also carry `applied_constraints` alongside `dropped_constraints`, so the record explains both what reached the provider and what did not. Demo candidates have no media and no prompt provenance. A keyframe compiled from locks records them as `dependencies`: the locked style (the locked sample's ID and the hash of `bible/style.json`'s content) and the locked cast of each of the shot's characters (the canonical portrait's ID and the hash of `character.json`'s content, `field: character.<id>`). The plan's input hash covers them. A keyframe is stale when a lock it followed changed or went, or a lock it did not follow has been made since; staleness names each, and the candidate is kept. Each candidate also records, under `intent`, the shot's intent fields it was made for (direction, purpose, action, framing, lens, camera motion, emotion, dialogue, characters, duration, continuity and `continuity_from`); a field changed since, for example by locking an edited shot plan, makes it stale as `intent.<field>`. Candidates of earlier generations are kept as superseded, so the director compares every take a shot has had; a generation refined by the director's note records `director.note` among its applied constraints.

A shot's video starts from its locked keyframe. A video generation asks for one to four candidates; each adds one `shot_video` record (`<shot_id>_vRRRR_candidate_x`, media `videos/<artifact_id>.mp4`; a video made before a generation could have several is `<shot_id>_vRRRR`) to the shot's `artifacts.json`, with a semantic dependency on the keyframe (`field: "first_frame"`, its artifact ID and content hash) and an `input_hash` derived from that dependency, the keyframe it closes on if any, the shot intent, the director's notes and the take it branches from, provider, model, parameters and compiler version. `shot.json` records the latest `video_revision`. Each candidate is its own paid job and attaches when it finishes: the first of a new generation supersedes the previous generation's videos, and the rest join it. A paid video is never thrown away: a candidate of an older generation that finishes late, or of a newer one that finishes after the director locked a video, is kept as superseded. The director chooses one of the latest generation's videos (candidate → selected; choosing again moves the choice), and locking approves the chosen one, or the only one (selected → locked); resetting the shot supersedes keyframes and videos alike. A video records the director's notes it was made with under `notes`, oldest first, and a branch the take it branched from under `branched_from`; a branch keeps that take's notes before its own. A video also records, under `intent`, the shot's intent fields when it was asked for. A video made to close on the locked keyframe of the shot that continues from it records that keyframe as a second semantic dependency (`field: "end_frame"`) and the shot under `end_frame_shot` (`scene/shot`). Locking records when, under `locked_at` (RFC 3339 UTC); videos locked before this was recorded have none. Staleness is computed: a video whose dependency no longer names the locked keyframe's bytes (`first_frame`), whose closing keyframe is no longer the next shot's locked keyframe or whose next shot no longer continues from it (`end_frame`), or whose recorded intent differs from the shot's (`intent.<field>`), is stale and cannot be locked, and its record is kept.

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

Dependency edges attach at the smallest meaningful production unit: scene, character, style, location, prop, shot, keyframe, and so on. A cosmetic change to screenplay formatting must not invalidate unrelated shots. Screenplay scenes already work this way (see Document revisions).

The UI must be able to explain which semantic parent or field caused a stale result.

## Retention and media presence

Artifact metadata and artifact bytes are separate concerns.

Retention classes:

- `pinned` — locked artifacts and required ancestry; never automatically collected
- `recent` — retained candidate window per shot/entity
- `archived` — canonical metadata retained while regenerable media bytes may be evicted
- `collectable` — unreferenced/superseded and outside the retention window

`media_state` must represent at least `present` and `evicted`. Shot artifacts, casting images and style samples all carry it; a record without one predates eviction and keeps its bytes.

The records are the authority for media (`kineto_project::media`). The catalog lists every file a record names, with its owner (a shot, a character or the style), its kind and the director's standing on it (locked, chosen, candidate, or passed: superseded or rejected). A session checks at open, and after it makes media, that every file a record calls present exists, and publishes what is gone; whether the bytes still hash as recorded is checked only when asked, since that reads every file. Both are reported, never rewritten.

Generated media is deduplicated by content: after the session makes media, files whose bytes are the same, verified by hashing both, become hard links to one another. Each record keeps its own path beside its owner; a file system without hard links keeps the copies. A finished video's download link that has expired or is refused is retrieved again on the next look at the task, without paying again.

Retention classes are computed from the records on every read, never stored (`kineto_project::media::retention`):

- `pinned`: a locked artifact (a locked keyframe or video, the locked style sample, a locked cast's portrait and pack) and every artifact it was made from, followed through dependencies however far back: a locked video's first frame, that keyframe's cast portrait, the style sample the portrait followed.
- `recent`: among the latest takes of its owner (a shot, a character, the style) and kind, or not decided on yet. How many latest takes are kept is `recent_takes` under `[workflow]` in `project.toml`, 6 by default.
- `archived`: its bytes were let go (`media_state: evicted`); its record, prompt and provenance stay, and it shows no file.
- `collectable`: superseded or rejected, older than the latest takes, and needed by nothing locked.

Collecting is dependency-aware and has a dry run. The dry run lists every collectable file with why and the bytes it frees; a file deduplicated with one that is kept frees nothing. Collecting lets go of what the dry run listed and nothing else, each item only if it is still collectable and its file, hashed again, still holds the bytes its record names, and never a pinned one; a file changed since the dry run is kept and reported as skipped. Each owner's records are marked evicted before their files are removed, so an interruption leaves at most a record that says evicted beside a file still there; opening the project removes it, if it still holds the evicted bytes. Disk use is reported for the project and per owner, each file counted once however many records name it.

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

## Shot plan

The shot plan (`shot_plan/`) is a document artifact kept as revisions like the screenplay and the bible, and follows `shot-plan.schema.json`. It has the shots of every scene of the locked screenplay, in screenplay order. Each shot has an ID unique in the plan, its purpose, a direction (`reaction`, `spatial_clarity`, `intimacy`, `tension`), a framing and a camera motion from fixed vocabularies, a lens, the action seen, the dialogue spoken (verbatim, or empty), the emotion, a target duration of 1–30 seconds, the cast IDs of those seen (only the scene's cast, each with a bible entry), continuity notes and, optionally, the earlier shot it continues from. A shot's location is its scene's. The plan is made by a model from the locked screenplay and the locked bible (`shot-plan/1`); one scene can be planned again on its own (`shot-plan-scene.schema.json`, `shot-plan-scene/1`) as a new revision `based_on` the one it reworked. An answer that misses or reorders a scene, shows anyone a scene's cast lacks, or breaks the plan's own rules is asked for once more. The director's edit of one scene's shots is a new revision made only from the latest one, held to the same checks, and locking a revision makes it the plan the shots follow. A shot's ID is its identity: a shot keeps it while it stays in the plan, a removed shot keeps it with its directory, takes and videos, and no shot added later takes it. Every new revision — a plan, a scene planned again, the director's edit — gives each shot it adds whose ID the project has used before (any plan revision names it, or a shot directory holds it) the scene's ID and the first number no shot has had, and a shot continuing from it follows. A shot the director adds in an edit is marked as added, so it is new even when the same edit removed a shot of the ID it was given; the editor also numbers it past every shot the scene had when editing began.

Locking a plan writes its shots into the project: each scene's `scenes/<scene_id>/scene.json` from the locked screenplay, and each shot's `scenes/<scene_id>/shots/<shot_id>/shot.json` with the plan's intent (purpose, direction, framing, lens, camera motion, action, dialogue, emotion, duration, characters, continuity and `continuity_from`). A shot already written keeps its generation and video revisions while its intent is replaced. Shot IDs are unique across the project, and a plan that reuses one another scene holds is refused. The shots in play are the locked plan's, in plan order, or the bootstrap scene's (`scene_001`) before one is locked; artifact IDs name their shot (`<shot_id>_g0001_candidate_a`, `<shot_id>_v0001`), so a job is placed back in its shot by looking for the scene that holds it.

A plan depends on each screenplay scene (`screenplay_scene.<id>`), on the bible entry of each cast member who appears (`bible_character.<id>`) and of each scene's location (`bible_location.<id>`), each by its content hash, and cosmetically on the screenplay and bible revisions it read. It is stale when a scene changed, went or came, when an entry it read changed, or when the screenplay or bible is no longer locked; staleness names each.

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
