# ADR-0008: The Director's Own Agent CLIs, and Provider Choice in Settings

- Status: Proposed
- Date: 2026-10-08

## Context

Directors already pay for agent subscriptions and are signed in to their command-line tools: Claude Code, Codex, Kimi Code, Antigravity (`agy`), Mini-Agent. Kineto drove one of them, Kimi Code, and only under a data root of its own, which meant signing in a second time. Everything else was closed:

- keys could be kept in the system's secure storage only for four built-in providers (`minimax`, `gemini`, `kimi`, `veo`);
- a plugin's secrets came from the environment only, though ADR-0006 says the secure storage comes first;
- the OpenAI-compatible backend sent no key, so a hosted server (DeepSeek, OpenRouter, OpenAI) could not be used;
- every provider was chosen by an environment variable, which a director who opens Kineto from the Dock, Finder or Start menu cannot set.

A director could use Kineto only with the vendors we had wired in, and only from a terminal.

Each CLI's non-interactive mode was measured on this machine with the director's own sign-in (Claude Code 2.1.295, Codex 0.160.0, Kimi Code 2.1.1, agy 1.2.16, Mini-Agent at `d76a4f6`). The measurement used a prompt that also asks the agent to run `ls` and read a file. Without restrictions, Codex in its read-only sandbox, and agy under the director's own permission rules (which allow `cat`, `rm -rf` and `git push --force`), both read the file. Each needed its own way to start without tools, and some cannot turn everything off.

## Decision

### Kineto drives the CLI the director already signed in to

An agent is found where it is installed: on the search path, in the places installers put CLIs (because a desktop app does not get the shell's path), or where the director's settings say. Kineto never signs in for it, never reads a credential it keeps, and never asks for a second sign-in. Every call runs in a fresh empty working directory, removed afterwards, gets the conversation as one prompt, and is started without tools:

| Agent | How it is started | What cannot be turned off |
|---|---|---|
| Claude Code | `-p`, `--tools ""`, `--setting-sources ""` (none of the user's settings, so none of their hooks or plugins), `--strict-mcp-config`, `--no-session-persistence`, prompt on stdin, schema as `--json-schema` | nothing measured |
| Codex | `exec --ignore-user-config --ignore-rules --ephemeral --sandbox read-only`, every tool feature disabled, prompt on stdin | nothing measured |
| Kimi Code | `-p` with an agent file of kineto's that allows no tools (`tools: []`) | the user's own hooks run; the prompt is an argument, at most about 120 KB (28 KB on Windows) |
| agy | an agent defined in the working directory, its only tool the one that hands back the answer (`finish`), no default prompt, no MCP, no rules | the user's own hooks run |
| Mini-Agent | its ACP server, started in its own Python with every tool switched off | it spends the director's MiniMax key per request |

Mini-Agent signs in by holding a MiniMax key in its own configuration. Kineto starts it so that the key is read by Mini-Agent's own code in its own process, never by kineto.

What a CLI does not let kineto turn off is said where the director chooses it. Agent answers are untrusted like any model output: they are checked against the schema before they become state (ADR-0003), and a call's cost is unknown (a subscription) except Mini-Agent's, which is metered.

### An agent that makes images is a built-in plugin

agy makes images, with reference images, through its `generate_image` tool. It is offered as an image provider by playing a plugin's part (ADR-0006) inside Kineto: the same invoke contract, records kept under `agent.agy`, written before and after the call, and the same refusal and recovery rules. The agent it runs has only `generate_image` and `finish`. Reference images are copied into the working directory and named in the request. The image is read from where agy keeps it, named by the conversation. Codex reports image generation as available, but neither `exec` nor its app server produced an image when measured, so it is not offered for images.

### Keys are kept by name

The secure storage keeps any key under its name:

- a provider's (`minimax`);
- the name the director gave the key an OpenAI-compatible server takes (`deepseek`, read from `KINETO_DEEPSEEK_API_KEY` when none is kept);
- a secret a plugin's or MCP server's entry lists (`STUDIO_IMAGES_KEY`, read from the variable of that name when none is kept).

Plugins and MCP servers read their secrets from the secure storage first, as ADR-0006 intended. The native key calls take the name, not a code from a closed table.

### Providers are chosen in settings

The director's choices live in `<config>/kineto/providers.toml`, beside `plugins.toml`. The file holds the image, video, speech and language-model choices, the OpenAI-compatible server's address, model and key name (never the key), and where an agent is when Kineto would not find it. Settings show and change it: each agent as installed or not found, each plugin by what it makes, and the server's fields. A session reads the file when it opens. An environment variable that is set still wins, for development, and settings say so. The file keeps what a newer build wrote that this one does not know.

## Consequences

- Any provider a director can reach is usable: a built-in, a plugin, any OpenAI-compatible server with its key, or an agent CLI they already pay for.
- Kimi Code no longer gets a sandboxed data root, so the user's hooks run and long requests are refused rather than truncated. The trade is no second sign-in. Claude Code and Codex lose nothing.
- Each agent's flags are measured against one version. A CLI that renames or drops a flag fails the call with its own words rather than running with tools. The isolation is re-measured when one is upgraded.
- The macOS app leaves the App Sandbox (both entitlements files). In it, the app's home is its container, so an agent CLI it starts cannot see the director's sign-in, and it had no outgoing network either, so no network provider worked on macOS. Kineto is distributed directly, not through the App Store, so it runs with the director's own user rights, as on Linux and Windows. An App Store build would need another way to reach the agents.
- The native ABI gains provider settings and agent discovery, and the key calls change shape (v47).
