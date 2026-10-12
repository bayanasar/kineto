# ADR-0010: Kineto as an MCP Server for Outside Agents

- Status: Proposed
- Date: 2026-10-10
- Amended by: [ADR-0011](0011-outside-agents-change-the-project-and-connect-from-the-internet.md), which lets agents change the project and agents in the cloud connect

## Context

ADR-0007 made Kineto an MCP client: it calls tools of servers the director registers. The director asked for the other direction too: let agents outside Kineto (Claude Code, Codex, Claude Desktop, ChatGPT) connect to Kineto, with a switch in Settings and, while it is on, an address that holds a hash.

Those agents speak MCP. Claude Code, Codex and Claude Desktop reach a server on this computer over Streamable HTTP. ChatGPT and claude.ai reach only servers on the internet, over HTTPS, from their own clouds.

A service on a loopback port is not private by itself (ADR-0003). Any program on the computer can connect, and a web page in a browser can, through a name rebound to `127.0.0.1`. ADR-0003 already sets the rules for a local network transport: opt-in, loopback only, a token on every call, `Origin` and `Host` checked and refused even when the token is right, and no way to discover the token.

## Decision

### Opt-in, on this computer, at a capability address

Settings have a switch, off by default. Turned on, Kineto listens on `127.0.0.1` and shows its address, `http://127.0.0.1:<port>/mcp/<token>`. The token is 256 random bits as 64 hex digits, and only that path answers. Any other path is answered 404, and the token is compared in constant time. The address is the credential: an agent that has it can read the project, and one that does not cannot learn it.

- **Host and Origin.** A request whose `Host` is not `127.0.0.1`, `localhost` or `[::1]` at that port, or that carries an `Origin` other than those, is refused with 403, even with the token.
- **Same address across restarts.** Whether it is on, its port and its token are kept in `mcp-server.toml` beside the provider settings. The file is written readable by its owner alone; `providers.toml` keeps no secret. Kineto serves again each time it starts, until the director turns it off.
- **New address.** "New address" makes a new token on a free port, and the old address stops working at once.
- **Port taken.** If its port is taken when Kineto starts, Settings say so (status 175) and offer a new address. It is never moved to another port quietly, because the agents the director pointed at it would lose it.

Settings give the address with a copy button, the Claude Code command (`claude mcp add --transport http kineto <address>`) and the Codex `config.toml` entry. They also say that ChatGPT and claude.ai need an internet address, which Kineto does not open; a tunnel the director runs can.

### Streamable HTTP, JSON answers

It speaks MCP 2025-11-25, 2025-06-18 and 2025-03-26 over Streamable HTTP. It answers the version the client asks for when it knows it, else the newest.

- **POST.** A POST carries one JSON-RPC message. A request is answered with `application/json`; a notification or a response is answered with 202.
- **GET.** It opens no stream (405), because it sends nothing unasked.
- **Sessions and batches.** It keeps no session, so it hands out no `Mcp-Session-Id`, and it refuses batches.
- **Limits.** A body is at most 1 MiB, a head 16 KiB, and a request with no `Content-Length` is refused.

### Tools that only read

| Tool | What it reads |
|---|---|
| `get_project` | The project's title, each recipe step and where it stands, and each scene's shots with their keyframe and video stages. |
| `get_story` | The source text (at most 200,000 characters) and the current story analysis. |
| `get_document` | The current revision of the screenplay, bible, shot plan or preferences: the locked one, or the latest candidate. |
| `get_shot` | One shot in play: its intent, its keyframe candidates and which is chosen or locked, its video state, and its files as paths on this computer. |

Each tool is annotated `readOnlyHint`. It opens the project read-only (`CanonicalProject::open_read_only`), taking no writer lock beside the session that holds it. It reads files that session replaces whole, so it sees one version or the next, never half of one. It reads the project Kineto opened last; with none, it says so.

Nothing an outside agent sends changes the project, spends money, or reaches a provider. Writing tools, and paid ones above all, need their own decision: what an agent may change, and how the director consents to what it would spend.

## Consequences

- An agent on this computer that the director gave the address can read the whole project, its media paths included. The address is shown only in Settings and kept only in a file its owner alone reads.
- Agents in the cloud (ChatGPT, claude.ai) cannot connect without a tunnel the director runs, and Kineto does not run one.
- The server runs inside Kineto: it serves while Kineto is open, and stops when Kineto closes.
- The native ABI gains `kineto_mcp_server_resume`, `kineto_mcp_server_set_enabled`, `kineto_mcp_server_renew` and `kineto_mcp_server_address_copy`, and status 175 (v53).
