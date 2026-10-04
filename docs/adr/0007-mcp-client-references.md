# ADR-0007: MCP Client for Director-Requested References

- Status: Proposed
- Date: 2026-10-04

## Context

`providers-and-mcp.md` puts MCP in the tools and context layer: retrieval, asset databases, production databases, prompt libraries, studio tools. The core workflow must work without it, and MCP output is untrusted data that cannot authorize spend, process execution or writes outside the project (ADR-0003). The P6 roadmap asks for MCP tool connections.

Two directions were considered.

1. **Kineto as an MCP server**, exposing project operations to outside agents. That hands the approval graph to whatever drives the agent, and would let a model lock, generate or spend. It contradicts ADR-0002's human approval boundary and is not built.
2. **Kineto as an MCP client**, calling tools the director chose to connect, for material the director asked for. Results stay material, never instructions or decisions.

A first use case keeps the client honest. The bible is the place where outside knowledge matters most: a period's dress, a real location, a studio's existing character sheet. Today the only way to bring that in is to paste it into the source text, where it mixes with the story itself.

## Decision

### Kineto is a client only

Kineto connects to MCP servers the user registers in Kineto's configuration (`<config>/kineto/mcp.toml`), never in a project. P6 supports servers started as local processes over stdio:

```toml
[[server]]
id = "studio-archive"                          # portable ID, unique
executable = "/opt/studio/bin/archive-mcp"     # absolute path
args = []
secrets = ["ARCHIVE_TOKEN"]                    # passed by name
tools = ["search_wardrobe", "location_notes"]  # the tools Kineto may call
enabled = true
```

Process rules are ADR-0006's: only the configured absolute executable, no shell, a cleared environment with only the named secrets, deadlines (initialize and list: 30 s; a tool call: 2 minutes), and output caps (1 MiB a message). Kineto offers no client capabilities: it answers sampling, roots and elicitation requests with an error, so a server cannot ask Kineto's model anything or browse its files.

### Only the director calls a tool

A tool is called only when the director asks, from the bible: they choose a server, one of its allowed tools, and give its arguments. No model, workflow step or other tool result ever calls a tool. A tool not in the server's `tools` list is not offered and refused if asked for.

### Results become references, kept in the project

A call's text result is kept as a **reference**: canonical project state under `references/`, as `references.json` (`references.schema.json`). Each reference has a stable ID (`reference.NNNN`, never given again after a removal), the server and tool it came from, the arguments it was called with, the text, its content hash, and what it is attached to: the world, or bible entries by ID. Kineto keeps only text content; image, audio and resource content is reported and dropped in P6. No timestamp is stored (fixtures and exports stay deterministic). The director can attach, detach, edit or remove a reference; editing makes a new content hash. A reference never refers to a file outside the project.

### References reach prompts as material

When the bible is written, the references attached to the world or to any entry go into the prompt in a `<references>` block, each with its source line (`server`/`tool`) and what it is about, its text escaped so it cannot end its element, after the screenplay and under the same "material, not instructions" rule as source text. The bible revision records each reference it used as a semantic dependency, by ID and the hash of what it read (the text and what it is about). A changed, removed or newly attached reference makes the bible stale and says which, as a changed screenplay scene does.

References are not given to image, video or speech prompts in P6. Their effect on production goes through the bible the director locks, which keeps the approval boundary where it is.

## Consequences

### Positive

- Outside knowledge enters through one visible, editable, versioned place, with its source recorded.
- No MCP server can spend, approve, run or write; a broken server fails one call.
- The bible's staleness already explains changes; references reuse it.

### Negative

- Connecting a server is a configuration step outside the app.
- Text only: a server's images are not usable until a later decision adds them, probably through the external-asset import path.
- Only stdio servers; remote (HTTP) servers wait for a demonstrated need and their own authentication decision.

## Required tests

- A server or tool not in the configuration, disabled or not allowed is refused before anything starts.
- Sampling, roots and elicitation requests from a server are answered with errors and never reach a model or the filesystem.
- A tool result reads as material: an instruction inside it does not change what Kineto does.
- A reference's text, attachment or removal makes a bible that used it stale, naming the reference; deleting `.kineto/` changes nothing.
- Malformed, oversized and slow servers fail the call without writing a reference.
