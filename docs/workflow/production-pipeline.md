# Production Pipeline

## Principle

Generation is not a single linear call. It is a sequence/graph of artifacts with explicit human approval boundaries.

The stages below define Kineto's **default filmmaking recipe**, not a hardcoded engine state machine. Projects may import an existing screenplay, skip voice, insert extra approvals, or use a future custom recipe. The engine should execute validated step/dependency data rather than require code changes for every workflow variation.

A recipe records an ID/version in the project so provenance includes the workflow definition used.

Recipes follow `schemas/recipe.schema.json`. Each step names what it produces (one of the artifact kinds the engine knows how to read back from the project: source, story analysis, screenplay, bible, style, cast, shot plan, keyframe, video), the steps it follows, whether the director approves what it made, and optionally when it does not apply (`skip_when`). Every step comes after the steps it follows, so a recipe has no cycle. A skip condition is one the engine reads from the project's files and holds only once what it reads is locked; it must read what a step the skipped one follows, directly or not, makes. This build knows one, `no_main_characters`: the locked bible has no main character. The build ships `recipes/default-film-1.json`, the stages below without voice and preference learning, in which casting is skipped when there is no one to cast. `project.toml` names the recipe by `recipe_id` and `recipe_version`, and a recipe the build does not ship is refused rather than replaced by the default.

Where a project stands is computed from its canonical files on every read (`kineto_core::workflow::status`), so deleting `.kineto/` changes no step. A step is *waiting* while a step it follows is not done and nothing is made, *ready* once they are, *awaiting approval* once made, and *done* once approved, or once made where no approval is asked. Once the steps it follows are done, a step whose skip condition holds is *skipped*, names the conditions that hold, and counts as done for the steps after it. Each step also names what changed since its work was made: the source under an analysis, the analysis under the locked screenplay, the screenplay scenes under the locked bible, the bible entry or style under a cast, and the locks under a locked keyframe.

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

The director can also import an image they already have as a style sample, with their own description of its look. Locked, the description is what prompts follow, and the image goes with each keyframe as a style reference to a provider that takes one ([Project format](../architecture/project-format.md)).

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

Dubbing (`kineto_core::dubbing`):

- **What is spoken.** Each line of the screenplay is spoken in its character's locked voice, by that voice's provider; another provider cannot speak a voice it did not list.
- **The parenthetical.** It becomes a delivery when a word of it names one the provider takes (`speech-prompt/1`: `quietly` is calm, `whispering` a whisper). Otherwise it is reported as dropped.
- **Refining.** A take may be asked for with the director's delivery and rate, which replace the voice's, and a note. The note is kept with the take; the provider never hears it.
- **Choosing.** The director selects a take per line and locks it.
- **The recipe.** `default-film` v2, which new projects follow, adds a `voices` step (the voice of every character who speaks) and a `dialogue` step (a locked take of every line). Both are done at once when no one speaks; v1 projects keep v1.

Dialogue duration (`kineto_core::dialogue`):

- **Line length.** Every line of the screenplay has a length. Until it is voiced, it is estimated at the speaker's locked voice rate, or the normal rate while the speaker has no voice: about 2.6 words or 4 Chinese or Japanese characters a second. A locked voice also sets the pace: its locked audition is that voice saying a line, measured, so the estimate is scaled by the audition's measured length over its own estimate, at most twice either way. Such a length is marked as an estimate.
- **Planning.** Shot planning (`shot-plan/2`, `shot-plan-scene/2`) is given each line's length between `<line_durations>` and `</line_durations>`, and asked to give a shot that carries dialogue at least as long. The plan's input label names the hash of those lengths.
- **A shot's dialogue.** Its length is the sum of the screenplay lines it holds verbatim, with a 250 ms breath between them. Any other words are estimated, and a speaker's name before a colon is not said.
- **Short shots.** A planned shot shorter than its dialogue is published as a short shot. It is reported, not refused.
- **Video clips.** A video clip that would cut a shot's dialogue off is made at the shortest length the provider declares that holds it. When even the longest is too short, the prompt (`video-prompt/7`) reports `shot.dialogue_duration` among what it dropped. The cost shown before asking uses the same length.

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

A shot's action is only what the camera sees. Shot planning (`shot-plan/4`, `shot-plan-scene/4`) asks for no transition or editing direction in it or in what must stay (FADE IN, FADE OUT, CUT TO, DISSOLVE TO, "the fade" and the like), which the edit decides, and no words meant to appear on screen; an answer with one is asked for again, with the shot named. In the live run, a planned "FADE OUT" came back from Veo as words across the picture. A plan locked before this rule can still name one; the video compiler leaves it out of the clip and reports it ([Providers and MCP](../architecture/providers-and-mcp.md)).

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

How it works now (`kineto_project::preferences`, `kineto_core::preferences`):

**The profile.** It is a document the director writes and edits. Each change is a revision (`preferences/preferences_rNNNN.json`, `preferences.schema.json`), the latest is in force, and nothing is locked. Each entry has:

- a dimension: camera motion, lighting, framing, lens, colour, composition, performance or pacing;
- `prefer` or `avoid`;
- a value in the director's words;
- optionally a strength;
- where it came from: the director, or an accepted suggestion with its reason and evidence.

**What reads it.** Each kind of work reads the dimensions it can show, worded as one sentence ("The director's preferences, where the locked style allows: prefer …; avoid …"):

- Shot planning (`shot-plan/4`) reads every dimension between `<preferences>` and `</preferences>`, and favours them where the screenplay leaves the choice open.
- Keyframes (`keyframe-prompt/4`) read lighting, framing, lens, colour, composition and performance, after the locked style.
- Videos (`video-prompt/7`) read camera motion, lighting, colour, performance and pacing.

In a prompt, the preferences are the lowest-ranked part, the first cut when the prompt runs out of room.

**What it never does.** Changing preferences never makes work stale:

- A plan and its keyframes record the revision they followed as a cosmetic dependency, which no staleness check counts.
- Undecided keyframe candidates, and the plan in play, report that they were made with other preferences, for the director's information.
- Videos keep the preferences they followed in their prompt provenance.

**Learning from choices** (`kineto_core::learning`):

1. **Evidence.** The director's choices are gathered: keyframe candidates and video takes of each shot, voice auditions of each character who speaks, and takes of each line. For each, what was chosen (selected or locked) and what was passed over, each by artifact ID with what it was asked for. That is the prompt, notes, delivery or rate: words only, for now.
2. **Suggestions.** On the director's request (`preference-suggestions/2`), a language model compares the evidence, given the profile in force, and suggests at most eight preferences. Each has a reason and the artifact IDs of the chosen work that shows it. A suggestion citing work passed over, or anything it was not given, is asked for once more.
3. **Storage.** Suggestions are kept as revisions (`preferences/suggestions/suggestions_rNNNN.json`, `preference-suggestions.schema.json`), with the chosen work they considered and `compared: text`.
4. **Review.** Nothing takes effect until the director reviews each suggestion: accept it, accept it in their own words, or turn it down (`preferences/review.json`). An accepted one enters the profile as a new revision, sourced to its reason and evidence.
5. **When to ask.** Once five choices no suggestions have considered have accumulated, the session says suggestions are worth asking for. The director decides when; nothing asks on every choice. With an agent on a subscription the cost shows as unknown.

**Voice recommendations** (`kineto_core::voice::recommend`, `voice-recommendations/1`). For the characters who speak, a model recommends up to three voices each from the speech provider's list, with reasons, drawn from the bible's descriptions. An unlisted voice or a missing character is asked about once more. Recommendations are kept as revisions (`recommendations/voices/`, `voice-recommendations.schema.json`) and are advice: the director auditions and chooses.

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

A shot carries the screenplay lines its dialogue holds verbatim. Each line's locked take goes into the package as `<scene>_<shot>_line<NN>.<ext>`. The sidecar's `lines` names, for each line:

- the line, its speaker and its words;
- its file, take, hash and length;
- the provider, model, voice ID, delivery and rate that spoke it.

The manifest lists the shot's line files. Packages are schema version 2.

Under a recipe with a dialogue step (`default-film` v2), a shot is not ready while a line it carries is not locked, or its locked take no longer fits.

Each shot's sidecar (`schemas/export-shot.schema.json`) says which take was locked and what made it:

- the video's file, artifact ID, content hash, provider, model, clip length, the parameters it was asked with, and when it was locked;
- the keyframe it starts from, by artifact ID and hash, and its image beside it when it has one, and the keyframe it closes on, by shot, artifact ID and hash, when it closes on one;
- the prompt exactly as the provider got it, with its hash, compiler version, applied constraints and the dropped ones with why;
- the shot's planned intent: action, framing, lens, camera motion, mood, dialogue, characters, target duration and continuity;
- the director's notes and the take it branched from.

The project manifest (`schemas/export-manifest.schema.json`) holds the project and recipe, the export time, the scenes in screenplay order with location and time, each shot's place among the shots in play and its files, what is missing from a partial package and why, and the total runtime.

Only locked videos that still follow their shot are exported, each checked against its recorded hash. When a shot is not ready, the export says which and why, and writes nothing unless the director asks for the ready shots only. The package is written beside its destination and renamed into place, so an interruption never leaves half a package under its name, and exporting the same project again gives the same bytes. Replacing a package of the same name exchanges the two in one step where the system can (Linux, macOS), so the name always holds a whole package; where it cannot (Windows), an interruption can leave the earlier package set aside, and the next export puts it back first. A sidecar's lock time is the one the video's record kept when it was locked (`null` for a video locked before Kineto recorded it); the manifest records when the package was made.

The package also holds its timeline (`timeline.otio`, OpenTimelineIO): the shots end to end on a video track and each shot's locked lines on a dialogue track, at 24 frames a second. A shot lasts as long as its file does, read from the MP4's movie header without decoding. A shot's lines follow one another from its start with a 250 ms breath between them, as planning times them, and a line running past its shot's end is cut there and listed in the manifest (`timeline.trimmed_lines`). Each clip names its file relative to the package and carries its Kineto identity, so the package can move as a whole. The same placement is written as a CMX 3600 edit decision list (`timeline.edl`, non-drop timecode): one video event a shot and one audio event a line, in record order, each with a comment naming its file, and a blank (`BL`) audio event for the silence before the first line. A list that would hold more than 999 events refuses the export rather than being cut short.

Later ecosystem exports can add NLE-specific metadata.
