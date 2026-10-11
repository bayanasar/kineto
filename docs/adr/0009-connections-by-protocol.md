# ADR-0009: Connections: Any Service, by the Protocol It Speaks

- Status: Proposed
- Date: 2026-10-10

## Context

ADR-0008 opened provider choice: built-in providers, plugins, the director's signed-in agent CLIs, and one OpenAI-compatible server for documents. The director asked for more: most of the services on the market, not the few that were wired in, entered by the director, grouped by what they make (text, images, video).

Services share far fewer request formats than they have vendors:

- **Text.** OpenAI's chat completions are spoken by OpenAI, xAI (Grok), Meta's Model API (Muse), Zhipu (GLM), Alibaba (Qwen), DeepSeek, Moonshot (Kimi), ByteDance (Doubao), MiniMax, Google's Gemini API through its compatibility address, OpenRouter, and local servers (Ollama, LM Studio, vLLM). Anthropic's messages are spoken by Claude's API and by the Anthropic-compatible addresses several of those vendors also serve.
- **Images.** OpenAI's images (`/images/generations`, `/images/edits`) are spoken by OpenAI, xAI and several others; Gemini's image models answer `generateContent`. Other services (Alibaba's DashScope, for one) use their own task formats.
- **Video.** Most services have their own task format: create a task, ask about it, fetch a file. Veo, Hailuo, Kling, Sora and Grok Imagine each differ. Seedance is the exception that matters: Volcengine's Ark (in China) and BytePlus's ModelArk (elsewhere) serve it through one task API (`/contents/generations/tasks`), and the gateways that resell it speak that API too.

A vendor list in the app (one entry per company) would be closed the day it shipped. A protocol list stays open: a new service that speaks a known protocol needs no change to Kineto.

Signing in with a consumer account was asked about too. Meta's Model API and the Gemini API take keys, not a Facebook or Google sign-in; neither lets a third-party app spend a consumer subscription. The only route to a signed-in subscription stays ADR-0008's: drive the vendor's own CLI (Codex for a ChatGPT plan, Claude Code for a Claude plan, agy for a Google one), never borrow its credentials.

## Decision

### A connection names a protocol, an address, a model and a key

`providers.toml` keeps any number of connections:

```toml
[[connection]]
name = "deepseek"              # how it is chosen: connection:deepseek
protocol = "openai-chat"       # openai-chat, anthropic-messages, openai-images, gemini-images, ark-video
url = "https://api.deepseek.com/v1"
model = "deepseek-chat"
key = "deepseek"               # the name its key is kept under; none for a keyless local server
price_per_million_input = 0.28 # USD, optional
price_per_million_output = 0.42
```

An image connection gives `price_per_image` and `references` (how many reference images it takes, 0 to 16) instead of token prices, and a video connection gives `price_per_second`. The director adds, changes and removes connections in Settings, typing the key in the same form; the key goes to the system's secure storage under its name (ADR-0008), never into the file. A choice (`language_model`, `image`, `video`) names one as `connection:<name>`. Renaming a connection carries the choice with it; removing it clears the choice and leaves the key.

The address is HTTPS, or HTTP to this computer, as for every network call. A file an answer points at, and every redirect on the way to it, is checked against the same rule before it is fetched, and may be on this computer only when the connection is: a service elsewhere never sends kineto to one on this computer. A connection's calls are recorded under `connection.<name>`.

### What the director did not say is unknown

A connection without prices is metered at an unknown price: every cost shown says so, and the paid-call rules (a call whose outcome is unknown is not sent again without the director's consent to pay twice) apply as to any paid provider. Only a keyless connection to this computer is free. An image connection takes no reference images unless the director says how many; without them it cannot lock a cast, which needs an identity reference.

### Text: two protocols

`openai-chat` uses the existing chat-completions backend. `anthropic-messages` posts to `<url>/v1/messages` (or `<url>/messages` when the address ends in `/v1`) with `x-api-key` and `anthropic-version: 2023-06-01`, the system prompt apart from the turns, and the schema instruction in it. Both are checked against the schema like any model answer.

### Images: a connection plays a plugin's part

As agy does under ADR-0008, an image connection is a built-in plugin: the same invoke contract, records written before and after the call, the same refusal evidence and recovery. A call that certainly made nothing (refused before sending, a connection never made, a 4xx answer, an answer without an image) is recorded as refused; any other failure leaves the call unresolved until the director consents to send it again.

Fields a service needs beyond the protocol's common part (Seedream's watermark switch, a size of its own) are written by the director under `[connection.request]` in the settings file, sent with every request, and win over kineto's. Settings show their names and never change them.

### Video: Ark's video tasks

A video connection speaks `ark-video`: `POST <url>/contents/generations/tasks`, then `GET` the task until it has succeeded and gives `content.video_url`, a file fetched without the key within 24 hours. It is a video provider like MiniMax or Veo, under the same write-ahead jobs: the task is recorded the moment it is accepted, a restart follows it again and never submits, and a submission whose outcome is unknown waits for the director's consent to pay twice. A 4xx answer or a connection never made proves nothing was started.

- **Request.** The prompt, at most 2,000 characters (the service takes 500 Chinese characters or 1,000 English words), the shot's locked keyframe as `first_frame` and, when the next shot continues from it, that shot's keyframe as `last_frame`, both as data URLs; the clip's length (4 to 15 seconds), its frame shape as `ratio` (16:9, 9:16, 4:3, 3:4, 1:1 or 21:9, else `adaptive`) and `resolution` (720p).
- **The director's fields.** `[connection.request]` is sent with every task and wins, as for images: `generate_audio = false` for a silent clip (the clip then declares no sound track), `resolution = "1080p"` for taller clips (the clip then declares that height).
- **Price.** `price_per_second` times the clip's length, for the height the connection renders; unknown when not given. Ark bills tokens that grow with a clip's pixels, so a price per second holds for one height only.
- **No references.** A shot's clip starts from its keyframe, which already carries who is in it; a video connection takes no reference images.

Other video services keep their own adapters (MiniMax, Veo, Gemini Omni) and plugins (ADR-0006).

### The old server becomes a connection

The single server ADR-0008 kept (`[openai_compatible]`, chosen as `openai-compatible`) is read as a connection named after its key, and the choice follows it. `KINETO_LLM_PROVIDER=local` with `KINETO_LOCAL_LLM_*` stays for development.

## Consequences

- A service that speaks a known protocol is usable without a new build: GPT, Grok, Muse, GLM, Qwen, DeepSeek, Kimi, Doubao, MiniMax, Gemini, Claude, OpenRouter, local servers.
- Kineto cannot know a connection's price or limits. It shows what the director entered and says when it does not know.
- A vendor's own extras (structured-output modes, image sizes, safety settings) are not reached; the protocol's common part is. A service that needs more is better served by a plugin.
- The native ABI gains connection calls and retires the server's three settings (v50), then image connections and whether an image price is known (v51), then video connections (v52).
- An image of another shape than the shot's frame is cut to it, which keeps the frame and loses the edges; a service that draws the frame asked for loses nothing.
