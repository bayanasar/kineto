# ADR-0006: Provider Plugins as Configured Local Processes

- Status: Accepted
- Date: 2026-10-04

## Context

Kineto's providers are built in: MiniMax image, video and speech, a free local demo of each, and language models behind `kineto-llm`. Workflow code asks for a capability (`image.generate`, `video.generate`, `tts.speak`) and never names a vendor (R7). Each provider declares what it can do (`ImageCapabilities`, `VideoCapabilities`, `SpeechCapabilities`), and every paid call runs as a write-ahead job with a receipt, an idempotency key and a fail-closed rule: a call that may have been charged without its result is never made again unasked.

The P6 roadmap asks for custom provider plugins: a studio's own image service, a local model, a provider Kineto does not ship, added without changing Kineto. That needs a boundary Kineto does not have yet.

The boundary is constrained by earlier decisions:

- ADR-0003: process adapters use application-defined executable identities; projects may provide data but never a command to run; model output and tool results are untrusted and cannot authorize spend, fallback to a paid provider, writes outside the project or process execution.
- ADR-0004: canonical state lives in the project directory, written only by the engine.
- ADR-0005: the desktop engine runs in-process; there is no engine process to put plugins beside.
- `providers-and-mcp.md`: dynamic dispatch only when a real plugin boundary requires it.

Three shapes were considered.

1. **In-process dynamic libraries** (`dlopen`). Rust has no stable ABI, so a plugin would need a C ABI and `unsafe` loading; a crash or memory error in a plugin takes the app down with the director's unsaved state; and it runs with every privilege the app has.
2. **WebAssembly.** A strong sandbox, but providers are mostly HTTP clients with credentials, and the host would have to offer networking, files and clocks before a plugin could do anything useful. Not justified for P6.
3. **Configured local processes.** The pattern Kineto already uses for agent CLIs (`kineto-llm`'s ACP backend): an executable the user installed and named, driven over stdio with a narrow protocol. A crash is a failed call, not a crashed app.

## Decision

### A plugin is a configured executable, never project data

A plugin is registered in the user's Kineto configuration (`<config>/kineto/plugins.toml`), not in a project:

```toml
[[plugin]]
id = "studio-images"                         # portable ID, unique
executable = "/opt/studio/bin/kineto-images" # absolute path
args = ["--profile", "film"]                 # fixed arguments
secrets = ["STUDIO_IMAGES_KEY"]              # secrets passed by name
enabled = true
```

A session selects a plugin as its provider (`KINETO_IMAGE_PROVIDER=plugin:<id>`), as it selects `minimax` today; the name is resolved against the user's configuration and refused when no enabled plugin has that ID or the plugin cannot describe itself. The selected plugin also speaks when it declares `tts.speak`. A project never supplies an executable, an argument or an environment variable. Records keep the provider key `plugin.<id>` (provider keys take letters, digits, `.`, `_` and `-` only) and the model the plugin declared.

### One process per operation, one JSON message each way

Kineto starts the plugin once per operation, writes one UTF-8 JSON request line to its stdin, closes stdin, and reads one JSON response line from its stdout. Stderr is discarded. There is no long-lived plugin process, no session state and nothing to supervise between calls; a crash, a hang or garbage on stdout fails that one operation.

Every message carries `"protocol": "kineto-plugin/1"`. A response with another major version is refused. Unknown fields are ignored, so a plugin may add metadata; missing required fields are refused.

### Operations

- `describe`, when a session selects the plugin and when settings list it: the plugin's name and version, optionally `invoke_deadline_seconds`, and for each capability it offers the same declaration a built-in makes: aspect ratios, reference roles and limits, formats and size limits, each clip it renders by length and height with its price, the first and last frames it takes, voices listing, prompt limits, prices, and whether results arrive at once or as jobs to follow. Kineto routes by this declaration exactly as it routes built-ins, so a plugin that cannot do what a step needs is refused before anything is written (`GenerationError::Incapable`).
- `estimate`: the cost of a request, or unknown.
- `invoke`: a request with its idempotency key. The answer is `completed` (output files), `pending` (a provider job ID to follow), or `failed` with a class (`retryable` or `terminal`) and `accepted`: `false` when the plugin knows the provider did not take the request, `true` or absent otherwise.
- `reconcile`: by provider job ID or by idempotency key, answering `completed`, `pending`, `failed` or `unknown`.
- `voices`, for `tts.speak`: the voices it offers. Free.

### Files go through a scratch directory, never the project

For each operation Kineto creates a fresh directory outside the project and runs the plugin there. Inputs a request needs, such as a locked keyframe or a reference portrait, are copied in and named by relative path with their hash; the plugin is never given a project path. A plugin returns outputs as files it wrote there, named by relative path with size and SHA-256. Kineto refuses an output whose hash, size or sniffed format does not match the declaration, then moves it into the project through the same write path as built-in outputs and removes the directory.

### Paid calls keep every existing safety rule

A plugin call runs through the write-ahead job runtime under provider key `plugin.<id>` (style samples and casting images under `plugin.<id>-style` and `plugin.<id>-cast`, speech under `plugin.<id>-<kind>`, as MiniMax's are), with the idempotency key passed in the request. Before an image call is sent, a record of what it is (its prompt's provenance, what it depends on, where its image will be kept) is written beside where its receipt will be; the image and its receipt are written before the call returns. Kineto treats a failed call as certainly not charged only when the plugin never started, nothing was sent, or the plugin answers `accepted: false`. Any other failure, including a crash, a timeout or an unreadable answer after the request was written, leaves the call unresolved, and it is never made again unless the director chooses to risk paying twice. When a project opens with a plugin as its provider, a sent image call that left no receipt is reconciled by idempotency key, and a plugin that kept the answer gives it back without a new call; a call never sent is not asked about. Jobs of a plugin that is not the session's provider wait until it is. Speech keeps its receipts in Kineto as for every speech provider.

### Process and environment

- Only the configured absolute executable starts, with its configured arguments, never through a shell.
- The environment is cleared. The plugin gets a minimal `PATH`, `HOME`, the scratch directory, and the secrets its entry names, read from OS secure storage or, in development, the environment. No other variable reaches it.
- Each operation has a deadline (`describe`, `estimate`, `voices` and `reconcile`: 30 s; `invoke`: up to 15 minutes, or the plugin's declared `invoke_deadline_seconds` if shorter). Past it the process is killed.
- Stdout is capped at 1 MiB; output files are capped by the declared size limits.
- Plugin answers are untrusted data under ADR-0003. A plugin cannot authorize spend, choose a fallback, run anything or write outside its scratch directory through Kineto. A plugin is still a program the user installed, with the user's network and file access; Kineto does not sandbox it beyond the process boundary and says so in its settings.

### What plugins do not cover

`llm.complete` stays with `kineto-llm`: HTTP API backends and agent CLIs over ACP already cover it. Plugins are for image, video and speech providers. Images and speech arrive in the answer to `invoke`. A video is always a job to follow: `invoke` is sent the first frame, the clip's length and height, and, where the plugin declares `end_frame_formats` and the next shot continues from this one with a locked keyframe, that keyframe as the last frame; it answers `pending` with the job, which is recorded as the video's task before anything else, and the job is followed with `reconcile` by that job until it is `completed` (one MP4 clip) or `failed`; `unknown` is a failure. Video jobs are kept under `plugin.<id>-video`, apart from the plugin's images, as MiniMax's are. The selected plugin also makes the session's video when it declares `video.generate`, unless `KINETO_VIDEO_PROVIDER` names another provider.

## Consequences

### Positive

- New providers need no Kineto release, and run behind the same capability routing, write-ahead jobs and receipts as built-ins.
- A plugin crash fails one call, never the app or the project.
- The protocol is language-neutral: a plugin can be a script or a binary.
- No project can make Kineto run a program.

### Negative

- A process start per operation adds latency, small next to a paid generation.
- Plugin authors must report `accepted` honestly; one that cannot tell gets fail-closed behaviour, and the director sees unresolved calls more often.
- Users install and trust plugins themselves; Kineto checks the protocol, not the program.

## Required tests

- A project naming a plugin that is not configured, or is disabled, is refused before anything is written.
- `describe` declarations route like built-ins: an incapable plugin is refused before a job is written.
- Malformed, oversized, wrong-version and missing-field answers fail the operation.
- A timeout kills the process and leaves a paid call unresolved.
- A crash after the receipt and before the answer is reconciled on reopen without paying twice.
- An output whose hash, size or format differs from its declaration is refused and nothing reaches the project.
- The plugin sees only its configured environment, its named secrets and its scratch directory; no project path appears in any request.
