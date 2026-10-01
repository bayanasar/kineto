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

## Initial provider support

Recommended first-wave support:

### LLM

- OpenAI API
- Anthropic API
- OpenAI-compatible API
- Ollama / local OpenAI-compatible endpoints
- optional CLI adapters for Codex/Claude-style tools

CLI adapters are experimental. Agent CLIs are not stable substitutes for inference APIs and must follow ADR-0003 process-spawn restrictions.

`kineto-llm` holds the `llm.complete` capability: an `LlmBackend` trait (blocking, static dispatch), requests that may carry the JSON Schema an answer must satisfy, and `complete_structured`, which turns an answer into typed state only after it deserializes and asks once more, with the parser's error, before refusing. Backends report how they are billed (per token, subscription, or free) so a cost estimate can say when the cost is unknown. Offline fakes cover scripted answers, latency, retryable-then-success, terminal failure and answers that do not fit the schema.

### Image / video

Start with one cloud provider and one local path.

The first cloud provider is MiniMax (`kineto-provider-minimax`): `image-01` for keyframes over blocking HTTPS with rustls and the bundled Mozilla roots, base64 responses so the image arrives inside the authenticated answer, no redirects, and every failure classified as retryable or terminal (unknown codes are terminal). Its image endpoint has no idempotency key or request lookup, so a request that fails after it was sent may already have been billed; the job runtime's attempt limit bounds the duplicate spend. `cargo run -p kineto-provider-minimax --example image_smoke` is the manual paid smoke test; no test suite calls it.

Video uses MiniMax Hailuo image-to-video (`MiniMax-Hailuo-02`) as a remote task: submitting (the paid call, with the first frame inlined as a `data:` URL) returns a task ID at once; querying it is free and can be repeated after a restart; a finished task names a file whose download URL is on MiniMax's content host, which is fetched without the API key, over https only, size-bounded and checked to be MP4.

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

The first compiler in use is `keyframe-prompt/1` (`kineto-core/src/keyframe_prompt.rs`). It reads the scene and shot intent from `scene.json` and `shot.json` and compiles a still-image prompt. Camera motion, dialogue and duration are reported as cosmetic drops kept for the video step. Until approved character reference images exist, every named character is an identity-affecting drop. The compiler refuses a shot without a written `action`, and it refuses a prompt longer than the provider accepts rather than truncating it.

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

A provider answer is only as durable as what is kept of it. For a synchronous provider without remote job handles, the adapter writes the result into the project and a receipt under `.kineto/generation/` before it returns, and the runtime marks the intent invoked only after that. Startup reconciliation completes such a job from its receipt, after checking the media bytes against the recorded hash, instead of calling the provider again; a receipt that no longer matches its media needs a person. Receipts are runtime state and are removed once the candidate is acknowledged. A crash before the receipt is written loses the answer, and the retry pays again.

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
