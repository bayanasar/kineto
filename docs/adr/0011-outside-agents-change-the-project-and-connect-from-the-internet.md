# ADR-0011: Outside Agents Change the Project, and Connect from the Internet

- Status: Proposed
- Date: 2026-10-11
- Amends: [ADR-0010](0010-mcp-server-for-outside-agents.md)

## Context

ADR-0010 let agents outside Kineto read the open project over MCP, at an address on this computer, and left two questions to the director: may an agent change the project, and how may one in the cloud connect. The director answered both: agents write, and ChatGPT and claude.ai connect too.

ChatGPT's and claude.ai's connectors call from their own servers. They need an HTTPS address on the internet, and they authorize with OAuth 2.1 as MCP's authorization specification describes: protected resource metadata, authorization server metadata, dynamic client registration, PKCE with S256, and the `resource` parameter. Both accept a server that is its own authorization server.

Kineto's paid-call rules hold whoever asks. A paid request is sent only on the director's decision, and one whose outcome is unknown is not sent again without the director's consent to pay twice.

## Decision

### Writes go through the open session

An agent's command runs on the open project's session worker, under its writer lock, with the director's own commands: the same refusals, records and recovery. The session hands agents a `SessionRemote`, which queues commands and reads what the worker published. Holding one does not keep the session open. An agent's command carries a token with bit 62 set; the director's commands are counted from 1. The session's sink is told when an agent's command starts (`agent_started`), and its outcome reaches both the agent and the window.

With no project open, a changing tool says so. The reading tools still read the project Kineto opened last.

### What runs at once, and what waits for the director

- **At once.** Edits, choices and locks, and calls that cost nothing per call: the free demo generator, a local model, and an agent CLI the director's subscription covers.
- **After the director allows it.** A call that pays per call: a metered or unpriced document model, images other than the demo's and a signed-in agent's, and any video other than the demo's. The request waits at the bottom of Kineto's window, whatever workspace is open, with who asks, what it asks and what Kineto knows of the price. Allowed, it is sent as the director's button would send it. Declined, the agent is told so.
- **Never.** Sending again what may already have been paid for (resubmitting), reading a file outside the project (importing a portrait or a style), calling other MCP servers, exporting a package and collecting media stay with the director.

A tool call answers within 45 seconds. A command still running, or a request still waiting, answers with a request number, and `get_request` says later how it ended: `awaiting_director`, `declined`, `lapsed`, `running`, `done` or `failed` with why. A request lapses after 30 minutes unanswered, or when Kineto closes the project. Requests are kept in memory, the latest 200.

### The tools

Reading: `get_project`, `get_story`, `get_document` and `get_shot` (ADR-0010; `get_shot` now lists the latest video takes); `get_production`, the style presets and samples and each character's portraits; `get_image`, a keyframe candidate, style sample or portrait as a JPEG at most 1024 pixels on its longest side, since an agent in the cloud cannot open a path on this computer; and `get_request`.

Changing: `import_story`, `analyze_story`, `write_screenplay`, `rewrite_scene`, `edit_scene`, `lock_screenplay`, `write_bible`, `edit_bible_entry`, `lock_bible`, `plan_shots`, `replan_scene`, `edit_scene_shots`, `lock_shot_plan`, `sample_style`, `choose_style`, `lock_style`, `audition`, `cast_portrait`, `direct_shot`, `make_keyframes`, `choose_keyframe`, `lock_keyframe`, `make_video`, `choose_video` and `lock_video`. An edit changes only the fields it gives and builds on the latest revision; a lock names the latest when no revision is given. Voices, dialogue and preferences are not reached yet.

Reading tools are annotated `readOnlyHint`. Those that ask a model or a provider are annotated `openWorldHint`.

### Agents in the cloud: a second listener, OAuth, the director's tunnel

Settings take a public address, the HTTPS origin the director's tunnel answers at (such as `https://computer.tailnet.ts.net:10000`), and optionally the command that opens the tunnel, one word a line, `{port}` for the port Kineto listens on. Turned on while Kineto serves, Kineto listens on a second port on `127.0.0.1` and runs the command; turned off, or when Kineto closes, both stop, the tunnel first asked to end (SIGTERM) so it can take down what it set up. On Linux the tunnel also ends if Kineto dies. Kineto names no tunnel vendor: Tailscale Funnel, Cloudflare Tunnel, ngrok or a reverse proxy all serve.

That listener takes only OAuth access tokens; the local address's token is not accepted there, whatever `Host` the tunnel sends. It serves:

- `/.well-known/oauth-protected-resource` (and `/mcp` after it): the resource `<origin>/mcp`, its authorization server, scope `kineto`.
- `/.well-known/oauth-authorization-server` (and `/.well-known/openid-configuration`): endpoints, `code` only, PKCE `S256` only, public clients (`none`), and `authorization_response_iss_parameter_supported`.
- `/register`: dynamic client registration. Redirect URIs must be HTTPS, or HTTP to this computer, without a fragment.
- `/authorize`: the authorization code flow. The agent's request waits in Kineto's window ("ChatGPT asks to connect from the internet, returning to chatgpt.com"), never on a web page anyone could open; the browser page reloads itself until the director answers, then returns to the agent with a code or `access_denied`, and with `iss`. At most five wait at once, since anyone can ask through the public address; a sixth is told to try later, and one unanswered for ten minutes lapses and leaves the window.
- `/token`: a code with its verifier (once, within 60 seconds), or a refresh token, for an access token of one hour and a refresh token, both turned over at each refresh. A `resource` other than this server's is refused.
- `/mcp`: MCP as on the local address, with `Authorization: Bearer`. Without a valid token it answers 401 with `WWW-Authenticate` naming the resource metadata.

Tokens are 256 random bits; only their SHA-256 hashes are kept, in `mcp-agents.json` beside `mcp-server.toml`, readable by its owner alone. Settings list the agents the director allowed, and disconnect one, which forgets its tokens. A request with an `Origin` other than the public origin is refused.

### Native ABI 54

- `kineto_mcp_server_asks_load`, `_count`, `_value`, `_text_copy` and `_destroy` read what waits for the director; `kineto_mcp_server_decide` answers it. They touch only memory.
- `kineto_mcp_server_public_load`, `_value`, `_text_copy` and `_destroy` read where agents in the cloud stand; `kineto_mcp_server_set_public` and `kineto_mcp_server_disconnect` change it. They read and write files, and set may start the tunnel, so Dart calls them off the UI thread.
- Session messages `AGENT_STARTED` (6, with the command's label) and `AGENT_FINISHED` (7, as `COMPLETED`). The window shows an agent's command in the status bar as one of the director's own, and every page reads the project again when it ends.
- Status 176 for a public address that is not an HTTPS origin, or a tunnel command that is not a program and its arguments.

## Consequences

- An agent can change the project as the director can, short of paying, risking paying twice or reading the director's files. Every change is a new revision or a recorded choice; nothing is deleted.
- An agent in the cloud reaches the project only through a tunnel the director chose, after the director allowed that agent. Anyone can reach the authorization page through the tunnel, and each such request reaches the window; only the director's answer lets an agent in.
- The address an agent in the cloud holds is not a secret; its tokens are. Disconnecting it, or turning the public address off, shuts it out.
- `get_image` sends the project's images to wherever the agent runs.
