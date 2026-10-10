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

Third-party image, video and speech providers are plugins (ADR-0006): executables the user registers in Kineto's configuration, never named by a project as a command, started once per operation and spoken to with one JSON message each way (`kineto-plugin/1`). A plugin declares the same capabilities a built-in does and runs behind the same routing, write-ahead jobs and receipts; its files pass through a scratch directory outside the project and are checked against their declared hashes, sizes and formats before they enter it. An answer names at most 16 files of at most 64 MiB each and 128 MiB together (`kineto_plugins::Limits`), which is checked before any file is read, and each file is read only to the size it declared, so a plugin cannot exhaust Kineto's memory with an answer that passes every per-file check.

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
- start/end-frame conditioning, and the clip lengths an end frame can be sent with
- whether the clip comes with sound the provider makes
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

One backend, `OpenAiCompatible`, serves both "one cloud provider" and "one OpenAI-compatible/local provider": Kimi's open platform by default (`kimi-k3`, key from `KINETO_KIMI_API_KEY`, prices for known models, anything else unpriced), any connection the director added that speaks the chat-completions protocol, or a local model server. It sends HTTPS, or HTTP only to a loopback address, follows no redirects, asks for `json_object` output with the schema in a system message, and treats a truncated or withheld answer as terminal. Rate limits and server errors are retryable; quota, authentication and request errors are terminal.

A second, `AnthropicMessages` (`kineto_llm::anthropic`), speaks Anthropic's messages protocol for connections that name it: `POST <url>/v1/messages` (`<url>/messages` when the address already ends in `/v1`), the key in `x-api-key`, `anthropic-version: 2023-06-01`, the system prompt and the schema instruction apart from the turns, and `max_tokens` always sent (16,384 unless the request sets one). Only text blocks are the answer. A cut-off (`max_tokens`) or declined (`refusal`) answer is terminal; 429 and 5xx (529, overloaded, included) are retryable.

Agent CLIs are the other kind of backend: the ones the director already installed and signed in to, Claude Code, Codex, Kimi Code, Antigravity (`agy`) and Mini-Agent (`kineto_llm::agents`, [ADR-0008](../adr/0008-directors-own-agents-and-provider-choice.md)). Kineto finds the CLI on the search path, where installers usually put it (a desktop app does not get the shell's path), or where the director's settings say; it drives it under the director's own sign-in, never signs in for them and never reads a credential the CLI keeps. Every call runs in a fresh empty working directory, removed afterwards, gets the conversation as one prompt, and is started without tools, as each CLI allows:

- Claude Code: `claude -p --tools "" --setting-sources "" --strict-mcp-config --disable-slash-commands --no-session-persistence`, the prompt on stdin and the schema as `--json-schema` (without its `$schema` and `$id`, which its validator refuses). No settings means none of the user's hooks or plugins load; measured with a `SessionStart` hook that ran without `--setting-sources ""` and did not with it.
- Codex: `codex exec --json --ignore-user-config --ignore-rules --ephemeral --skip-git-repo-check --sandbox read-only`, approval `never`, web search off and every feature that is a tool disabled (`shell_tool`, `unified_exec`, `apps`, `browser_use`, `computer_use`, `multi_agent`, `plugins`, `hooks`, `view_image`, `image_generation` and the rest), the prompt on stdin. Measured: with the read-only sandbox alone it still ran `cat` on a file outside the working directory; with the features off it ran nothing. Ignoring the user's configuration also ignores a model it names that the sign-in cannot use.
- Kimi Code: `kimi -p <prompt> --agent-file <file> --output-format stream-json`, with an agent file of kineto's that allows no tools (`tools: []`, MCP tools included, no subagents). Its print mode takes the prompt only as an argument, so a request longer than about 120 KB (28 KB on Windows, where the whole command line is limited) is refused before it starts. The user's own hooks still run and speak before the answer, which is the last thing the assistant says.
- agy: an agent defined in the working directory (`.agents/agents/kineto-writer/agent.md`) with no default prompt, no MCP servers, no rules and only `finish`, the tool that hands back a structured answer; `--sandbox`, the message on stdin as stream-json and the schema as `--json-schema`. Without a schema it answers `{}`, so a request for text asks for `{"answer": ...}`. The user's own hooks still run.
- Mini-Agent: its ACP server, started in its own Python (the one its `mini-agent-acp` launcher names) with every tool switched off, reading its own configuration, which holds the director's MiniMax key; so it is billed per request, unpriced. Over ACP, kineto answers every permission request with a rejection and every file or terminal request with an error, and declares no terminal.

Measured on 2026-10-08 with Claude Code 2.1.295, Codex 0.160.0, Kimi Code 2.1.1, agy 1.2.16 and Mini-Agent `d76a4f6`, each signed in as the director uses it: a story analysis of the one-paragraph test story satisfied the schema on its first answer through each (Codex 10 s, Claude Code 17 s, Kimi Code 37 s, Mini-Agent 58 s, agy 166 s), and a prompt that also asked to run `ls` and read a file ran no tool through any of them. The answer is the structured output or the last message; thoughts and tool traffic are ignored. Agent backends on a subscription report their cost as unknown. Offline tests drive a scripted stand-in (`fake_agent_cli`) through each CLI's arguments, sign-in failures, a hang past the deadline and cancellation, and a scripted ACP agent (`fake_acp_agent`) through approval and file requests.

A session's model is chosen in settings (below), or named by `KINETO_LLM_PROVIDER`, which wins when set: unset, empty or `none` for no model; `claude-code`, `codex`, `kimi-code`, `agy` or `mini-agent` for that agent, found as above or at `KINETO_<AGENT>_BIN` (`KINETO_CLAUDE_CODE_BIN`; for Mini-Agent, the Python that runs it); `kimi` for Kimi's API with its key; `connection:<name>` for a connection the director added (below); `openai-compatible` (or `local`), for development, for any server that speaks the chat-completions protocol, at `KINETO_LOCAL_LLM_URL` serving `KINETO_LOCAL_LLM_MODEL`, with `KINETO_LOCAL_LLM_KEY` naming the key it takes (`deepseek` reads the key kept as `deepseek`, or `KINETO_DEEPSEEK_API_KEY`). A server with a key is metered at prices this build does not know; one without is local and free. HTTPS, or HTTP to a loopback address only. As with images, no model is asked anything because a key or CLI happens to be present.

### Choosing providers

The director chooses in Settings who makes each part: documents (any agent above, Kimi's API, or a text connection), images (the local demo, MiniMax, agy, an image connection, or a plugin that makes images), video and speech (each following images unless chosen). The choices are kept in `<config>/kineto/providers.toml` beside `plugins.toml` (`kineto_plugins::settings`), with the connections, never a key, and where an agent is when kineto would not find it.

A connection ([ADR-0009](../adr/0009-connections-by-protocol.md)) is any service the director adds by the protocol it speaks, never by vendor: a name (how it is chosen, `connection:<name>`, and how its calls are recorded, `connection.<name>`), a protocol (`openai-chat` or `anthropic-messages` for text), an address (HTTPS, or HTTP to this computer), a model, the name its key is kept under (none for a keyless local server), and optionally its prices in US dollars per million tokens read and written. Without both prices its cost is unknown, and says so; only a keyless connection to this computer is free. Settings add, change and remove connections, with the key typed in the same form and kept in the secure storage under its name. Renaming one carries a choice of it along; removing one clears the choice and keeps the key. The single OpenAI-compatible server an earlier build kept (`[openai_compatible]`) is read as a connection named after its key, and a choice of it follows.

An image connection speaks `openai-images` or `gemini-images`, and gives a price per image and how many reference images it takes (0 to 16, none unless the director says) instead of token prices; one that takes none cannot lock a cast. It plays a plugin's part inside Kineto, as agy does: the same invoke contract, records written before and after the call under `connection.<name>`, the same refusal evidence and recovery.

- `openai-images` (`kineto-provider-openai`) posts `<url>/images/generations` (JSON: model, prompt, one image, a size), or `<url>/images/edits` as a multipart form with each reference an `image[]` file. The size is the one of OpenAI's three (1024x1024, 1536x1024, 1024x1536) nearest the shot's frame, and the frame is said in the prompt too; a service that refuses the size (a 400 that names it) is asked once more without it, which its refusal made free. The image arrives inline (`b64_json`) or at an address, fetched without the key, three times before giving up: it was made and paid for.
- `gemini-images` (`kineto-provider-gemini`, module `image`) posts `<url>/models/<model>:generateContent` with the references as inline parts before the prompt, `responseModalities: ["IMAGE"]` and the frame as `imageConfig.aspectRatio` when the model draws it. An answer without an image (a blocked prompt, an image withheld) is a refusal.

An image whose shape is more than 2 % from the shot's frame is cut to it from its middle, as PNG. A refusal, a connection never made and any 4xx certainly made nothing, so asking again needs no consent; a timeout, a lost connection, a 5xx or an image that could not be fetched leave the call unresolved until the director consents to pay twice. Fields the director writes under `[connection.request]` in the settings file (Seedream's `watermark = false`, a `size` of its own) are sent with every request and win; settings show their names and never change them. A text connection takes none. A session reads them when it opens; the app opens the project again when the director leaves Settings having changed a choice, a key or the plugin configuration, after any request under way has finished, so the change applies at once. An environment variable that is set (`KINETO_IMAGE_PROVIDER`, `KINETO_VIDEO_PROVIDER`, `KINETO_SPEECH_PROVIDER`, `KINETO_LLM_PROVIDER`) wins, for development, and settings say so. The file keeps fields a newer build wrote.

### Image / video

Start with one cloud provider and one local path.

The first cloud provider is MiniMax (`kineto-provider-minimax`): `image-01` for keyframes over blocking HTTPS with rustls and the bundled Mozilla roots, base64 responses so the image arrives inside the authenticated answer, no redirects, and every failure classified as retryable or terminal (unknown codes are terminal). Its image endpoint has no idempotency key or request lookup, so a request that fails after it was sent may already have been billed; the job runtime's attempt limit bounds the duplicate spend. `cargo run -p kineto-provider-minimax --example image_smoke` is the manual paid smoke test; no test suite calls it.

Video uses MiniMax Hailuo image-to-video (`MiniMax-Hailuo-02`) as a remote task: submitting (the paid call, with the first frame inlined as a `data:` URL) returns a task ID at once; querying it is free and can be repeated after a restart; a finished task names a file whose download URL is on MiniMax's content host, which is fetched without the API key, over https only, size-bounded and checked to be MP4.

The second paid video provider is Google Veo 3.1 over the Gemini API (`kineto-provider-gemini`, module `veo`). Google shuts its three preview models down on 2026-10-22 (`veo::SHUTDOWN`) and names Gemini Omni Flash, below, as their replacement; until then Veo is selected with `KINETO_VIDEO_PROVIDER=veo` (the Lite model) or `veo:fast` / `veo:standard`, with the key in `KINETO_VEO_API_KEY`. The first frame, and a last frame where one is sent, go as `bytesBase64Encoded` with their MIME type (the documented `inlineData` is refused) to `models/<model>:predictLongRunning`; the answer names a long-running operation, which is the video's task. Asking about the operation is free. A finished one names the video's URI on the API, which is downloaded with the key, checked to be MP4 and bounded in size; a redirect to another host is followed without the key, and the key is never sent off the API's host. Veo keeps a video for 2 days. An operation that ended in an error, or with its video withheld by Veo's filters, is final and was not charged, so asking again submits afresh, and the director is told it is worth asking again: code 14 means Veo was too busy, and in the live run Veo withheld one shot's video twice over "an issue with the audio" and made it from the same prompt on the third try. A file gone from the server is final too. Each model declares what Google documents: clips of 4, 6 or 8 seconds at 720p and only of 8 seconds at 1080p (and 4k, except Lite), 24 fps, JPEG or PNG frames of up to 7 MB each (two of them base64 encoded stay under the API's 20 MB inline limit), camera motion in words, a 3,000-character prompt (Veo reads 1,024 tokens), and its price per second at each height. A last frame is taken only with an 8-second clip (`end_frame_seconds`); a 4-second clip with one is refused with HTTP 400. Every clip comes with an AAC sound track Veo makes, which cannot be turned off (`audio`); when that sound fails Veo's own checks, Veo withholds the whole video (7 of 15 live submissions), uncharged, and the director is told to ask again. A clip is framed 16:9 or 9:16 (`aspect_ratios`), sent as `aspectRatio` and named in its parameters (`aspect=16:9`). Standard and Fast take up to 3 reference images of a subject to keep as it looks (`max_reference_images`, role `Identity`, sent as `referenceImages` of type `asset`), JPEG or PNG, only in an 8-second clip (`reference_seconds`), with all of a request's images together no larger than two frames; Lite takes none. Each reference's content hash enters the parameters, so a different portrait is a different input. A shot's video sends no references yet, since its locked keyframe already carries who is in it, and references have not been tried live.

The third, and Veo's replacement, is Gemini Omni Flash (`gemini-omni-1.1-flash`) over the Gemini API's Interactions API (`kineto-provider-gemini`, module `omni`), selected with `KINETO_VIDEO_PROVIDER=omni`, with the key in `KINETO_GEMINI_API_KEY`. The paid step is one interaction made in the background (`POST interactions` with `background` and `store`), answered at once with its ID, the video's task; asking about it is free, and once completed it holds the video inline, base64 encoded, which is checked to be MP4 and bounded in size. Google keeps an interaction for 55 days on the paid tier. Omni takes neither a length nor frame roles as parameters, so the text sent binds the keyframe with Omni's tag (`<FIRST_FRAME>`, and `<LAST_FRAME>` for a last frame) and says how long the clip lasts (`omni::text`); both follow from the request, and the clip's parameters (`duration=…;resolution=720p;aspect=…;wording=1`) name them, so a change of wording is a change of input. A prompt that names one of Omni's media tags is refused before it is sent. No `task` is sent. Omni declares what Google documents and live trials on 2026-10-08 showed: clips of any whole second from 4 to 10, asked for in words and kept to (4 and 6 s were tried), 720p at 24 fps, 16:9 or 9:16, JPEG or PNG frames of up to 7 MB, a first frame it starts on (SSIM 0.96 against the keyframe) and a last frame it ends near (0.84, against a frame of the same setup), camera motion in words, and its own AAC sound track. It is billed by output tokens: 5,792 for a second of 720p video at $17.50 a million, about $0.10 a second, which is each clip's declared price; taller clips are upscaled at a price Google does not state and are not declared. Omni takes reference images (`<IMAGE_REF_N>`) without a documented limit; none are declared or sent. A failed or cancelled interaction, or one completed without a video, is final and worth asking again; Google does not say whether it is charged, and kineto does not claim it is not.

Video providers implement `kineto_core::video::VideoGenerator` (statically dispatched): what they declare (`VideoCapabilities`), a paid submission, free queries and a download. A provider declares the clip heights it renders (in lines, as in 768 for 768P), which length and height it renders together, the price of each such clip, the formats and size of the first and last frames it takes, the frame shapes it renders (none when it frames a clip as its first frame), and how many reference images of which role it takes, in which formats, sizes and clip lengths. MiniMax declares image-to-video from a JPEG, PNG or WebP first frame of at most 20 MB, closing on a last frame of the same kinds where one is sent, clips of 6 or 10 seconds at 512P or 768P and of 6 seconds at 1080P, each at its published price, Hailuo's bracketed camera commands as parametric camera control, a 2,000-character prompt and tasks that outlive the client. The local demo declares the same image-to-video contract, free, in 2-second clips: its task is done as soon as it is submitted, and its file is a still clip bundled with the build (`crates/kineto-core/assets/demo-clip.mp4`), so a demo project goes through generation, selection and locking offline, through the same write-ahead jobs and receipts, recorded under its own provider key (`local-demo-video`). It starts from a demo keyframe, which has no image; MiniMax refuses one. Before anything is written, a shot video checks the provider's capabilities: one that would make it from words alone, cannot take the locked keyframe's format or size, cannot frame a clip in the keyframe's 16:9 (`video.aspect_ratio`), or whose tasks are not followed by a handle that survives a restart (`async_jobs` and `resumable_remote_handle`), is refused (`GenerationError::Incapable`, naming the capability) rather than quietly used. The clip length is the declared length closest to the shot's; its height is the lowest the provider renders at that length that is at least 720 lines, or its tallest when none is; and the job's cost estimate is the declared price of that clip.

An agent CLI that makes images is an image provider too. agy's `generate_image` takes reference images, so it is offered with identity, composition and style references; it frames by the words of the prompt (a 16:9 request came back 1376×768). It is a plugin built in (ADR-0006, [ADR-0008](../adr/0008-directors-own-agents-and-provider-choice.md)): kineto plays the plugin's part in its own process, so its candidates are written ahead, kept, recovered and refused exactly as a plugin's, under the provider key `agent.agy`. The agent it runs has only `generate_image` and `finish`; references are copied into the call's working directory and named in the request, and the image is read from where agy keeps it, named by the conversation. Codex declares image generation available, but neither `codex exec` nor its app server made an image when measured, so it is not offered.

Local generation should be implementable through a process/API adapter such as ComfyUI without coupling core workflow logic to it.

### TTS

Support:

- one production-quality cloud provider
- one local provider
- custom HTTP endpoint

Voice metadata belongs to character production memory.

`SpeechCapabilities` (`kineto-providers`) is what workflow code reads. It declares:

- where voices come from: a listed catalogue, cloning from reference audio, or design from a description;
- the languages spoken, as ISO 639-1 codes;
- the deliveries (emotions) a line can be asked for;
- the range of speaking rates (in thousandths of the normal rate) and pitch shifts (in semitones);
- how finely it times what it spoke: sentences, words, or only the whole;
- whether speech arrives in the answer or as a remote job;
- its output formats;
- the longest text it takes;
- its price per character.

`evaluate_speech` holds a chosen voice to what was chosen. A provider that would change its language, delivery, rate or pitch, or would answer with a remote job where the workflow keeps the answer, is incompatible. Less timing than wanted is only a warning, because a line's length is known from its audio either way.

Speech providers implement `kineto_core::speech::SpeechGenerator`, statically dispatched:

- what they declare;
- their voices, which are free to list;
- synthesis, the paid call;
- error classification, and whether an error proves the call was not accepted.

Before anything is written, `check_speech` refuses a provider that cannot keep the voice or take the text (`GenerationError::Incapable`, naming each capability). It also picks the format to ask for, compressed when the provider offers it.

Synthesis runs as a write-ahead job, like a paid image, in its own partition per provider and kind of work (`<provider>-<kind>`, such as `local-demo-speech-line`). The order is:

1. The intent is prepared.
2. A receipt holds the record's plan.
3. The provider answers.
4. The audio is written beside its record, and the receipt names its hash.
5. Only then is the intent marked invoked.

A restart records speech from a receipt whose audio matches, and never calls again. An attempt that left no audio is asked again only if its receipt proves the provider certainly did not accept that attempt (`refused_attempt`). Otherwise speech in its numbering is refused until the director chooses to risk paying twice. That choice retires only that numbering's calls.

The local demo (`local-demo-speech`, model `demo-tone`) is free, offline and the same for the same line:

- It declares a catalogue of three voices, every delivery, rates of 0.5–2×, pitch of ±12 semitones, sentence timing, WAV output and no cost.
- It speaks each sentence as a triangle tone, made in integer arithmetic so the bytes are the same on every platform.
- Each sentence lasts about as long as a person would take to say it (`estimate_duration_ms`): about 2.6 words a second, or 4 Chinese or Japanese characters a second, with a 250 ms breath between sentences, scaled by the voice's rate.
- Delivery sets its loudness and pitch sets its frequency.

The cloud provider is MiniMax `speech-2.8-hd` (`t2a_v2`), recorded under `minimax-speech`, apart from MiniMax images and videos.

- **The call is synchronous.** The audio arrives hex encoded inside the authenticated answer (`output_format: hex`), and `extra_info.audio_length` is its measured length in milliseconds.
- **Sentence timings come from a separate file.** The request turns on `subtitle_enable`, and MiniMax writes the timings to a file on its content host. That file is fetched over https only, without the key, and is at most 1 MB. The audio is already paid for, so if the file cannot be had, the speech is kept without timings.
- **Declared capabilities**, as MiniMax documents them:
  - its catalogue of system voices, listed free by `POST /v1/get_voice` (`voice_type: system`);
  - 39 languages, sent as `language_boost` names for their ISO 639-1 codes;
  - happy, sad, angry, fearful, disgusted, surprised and calm (MiniMax's `fluent` and `whisper` belong to its 2.6 models only);
  - rates of 0.5–2× and pitch of ±12 semitones;
  - MP3 (32 kHz, 128 kbps, mono), WAV or FLAC;
  - fewer than 10,000 characters a call;
  - $100 per million characters (100 USD micros a character).
- **Refusals before sending.** The client refuses a request it cannot send before anything is sent, so a refused request was certainly not accepted. It also refuses an answer whose audio is not in the format asked for.
- **Not supported.** MiniMax also clones voices ($1.5 a voice) and designs them ($3 a voice). This adapter does neither, so neither is declared.
- **Error classes.** Rate limits (1002, 1039 tokens per minute) are retryable. Every refusal, including 2056 when the plan's quota is spent, is certainly not accepted, and its words reach the director.
- **Manual smoke test.** `cargo run -p kineto-provider-minimax --example speech_smoke` lists the voices free. With an output file and a voice ID it speaks one paid line. No test suite calls it.

A session's speech provider is `SessionOptions::speech_provider`. By default it follows the image provider: MiniMax with the same key, otherwise the local demo. `SpeechProvider::select` names one by `local-demo` or `minimax`, and a paid provider is used only when it is named.

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

The keyframe compiler is `keyframe-prompt/4` (`kineto-core/src/keyframe_prompt.rs`). It reads the scene and shot intent from `scene.json` and `shot.json`, and what production has locked, and compiles a still-image prompt for the provider's declared `ImageCapabilities`.

- The locked style's directives and note lead the prompt; without a locked style it asks for a photorealistic film still.
- A character whose cast is locked is shown from their canonical portrait, sent as an identity reference, up to as many references as the provider declares it takes in one image, in the shot's order.
- Every other named character is an identity-affecting drop, and the reason says why: not cast yet, one reference per image, or a provider that takes none.
- A cast character's forbidden changes are said while the prompt has room; dropping them is identity-affecting too.
- Camera motion, dialogue and duration are reported as cosmetic drops kept for the video step.
- A generation the director refines carries their note for that generation only, as `director.note`.
- Continuity comes from the locked bible and the shot's plan, each said while the prompt has room and reported when cut: what each character wears in this scene (its wardrobe change there, or its usual wardrobe), the location's look and what never changes there, the props the bible places in the scene, and the shot's own continuity notes. A shot that continues from another says so in words. The earlier shot's locked keyframe is sent as a composition reference (`continuity.previous_frame`) when the provider declares `ReferenceRole::Composition` and has a reference image left after the cast's identities, which come first; the keyframe then depends on that frame by its bytes (dependency field `continuity.previous_frame`) and is stale when the shot no longer continues from it. Otherwise, when the earlier shot has no locked keyframe, the provider takes no composition reference, or every reference image went to identities, the frame is not sent, a cosmetic drop that says which, and nothing depends on it. MiniMax takes no composition reference. A keyframe depends on every bible entry it read (`bible_character.<id>`, `bible_location.<id>`, `bible_prop.<id>`), and is stale when one changes.

The compiler refuses a shot without a written `action`, and it refuses a prompt longer than the provider accepts rather than truncating it. Identity drift is reported on the candidates; a project can block it instead (`identity_policy = "block"` under `[workflow]`), and then a generation that would show a character without an identity reference waits for the director's explicit consent.

The video compiler is `video-prompt/7` (`kineto-core/src/video_prompt.rs`). It reads the shot's planned intent and compiles the prompt of a clip that starts from the locked keyframe, for the provider's declared `VideoCapabilities`.

- The locked style leads, to be kept throughout the clip, and the clip is asked for as one continuous shot without cuts. The first frame alone does not hold a model to its look: in the P4 live run, Hailuo started from black-and-white noir keyframes and cut within a second to colour scenes that followed the action's words.
- The first frame carries framing, lens, setting, the characters' look and the shot it continues from; those are applied through it, not restated.
- Camera motion is said in the provider's own command where its control is parametric and it has one (MiniMax's bracketed commands, such as `[Push in]`), in words where it takes words, and reported as a cosmetic drop where it takes neither.
- The clip is the declared length nearest the plan's duration; a difference is reported.
- Dialogue is reported as dropped: lines are dubbed apart from the clip. A provider that makes its own sound (`audio`) is asked for no speech or dialogue, only the sound of the scene (`audio.no_speech`). Its clip is kept as the provider delivered it, sound included, so its bytes and hash are the provider's. The sound never reaches the timeline: the package's OTIO video track and EDL video events place the picture, and the dialogue track holds the dubbed lines.
- Mood, the shot's direction and its continuity notes are said while the prompt has room, cut by rank (continuity notes first, mood last) and reported when cut; a prompt whose required parts do not fit is refused.
- When the next shot in play continues from this one in the same setup and its keyframe is locked, the clip closes on that keyframe where the provider takes a last frame in its format and size, and in a clip of this length: the image is sent, and the prompt asks for the clip to end on it (`continuity.end_frame`). Continuing from a shot is not enough. The same setup means the same framing and lens (where both are planned), or a dolly in or out in this shot that carries its framing to the next one's on the scale from extreme wide to extreme close-up. Anything else is a cut, which is the editor's: in a live trial, Veo given a wide first frame and an over-the-shoulder last one made seven seconds of action and dissolved into the close shot in the last. The two locked keyframes must also look alike, since the plan says what was asked for and the keyframes are what the provider is given: in the live run, two shots planned as the same close-up on 85mm had keyframes MiniMax framed differently, and Veo dissolved again. Both keyframes are scaled to a 128 by 72 grey picture and compared by their structural similarity (`kineto-core/src/likeness.rs`); below 0.5 the next one is framed unlike this one, and nothing is sent. On the live runs, the pairs Veo dissolved between scored 0.13 and 0.32, a cut 0.21, frames of one static setup seconds apart 0.62 to 0.97, and two candidates MiniMax drew for one shot 0.24 to 0.37: keyframes drawn apart rarely share a setup, so a clip closes on the next shot only when its keyframe was made to match. A keyframe that cannot be read as an image (JPEG, PNG or WebP) is not compared, and not sent. Otherwise the compiler reports why not: the provider takes none, it is a cut (and why, including a likeness below the threshold), the provider closes on a last frame only at other lengths, the keyframe is not locked, it has no image, or the provider cannot take it.
- Every clip is asked for no on-screen text, titles, captions, graphics or overlays (`frame.no_text`): in the live run, a shot whose planned action ended "FADE OUT" came back with the words across the picture.
- A transition is the edit's. Shot planning keeps them out of an action and what must stay, but a plan locked before it did can still name one (FADE OUT, CUT TO, "the fade" and the like, the list shot planning checks). The compiler leaves out each clause of the action and of each continuity note that names one, reports it as a cosmetic drop (`shot.transition`), and asks for the picture not to fade, dissolve or cut away but to hold to the last frame (`frame.no_transition`); an action, or a set of notes, that is only a transition is dropped (`shot.action`, `shot.continuity`). In the live run, told to fade to black, Veo did, and laid a film leader's marks over the picture as it went; with the fade gone from the action alone, it faded again on a note that said "the fade is the only change in the image".

A video records the shot's intent when it was asked for, carried in the job's receipt, under `intent`, and the keyframe it closes on, if any. It is stale when its keyframe dependency no longer names the locked keyframe's bytes (`first_frame`), when the keyframe it closes on is no longer the next shot's locked keyframe or that shot no longer continues from it (`end_frame`), or when an intent field changed since (`intent.<field>`), and a stale video cannot be locked; locking judges the shots as they are then.

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

Kineto is an MCP client only (ADR-0007). It connects to stdio servers the user registers in Kineto's configuration, calls only the tools each entry allows, and only when the director asks. It offers no sampling, roots or elicitation. A tool's text result becomes a reference: project state under `references/`, with its source, attached to the world or to bible entries, given to the bible's prompts as material, and recorded as a dependency so a changed reference makes the bible stale.

## Secrets

Secrets are stored outside project files.

A key is read from the system's secure storage first (`kineto-keychain`): the macOS Keychain, the Windows Credential Manager, or a Secret Service (GNOME Keyring, KWallet, KeePassXC) on Linux, reached over D-Bus without a system library. Each key is one credential, service `kineto`, account its name: a built-in provider's (`minimax`, `gemini`, `kimi`, `veo`), the name a connection gives its key (`deepseek`), or a secret a plugin's or MCP server's entry lists (`STUDIO_IMAGES_KEY`). When the storage holds none, or cannot be read, the environment is read instead, for development and CLI use: `KINETO_<NAME>_API_KEY` for a provider's or server's key, the variable of its own name for a plugin's or server's secret. A locked or missing storage never stops a project opening. A session reads its keys on its own worker before the open, never on the UI thread, so a storage that waits (a locked keyring's unlock prompt) delays only that open. There is no file fallback, encrypted or not: where the system offers no secure storage, settings say why and keys come from the environment only.

Settings manage the keys by name (`kineto_keys_*`, independent of any project, called off the UI thread because the storage can wait on the user): the built-in providers', the chosen server's, and every secret a registered plugin or MCP server lists; where each is (saved, from the environment, or not set), and a field to save, replace or remove one. A key crosses into the engine once, from a buffer Dart clears afterwards; it is never read back across the boundary or shown again. Projects opened after a change use it.

Never write API keys or authentication material into:

- project files
- logs
- generated prompt files
- issue reports
- exported artifacts
- telemetry

Secret-redaction behavior requires automated tests.
