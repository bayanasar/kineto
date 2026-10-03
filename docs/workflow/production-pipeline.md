# Production Pipeline

## Principle

Generation is not a single linear call. It is a sequence/graph of artifacts with explicit human approval boundaries.

The stages below define Kineto's **default filmmaking recipe**, not a hardcoded engine state machine. Projects may import an existing screenplay, skip voice, insert extra approvals, or use a future custom recipe. The engine should execute validated step/dependency data rather than require code changes for every workflow variation.

A recipe records an ID/version in the project so provenance includes the workflow definition used.

Recipes follow `schemas/recipe.schema.json`. Each step names what it produces (one of the artifact kinds the engine knows how to read back from the project: source, story analysis, screenplay, bible, style, cast, shot plan, keyframe, video), the steps it follows, and whether the director approves what it made. Every step comes after the steps it follows, so a recipe has no cycle. The build ships `recipes/default-film-1.json`, the stages below without voice and preference learning. `project.toml` names the recipe by `recipe_id` and `recipe_version`, and a recipe the build does not ship is refused rather than replaced by the default.

Where a project stands is computed from its canonical files on every read (`kineto_core::workflow::status`), so deleting `.kineto/` changes no step. A step is *waiting* while a step it follows is not done and nothing is made, *ready* once they are, *awaiting approval* once made, and *done* once approved, or once made where no approval is asked. Each step also names what changed since its work was made: the source under an analysis, the analysis under the locked screenplay, the screenplay scenes under the locked bible, the bible entry or style under a cast, and the locks under a locked keyframe.

## Stage 1 — Ingest

Inputs:

- plain text
- Markdown
- document-imported text
- later: EPUB/PDF/document connectors

Outputs:

- normalized source
- source metadata
- optional semantic index

## Stage 2 — Story analysis

Extract:

- characters
- locations
- time period
- major events
- relationships
- tone
- themes
- candidate scenes

The result is structured project state, not only prose.

## Stage 3 — Screenplay

The screenplay agent produces:

- screenplay JSON
- human-readable screenplay Markdown

The user can:

- edit
- regenerate sections
- split/merge scenes
- reorder scenes
- approve and lock a revision

Downstream generation references explicit scene/sub-artifact revisions rather than invalidating every shot for cosmetic screenplay changes.

## Stage 4 — World and character bible

Generate reusable production entities:

- characters
- locations
- props
- visual constraints
- continuity constraints

Character records contain both semantic descriptions and concrete visual references.

## Stage 5 — Style exploration

Use tested pre-prompts/templates to generate a small number of style directions.

User actions:

- select
- refine
- reject
- lock

A locked style becomes part of the production context for later visual generations.

## Stage 6 — Character casting

Generate candidate appearances per important character.

Once selected, create a character reference pack rather than storing only one portrait.

Recommended references:

- canonical portrait
- front view
- left/right profile
- full body
- costume
- expression samples

The selected references become production memory. Selection points to stable candidate IDs; files are not renamed to `selected.*`.

## Stage 7 — Voice casting

Voice belongs to the character definition when dialogue/voice is used by the recipe.

The user can choose:

- voice provider
- voice identity
- tone
- pacing
- accent/style constraints

Dialogue duration may affect shot design, so voice planning happens before final shot rendering when applicable, even if audio files are generated lazily.

How it works now (`kineto_core::voice`):

1. Voice casting follows the character's locked cast.
2. The director lists the speech provider's voices; listing is free.
3. Chosen voices audition the character's first line, or a line the director writes. Each audition is paid speech, priced before it is asked for.
4. The director selects one audition and locks it. The lock fixes the provider, the voice ID, the language, the delivery, the rate and the pitch.
5. Lines are spoken with the locked voice from then on. When the character's bible entry or cast changes, the voice is stale until it is cast again.

## Stage 8 — Scene breakdown and shot planning

For each scene, generate explicit shots with:

- dramatic purpose
- shot type
- framing
- lens
- camera motion
- subject action
- dialogue
- emotion
- duration target
- continuity requirements

The intent remains expressive even if a target provider cannot preserve every field.

## Stage 9 — Keyframes first

Do not immediately render multiple expensive videos.

Default flow:

```text
Shot plan
  ↓
3–4 keyframe candidates
  ↓
user selects/refines
  ↓
selected keyframe artifact ID
  ↓
video generation
```

This reduces cost and improves visual consistency.

## Stage 10 — Video candidates

The prompt compiler builds provider-specific input from:

```text
shot intent
+ character locks
+ location locks
+ style lock
+ previous shot state
+ selected keyframe
+ continuity constraints
```

Compilation reports applied and dropped constraints plus degradation severity before generation. Identity-affecting degradation can be blocked by policy.

Before paid batches, the user can see cost/call-count preflight where provider metadata supports it.

The user may:

- select
- refine
- regenerate
- branch a variant
- lock

## Stage 11 — Preference learning

The first version should not train a model.

Maintain an interpretable director preference profile:

```json
{
  "camera_motion": {
    "prefer": ["slow dolly"],
    "avoid": ["handheld"]
  },
  "lighting": {
    "prefer": ["low-key", "practical"]
  },
  "performance": {
    "intensity": 0.35
  }
}
```

Candidate choices can update this profile after an LLM/vision comparison explains what differs between accepted and rejected candidates.

The profile informs future candidate generation.

## Stage 12 — Export

Kineto exports production-ready shot packages, not a finished edit.

The minimum deliverable is not just bare numbered files. Every exported media asset has a metadata sidecar, and the package has a project-level manifest with shot order and scene grouping.

The package (`kineto_core::export`) is a folder the director names, made in a folder they choose:

```text
dailies/
├── manifest.json
├── scene_001_shot_001.mp4
├── scene_001_shot_001.json
├── scene_001_shot_001_first_frame.jpg
├── scene_001_shot_002.mp4
├── scene_001_shot_002.json
└── scene_001_shot_002_first_frame.jpg
```

Voice files join it when P5 brings voice.

Each shot's sidecar (`schemas/export-shot.schema.json`) says which take was locked and what made it:

- the video's file, artifact ID, content hash, provider, model, clip length and the parameters it was asked with;
- the keyframe it starts from, by artifact ID and hash, and its image beside it when it has one;
- the prompt exactly as the provider got it, with its hash, compiler version, applied constraints and the dropped ones with why;
- the shot's planned intent: action, framing, lens, camera motion, mood, dialogue, characters, target duration and continuity;
- the director's notes and the take it branched from.

The project manifest (`schemas/export-manifest.schema.json`) holds the project and recipe, the export time, the scenes in screenplay order with location and time, each shot's place among the shots in play and its files, what is missing from a partial package and why, and the total runtime.

Only locked videos that still follow their shot are exported, each checked against its recorded hash. When a shot is not ready, the export says which and why, and writes nothing unless the director asks for the ready shots only. The package is written beside its destination and renamed into place, so an interruption never leaves half a package under its name, and exporting the same project again gives the same bytes. Kineto does not stamp a lock time on its records, so a sidecar does not carry one; the manifest records when the package was made.

Later ecosystem exports can add:

- OTIO
- EDL
- NLE-specific metadata
