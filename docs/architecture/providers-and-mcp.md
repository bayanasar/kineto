# Providers and MCP

## Provider abstraction

Kineto separates model invocation from workflow logic.

Provider categories:

- LLM
- image
- video
- TTS
- embedding
- vision/evaluation
- local process
- custom HTTP

The workflow engine asks for capabilities, not vendor names.

Provider interfaces stay typed, but Kineto should not pay for runtime dynamic dispatch or boxed futures before a real plugin boundary requires it. Built-in providers should initially use concrete types or enum/static dispatch.

Conceptual Rust interface for a statically dispatched provider:

```rust
pub trait LlmProvider {
    fn capabilities(&self) -> LlmCapabilities;
    fn estimate_cost(&self, request: &LlmRequest) -> Result<CostEstimate>;
    fn generate(
        &self,
        request: LlmRequest,
    ) -> impl Future<Output = Result<LlmResponse>> + Send;
}
```

Equivalent capability-oriented interfaces exist for image, video, and TTS.

This sketch is deliberately not `dyn`-compatible. If runtime-loaded providers later require object safety, introduce an explicit adapter at that boundary and measure its allocation/dispatch cost. Do not make `async_trait`, boxed futures, or trait objects the default architecture merely for stylistic uniformity.

Provider core requests/responses are typed Rust data. `serde_json::Value` or other untyped maps may exist at a vendor HTTP serialization edge when an API requires them, but they are not the workflow/provider hot-path model.

## Capability models

Provider portability depends on describing what a provider can actually preserve.

Image capabilities should cover at least:

- aspect ratios and resolution limits
- seed support and determinism guarantees
- negative prompts
- reference images and reference roles (identity / style / composition)
- reference-count limits
- inpainting/mask support
- output formats

Video capabilities should cover at least:

- text-to-video and image-to-video modes
- min/max duration and fps options
- start/end-frame conditioning
- camera control: none / textual / parametric
- character/identity consistency mechanisms
- async job support and resumable remote handles
- output formats/containers

TTS capabilities should cover at least:

- voice identity/reference support
- language/locale
- style/emotion controls
- pacing/rate controls
- timing metadata
- streaming vs async generation

Cross-cutting capability metadata includes cost model, concurrency/rate limits, retry semantics, and known provider policy constraints where useful for preflight.

`ImageCapabilities` (`kineto-providers`) is what workflow code reads; an adapter declares it and no workflow code branches on a vendor name. Besides the items above it carries the formats and size limit of reference images, the longest prompt the provider accepts, whether an image arrives in the answer or as a remote job to follow (`async_jobs`), and the price of one image (`price_per_image`, a provider-neutral amount and currency, `None` when unknown). MiniMax `image-01` declares 1:1, 16:9 and 9:16, JPEG output, prompts up to 1500 characters, one identity reference (`subject_reference` of type `character`: a JPG or PNG portrait under 10 MB, best a single front-facing face), the image in its answer, and 0.0035 USD an image (the client sends the identity reference as a `character` data URL and refuses one that is not a JPEG or PNG under 10 MB before calling); it takes no style or composition reference and no seed or negative prompt. The local demo generator declares no references, the image at once, and no cost. `ImageProvider::capabilities` routes to a provider's declaration: the cost the director is shown before generating comes from its price, and keyframes and style samples, which keep the image the provider answers with, refuse a provider of remote jobs (`image.async_jobs`) before anything is written.

## Initial provider support

Recommended first-wave support:

### LLM

- OpenAI API
- Anthropic API
- OpenAI-compatible API
- Ollama / local OpenAI-compatible endpoints
- optional CLI adapters for Codex/Claude-style tools

CLI adapters are experimental. Agent CLIs are not stable substitutes for inference APIs and must follow ADR-0003 process-spawn restrictions.

`kineto-llm` holds the `llm.complete` capability: an `LlmBackend` trait (blocking, static dispatch), requests that may carry the JSON Schema an answer must satisfy, and `complete_structured`, which turns an answer into typed state only after it satisfies that schema (checked by `kineto-json-schema`, the same executable subset the schema contract tests use) and deserializes, and asks once more, with the reason, before refusing. A request whose schema uses anything the subset does not implement is refused before the model is asked. `complete_checked` adds a check the parsed answer must also pass, such as a screenplay naming only characters and locations its story analysis defines; a failed check is asked about once more in the same way. Backends report how they are billed (per token, subscription, or free) so a cost estimate can say when the cost is unknown. Backends also say whether a failed call may have lost an answer the model gave, and was charged for (a connection lost or timed out after sending, a server error), or certainly left none (no connection, an API refusal, an answer that arrived and was refused). Offline fakes cover scripted answers, latency, retryable-then-success, terminal failure, metered billing and answers that do not fit the schema.

One backend, `OpenAiCompatible`, serves both "one cloud provider" and "one OpenAI-compatible/local provider": Kimi's open platform by default (`kimi-k3`, key from `KINETO_KIMI_API_KEY`, prices for known models, anything else unpriced), or a local model server. It sends HTTPS, or HTTP only to a loopback address, follows no redirects, asks for `json_object` output with the schema in a system message, and treats a truncated or withheld answer as terminal. Rate limits and server errors are retryable; quota, authentication and request errors are terminal.

Agent CLIs are the other kind of backend. Kineto drives the CLI itself (Kimi Code first) and never reads or reuses its credentials. It speaks the Agent Client Protocol (`kimi acp`) rather than the one-shot `-p` mode: `-p` takes the prompt as a command-line argument, which caps its length, and runs with automatic tool approval, so an instruction hidden in source text could run commands. Measured on Kimi Code 2.1.1 with the user's own configuration, an ACP session also inherited the user's hooks, and read-only tools such as `Glob` ran locally without asking or routing through the client. Every call therefore runs inside these layers:

- a data root kineto creates and owns (`KIMI_CODE_HOME`), marked by a `kineto-sandbox` file; a root without the marker, such as the user's own `~/.kimi-code`, is refused and never touched. Before every call kineto re-applies the sandbox to its `config.toml`, keeping only what sign-in writes (providers, models): `[tools] enabled = ["*"]`, which Kimi Code documents as matching no tool and so leaves none, including tools a later release adds; the known built-in tools and `mcp__*` also in `disabled` and denied in `[[permission.rules]]`; manual permission mode; no hooks, extra agent directories, skills merging, `mcp.json` or telemetry. Sign-in rewrites the file and drops comments, which is why the marker is a file and the sandbox is enforced rather than assumed. The user signs in once under the root (`KIMI_CODE_HOME=<root> kimi login`).
- kineto as the ACP client: file access is declared as client-routed and refused, there is no terminal, and every permission request is answered with the reject option.
- a fresh empty working directory per call, removed afterwards, and the session deleted from the agent's history.

A session's model is named by `KINETO_LLM_PROVIDER`: unset, empty or `none` for no model; `kimi-code` for Kimi Code (the CLI from `KINETO_KIMI_CODE_BIN`, otherwise `kimi` on `PATH`; its data root from `KINETO_KIMI_CODE_HOME`, otherwise `agents/kimi-code` in Kineto's state directory, for example `~/.local/state/kineto/agents/kimi-code`); `kimi` for Kimi's API with `KINETO_KIMI_API_KEY` (and `KINETO_KIMI_MODEL`); `local` for a server at `KINETO_LOCAL_LLM_URL` serving `KINETO_LOCAL_LLM_MODEL`. As with images, no model is asked anything because a key or CLI happens to be present. Sign in to Kimi Code once under Kineto's data root (`KIMI_CODE_HOME=<root> kimi login`).

Verified live (2026-10-01) with Kimi Code 2.1.1 under such a root: a prompt that also asked it to run `ls` and read a file produced no tool calls, permission or file requests at all, and the file's content did not appear. The answer is the concatenated `agent_message_chunk` text; thoughts and tool traffic are ignored. Agent backends are billed by subscription, so their cost is reported as unknown. A live story analysis of a one-paragraph story through Kimi Code took about 100 seconds and satisfied the schema on its first answer. A four-scene screenplay of that analysis took about 70 seconds and passed both the schema and the check against the analysis on its first answer; rewriting one scene with a director's note ("keep it wordless") took about 40 seconds and left the other three scenes' hashes unchanged. Offline tests drive a scripted agent binary (`fake_acp_agent`) through approval and file requests, missing sign-in, crashes, refusal, malformed output and a hang past the deadline.

### Image / video

Start with one cloud provider and one local path.

The first cloud provider is MiniMax (`kineto-provider-minimax`): `image-01` for keyframes over blocking HTTPS with rustls and the bundled Mozilla roots, base64 responses so the image arrives inside the authenticated answer, no redirects, and every failure classified as retryable or terminal (unknown codes are terminal). Its image endpoint has no idempotency key or request lookup, so a request that fails after it was sent may already have been billed; the job runtime's attempt limit bounds the duplicate spend. `cargo run -p kineto-provider-minimax --example image_smoke` is the manual paid smoke test; no test suite calls it.

Video uses MiniMax Hailuo image-to-video (`MiniMax-Hailuo-02`) as a remote task: submitting (the paid call, with the first frame inlined as a `data:` URL) returns a task ID at once; querying it is free and can be repeated after a restart; a finished task names a file whose download URL is on MiniMax's content host, which is fetched without the API key, over https only, size-bounded and checked to be MP4.

Video providers implement `kineto_core::video::VideoGenerator` (statically dispatched): what they declare (`VideoCapabilities`), a paid submission, free queries and a download. MiniMax declares image-to-video from a JPEG, PNG or WebP first frame of at most 20 MB, clips of 6 or 10 seconds at 768P priced per length, Hailuo's bracketed camera commands as parametric camera control, a 2,000-character prompt and tasks that outlive the client. The local demo declares the same image-to-video contract, free, in 2-second clips: its task is done as soon as it is submitted, and its file is a still clip bundled with the build (`crates/kineto-core/assets/demo-clip.mp4`), so a demo project goes through generation, selection and locking offline, through the same write-ahead jobs and receipts, recorded under its own provider key (`local-demo-video`). It starts from a demo keyframe, which has no image; MiniMax refuses one. Before anything is written, a shot video checks the provider's capabilities: one that would make it from words alone, or cannot take the locked keyframe's format or size, is refused (`GenerationError::Incapable`, naming the capability) rather than quietly used. The clip length is the declared length closest to the shot's, and the job's cost estimate is the declared price of that length.

Local generation should be implementable through a process/API adapter such as ComfyUI without coupling core workflow logic to it.

### TTS

Support:

- one production-quality cloud provider
- one local provider
- custom HTTP endpoint

Voice metadata belongs to character production memory.

## Model router and fallback policy

Provider selection supports:

- explicit user selection
- per-capability default
- project-level override
- task-level override
- fallback chain

Example:

```text
video.generate
  preferred: provider-a/model-x
  fallback: provider-b/model-y
```

The router checks required capabilities before fallback. A fallback that cannot preserve a required identity/reference constraint must refuse or require explicit degraded-generation approval.

Fallback policy is configuration, not prose. It must explicitly govern whether paid fallback is allowed and any spend ceiling/approval requirement.

## Prompt compiler

Do not store only a single raw prompt and do not return only a string from compilation.

Store structured intent and compile it for each provider:

```text
Shot Intent
   ↓
Provider Prompt Compiler
   ├── Provider A compiled request
   ├── Provider B compiled request
   └── Local model compiled request
```

A conceptual compilation result:

```rust
pub struct CompiledPrompt {
    pub prompt: String,
    pub applied_constraints: Vec<ConstraintId>,
    pub dropped_constraints: Vec<DroppedConstraint>,
    pub degradation: DegradationLevel, // none | cosmetic | identity_affecting
    pub compiler_version: String,
}
```

Rules:

- every dropped constraint has a reason
- non-empty dropped constraints are visible before generation
- `identity_affecting` degradation is blockable by policy
- the artifact records the drop list, degradation level, compiler version, and compiled provider input verbatim
- `prompt_hash` includes structured intent, compiler version, and compiled provider input

Input may include:

- visual style
- framing
- lens
- camera motion
- action
- emotion
- character locks
- location locks
- continuity state
- negative constraints
- provider capability hints

This layer is a major piece of Kineto's model portability.

The keyframe compiler is `keyframe-prompt/3` (`kineto-core/src/keyframe_prompt.rs`). It reads the scene and shot intent from `scene.json` and `shot.json`, and what production has locked, and compiles a still-image prompt for the provider's declared `ImageCapabilities`.

- The locked style's directives and note lead the prompt; without a locked style it asks for a photorealistic film still.
- A character whose cast is locked is shown from their canonical portrait, sent as an identity reference, up to as many references as the provider declares it takes in one image, in the shot's order.
- Every other named character is an identity-affecting drop, and the reason says why: not cast yet, one reference per image, or a provider that takes none.
- A cast character's forbidden changes are said while the prompt has room; dropping them is identity-affecting too.
- Camera motion, dialogue and duration are reported as cosmetic drops kept for the video step.
- A generation the director refines carries their note for that generation only, as `director.note`.
- Continuity comes from the locked bible and the shot's plan, each said while the prompt has room and reported when cut: what each character wears in this scene (its wardrobe change there, or its usual wardrobe), the location's look and what never changes there, the props the bible places in the scene, and the shot's own continuity notes. A shot that continues from another says so in words; the earlier shot's keyframe is not sent as a reference, a cosmetic drop. A keyframe depends on every bible entry it read (`bible_character.<id>`, `bible_location.<id>`, `bible_prop.<id>`), and is stale when one changes.

The compiler refuses a shot without a written `action`, and it refuses a prompt longer than the provider accepts rather than truncating it. Identity drift is reported on the candidates; a project can block it instead (`identity_policy = "block"` under `[workflow]`), and then a generation that would show a character without an identity reference waits for the director's explicit consent.

Projects created by the desktop app start with the bootstrap scene (`kineto-core/src/bootstrap.rs`) written into these files, published atomically with the project.

## Paid job safety

A paid/asynchronous provider call has a deterministic idempotency key derived from semantic `input_hash` plus operation identity.

The engine persists a write-ahead intent record before invoking the provider. When supported, it persists the provider's remote job handle (`provider_job_id`) immediately and reconciles it on restart rather than issuing a duplicate call.

Adapters declare:

- cost-estimation support
- concurrency ceiling
- rate-limit/backoff policy
- retryable vs terminal error classification
- remote-job recovery capability

Batch generation has a dry-run/preflight path that can report call count and estimated cost before spend.

A provider answer is only as durable as what is kept of it. For a synchronous provider without remote job handles, the adapter writes the result into the project and a receipt under `.kineto/generation/` before it returns, and the runtime marks the intent invoked only after that. Startup reconciliation completes such a job from its receipt, after checking the media bytes against the recorded hash, instead of calling the provider again; a receipt that no longer matches its media needs a person. Receipts are runtime state and are removed once the candidate is acknowledged. A call that was made but left no receipt (a crash before it was written, or a failure after the request was sent) may have been charged, and a provider that cannot be asked whether a request arrived gives no way to find out. Such an attempt fails closed: it is not retried automatically or on an ordinary request. It is retried only when a refusal record proves the provider certainly did not accept it (never sent, no connection, or an explicit refusal), or on a person's explicit decision to risk paying twice, which retires it with `JobRuntime::retire_unresolved`. Documents a model writes follow the same rule when the model is charged per call (Kimi's API): the answer and its receipt are written before the intent is marked invoked, a failure that certainly left no answer writes a refusal record, and an attempt with neither (a crash after the model answered, a connection lost after sending) makes the next request for that document fail with `paid_call_unresolved` until the director asks again at the risk of paying twice (`documents::risk_paying_twice`, the `resubmit` flag of the document commands). A model not charged per call (Kimi Code on its subscription, a local model) is simply asked again. A remote provider's job handle is kept the same way: the adapter writes it into the receipt as soon as the provider accepts the submission, so a job accepted just before a crash is reconciled from its receipt and followed, not submitted again.

The runtime refuses, rather than queues, a call over the provider's process-wide concurrency ceiling. A refusal spends no attempt, so the generation path waits for a permit (bounded) before invoking.

## Deterministic fake providers

The same provider interfaces must support offline deterministic fakes with configurable:

- success
- injected latency
- transient failure then success
- terminal failure
- capability-limited behavior

These fakes are required for workflow, retry, dropped-constraint, and migration tests without provider spend.

## MCP role

MCP belongs in the tools/context layer, not as the only LLM abstraction.

Good MCP use cases:

- RAG and external references
- filesystem tools
- Blender
- asset databases
- production databases
- prompt libraries
- custom studio tools
- external metadata sources

Architecture:

```text
               kineto-core
              /          \
     Provider adapters    MCP clients
            │                 │
       model APIs        tools/context
```

The core workflow remains functional without MCP. MCP output is untrusted data and cannot authorize provider spend, arbitrary process execution, or unrestricted filesystem writes.

## Secrets

Secrets are stored outside project files.

Recommended strategy:

- OS keychain / secure credential store for desktop
- environment variables for development/CLI
- local encrypted settings fallback only if platform secure storage is unavailable

Never write API keys or authentication material into:

- project files
- logs
- generated prompt files
- issue reports
- exported artifacts
- telemetry

Secret-redaction behavior requires automated tests.
