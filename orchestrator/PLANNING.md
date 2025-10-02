# Codex Orchestrator Planning

This document describes a robust design for the Codex Orchestrator MCP server. It details the architecture, module boundaries, request/response routing, process lifecycle, error handling, and a practical testing strategy. All MCP I/O is handled by the `rmcp` crate and isolated to `src/mcp.rs`. All Codex MCP communications are isolated to `src/codex.rs`. Types for Codex messages are currently provided by minimal local structs in `src/protocol_types.rs` (sufficient for basic wiring); fuller Codex protocol types may be integrated later if needed.

Status: initial plan; subject to iteration as features mature.


## Objectives

- Expose a small MCP tool surface to spawn/manage Codex agents and proxy conversation operations.
- Use `rmcp` for all MCP (JSON‑RPC over Content‑Length framing) and keep it contained in `src/mcp.rs`.
- Use minimal local types for Codex-facing requests and responses in `src/protocol_types.rs`. If richer fields are required (e.g., message items, prompts, approvals), consider integrating a dedicated protocol crate behind a feature flag.
- Correctly forward Codex event streams and approval requests to the upstream MCP client.
- Avoid pitfalls: request ID routing, backpressure, process lifecycle bugs, partial framing, timeouts.


## Architecture Overview

- Roles
  - Orchestrator runs as an MCP server over stdio to the host (any MCP client).
  - Orchestrator manages one or more Codex MCP servers, each spawned as a subprocess. For each agent, the orchestrator is an MCP client to that subprocess.

- Transports
  - Upstream (host ↔ orchestrator): `rmcp` server over stdio (Content-Length framed JSON‑RPC 2.0).
  - Downstream (orchestrator ↔ codex): spawn `codex mcp` subprocess with stdio pipes; wrap with an `rmcp`‑compatible client transport.

- Type system
  - Upstream MCP: `rmcp` request/notification abstractions with `serde` for tool inputs/outputs.
  - Downstream Codex: local placeholder types in `src/protocol_types.rs` for request/response shapes. If the Codex agent expects richer payloads (e.g., `items` with text/images), expand local types or reintroduce a protocol crate under a gated feature.

- Concurrency
  - One `tokio` task per Codex agent for the read loop (child stdout → JSON‑RPC decode → route).
  - `rmcp` server task for upstream handling.
  - Internal bounded channels to bridge Codex notifications/approvals to upstream.


## Module Layout

- `src/main.rs`
  - Sets up logging, reads env/config, initializes shared `OrchestratorState`, and runs `mcp::run_server()`.

- `src/mcp.rs`
  - Owns the MCP server (`rmcp`), tool registration, request handlers, and notifications to the upstream client.
  - Exposes helpers used by `codex.rs` to forward Codex events and relay approval requests upstream.

- `src/codex.rs`
  - Spawns/attaches to Codex MCP servers, provides typed client calls using `codex_protocol::mcp_protocol`.
  - Bridges Codex notifications and server→client approval requests via `mcp.rs`.

- `src/state.rs` (optional)
  - `OrchestratorState` with `HashMap<String, AgentHandle>` and helpers for lifecycle and lookup.

- `src/error.rs` (optional)
  - Central error types; conversions to `anyhow` or structured tool errors.


## MCP Server (`src/mcp.rs`)

Responsibilities:

- Initialize `rmcp` MCP server over stdio; use `transport-io` feature.
- Advertise tools with typed inputs/outputs (serde):
  - `spawn_agent { id?: string, cwd?: string } -> { agentId: string }`
  - `list_agents {} -> { agentIds: string[] }`
  - `kill_agent { agentId: string } -> {}`
  - Conversation operations (forwarded to the specified agent):
    - `new_conversation { agentId: string, params: NewConversationParams } -> NewConversationResponse`
    - `send_user_message { agentId: string, params: SendUserMessageParams } -> SendUserMessageResponse`
    - `send_user_turn { agentId: string, params: SendUserTurnParams } -> SendUserTurnResponse`
    - `interrupt { agentId: string, params: InterruptConversationParams } -> InterruptConversationResponse`
  - Optional pass‑through helpers (if desired):
    - `get_user_saved_config`, `set_default_model`, `get_user_agent`, `user_info`
    - `login_api_key`, `login_chat_gpt`, `cancel_login_chat_gpt`, `logout_chat_gpt`, `get_auth_status`
    - `git_diff_to_remote`, `exec_one_off_command`

- Forward downstream Codex notifications upstream:
  - Method `codex/event` with a JSON payload (currently forwarded as opaque JSON without TS bindings).
  - Auth notifications: `authStatusChange`, `loginChatGptComplete` with their original payloads.

- Forward approvals (Codex → orchestrator → upstream host):
  - Requests: `applyPatchApproval`, `execCommandApproval` (see `protocol/src/mcp_protocol.rs`).
  - Relay to upstream as JSON‑RPC requests with same method/params; await decision; respond back to Codex.

Suggested APIs:

- `pub async fn run_server(state: OrchestratorState) -> anyhow::Result<()>`
- `pub async fn notify_codex_event(agent_id: &str, event: codex_protocol::protocol::Event) -> anyhow::Result<()>`
- `pub async fn request_apply_patch_approval(params: ApplyPatchApprovalParams) -> anyhow::Result<ApplyPatchApprovalResponse>`
- `pub async fn request_exec_command_approval(params: ExecCommandApprovalParams) -> anyhow::Result<ExecCommandApprovalResponse>`

Implementation notes:

- Keep `rmcp` setup/dispatch localized to this file.
- Use strongly‑typed serde structs for tool IO; avoid `serde_json::Value` where possible.
- Avoid blocking; call into `codex.rs` and state methods asynchronously.
- Provide graceful shutdown: terminate agents, drain tasks, close transports.


## Codex Client (`src/codex.rs`)

Responsibilities:

- Discover Codex binary: `CODEX_BIN` or fallback to `codex` via `which`.
- Spawn `codex mcp` as a child process per agent with stdio pipes.
- Wrap child pipes with a JSON‑RPC client using `rmcp` framing.
- Provide typed client calls using `codex_protocol::mcp_protocol`:
  - `new_conversation`, `send_user_message`, `send_user_turn`, `interrupt_conversation`, etc.
- Read loop per agent:
  - Responses: complete the matching pending request.
  - Notifications: forward `codex/event` and auth notifications to `mcp.rs`.
  - Server→client requests (approvals): call `mcp::request_*_approval`, await decision, respond to Codex.

Core types (sketch):

- `struct CodexAgent { id: String, child: tokio::process::Child, rpc: JsonRpcClient, read_task: JoinHandle<()>, … }`
- `struct CodexClient { agents: HashMap<String, Arc<CodexAgent>> }`

Suggested APIs:

- `pub async fn spawn_agent(id: Option<String>, cwd: Option<PathBuf>) -> anyhow::Result<AgentHandle>`
- `pub async fn new_conversation(agent: &AgentHandle, params: NewConversationParams) -> anyhow::Result<NewConversationResponse>`
- `pub async fn send_user_message(agent: &AgentHandle, params: SendUserMessageParams) -> anyhow::Result<SendUserMessageResponse>`
- `pub async fn send_user_turn(agent: &AgentHandle, params: SendUserTurnParams) -> anyhow::Result<SendUserTurnResponse>`
- `pub async fn interrupt_conversation(agent: &AgentHandle, params: InterruptConversationParams) -> anyhow::Result<InterruptConversationResponse>`

Read loop details:

- Single `tokio` task per agent reading framed messages from child stdout.
- If message is a response: resolve pending request (per‑agent ID map).
- If notification: call `mcp::notify_codex_event` or forward auth notifications.
- If server→client request: forward to `mcp.rs` for a decision; send reply.
- On fatal error/exit: clean up agent state, cancel pending requests, surface an event upstream.


## Request/Response Routing

- Upstream tool → orchestrator:
  - Deserialize input, locate `AgentHandle`, call `codex.rs`, return typed result to MCP `result`.

- Downstream `codex/event` → upstream client:
  - Forward as `codex/event` notification with `codex_protocol::protocol::Event` payload.

- Approvals (Codex → orchestrator → upstream):
  - Methods: `applyPatchApproval`, `execCommandApproval`.
  - Params include `conversation_id` and `call_id`; preserve for correlation with events.
  - Await upstream decision (with timeout policy) and return to Codex.


## Process Lifecycle

- Spawn
  - Resolve `CODEX_BIN` (env) or `which("codex")` fallback.
  - Spawn `codex mcp`; set working directory if provided.

- Health checks
  - Fail fast on spawn/handshake failures (initialize timeout).

- Supervision
  - If the child exits: clean up agent state, cancel pending requests, emit a final log/event upstream.
  - No auto‑restart by default; can be added later.

- Shutdown
  - On orchestrator shutdown: attempt graceful shutdown of agents; kill after grace period if needed.


## IDs, Framing, and Backpressure

- JSON‑RPC IDs
  - Maintain independent ID spaces per agent; store `HashMap<RequestId, oneshot::Sender<_>>` for inflight requests.
  - Do not reuse IDs until response/timeout.

- Framing
  - Use `rmcp` Content‑Length framing for both server and client; do not hand‑roll parsers.

- Backpressure
  - Use bounded channels between read loops and upstream notifier.
  - Never drop approval requests; if needed, apply back‑pressure to non‑critical logs/events.


## Error Handling and Timeouts

- Timeouts
  - Apply reasonable per‑call timeouts (e.g., 60s for conversation calls; 15s for approvals).
  - Allow overrides via environment/config.

- Error mapping
  - Downstream JSON‑RPC errors → MCP tool errors with context (agentId, method, summary of params).
  - Internal errors → actionable logs; avoid leaking secrets/PII.

- Partial/invalid frames
  - `rmcp` handles framing; guard deserialization with clear error logs (log sizes, not raw payloads).

- Cancellation
  - On upstream interrupt: call Codex `interruptConversation`; if still busy, escalate according to policy.


## Configuration and Security

- Environment variables
  - `CODEX_BIN`: full path to Codex binary.
  - `ORCH_AUTO_APPROVE`: optional policy to auto‑approve/deny for dev/testing.
  - `RUST_LOG`: log level (e.g., `info`, `debug`).

- Defaults
  - Prefer `codex`.
  - Deny approvals by default unless configured.

- Security
  - Validate `cwd` exists and is accessible.
  - Never pass secrets in logs or over channels in plaintext.
  - Approval requests must be explicit and strongly typed (paths, commands, reasons).


## Testing Strategy

- Unit tests
  - Serialize/deserialize `codex-protocol` requests/responses to verify JSON shapes match `protocol/src/mcp_protocol.rs`.
  - Agent state machine: spawn → active → exit → cleanup.

- Integration tests (hermetic)
  - Stub Codex MCP server that:
    - Echoes requests.
    - Emits a `codex/event` notification.
    - Issues an approval request and validates the orchestrator forwards and responds correctly.

- Failure cases
  - Child exits mid‑request.
  - Approval timeout.
  - Invalid payloads.
  - Slow upstream causing backpressure.

- CI
  - `cargo test` and `cargo clippy --all-targets --all-features -D warnings`.
  - Format with `cargo fmt --all`.


## Pitfalls and Avoidance

- ID collisions/misrouting → per‑agent ID spaces and pending maps.
- Approval deadlocks → non‑blocking read loops; forward approvals via channels with timeouts; default‑deny on timeout if configured.
- Event loss → bounded channels; never drop approval‑related messages; log drop counts for non‑critical events.
- Partial JSON frames → always rely on `rmcp` framing.
- Orphaned children → process group cleanup on exit; kill on drop with grace period.
- Log poisoning/content‑length injection → never log raw frames; log shapes/sizes only.
- Mixed policies → enforce approval/sandbox policies from upstream turn config; if unset, apply server defaults and surface them in events.


## Phased Implementation Plan

1. Define module interfaces and shared state.
2. Wire `rmcp` MCP server and register tools.
3. Implement Codex client spawn + JSON‑RPC bridge.
4. Bridge notifications and approvals across sides.
5. Add tests and basic logging/metrics.


## Troubleshooting: Orchestrator Tool Calls

Symptoms
- `tool call error: tool call failed for codex-orchestrator/new_conversation` (similar for `send_user_message` and `send_user_turn`).
- Requests appear to succeed for `spawn_agent`, but conversation calls fail immediately.

Root causes and resolutions
- Params passed as strings instead of JSON objects
  - The orchestrator deserializes `params` directly into structs via Serde. If `params` is a JSON string (e.g., `"{...}"`) or empty string, deserialization fails.
  - Always send `params` as a JSON object.

- Minimal local protocol types (placeholders)
  - Current local types are intentionally small (see `src/protocol_types.rs`) and only include fields like `conversationId` for user message/turn operations. They do not carry message content. `new_conversation` returns an empty object.
  - Result: even with correctly shaped JSON, real Codex agents will likely reject conversation calls due to missing fields (e.g., message items, prompt).
  - Options:
    - Extend local types to include required fields (e.g., `items: [{type: "text", text: "..."}]`).
    - Reintroduce a full protocol crate (gated feature) to mirror Codex’s expected shapes.

- Spawned Codex agent exits after initialize
  - If `CODEX_BIN` points to a stub or a binary that only replies to `initialize` and exits, subsequent calls will fail (broken pipe or no response).
  - Verify the binary stays alive after `initialize` and responds to a second request.

Correct call shapes (JSON object params)
- With current local types:
  - `new_conversation`:
    - `{"agentId": "<id>", "params": {}}`
  - `send_user_message`:
    - `{"agentId": "<id>", "params": {"conversationId": "<cid>"}}`
  - `send_user_turn`:
    - `{"agentId": "<id>", "params": {"conversationId": "<cid>"}}`
  - `interrupt`:
    - `{"agentId": "<id>", "params": {"conversationId": "<cid>"}}`

- If using fuller Codex protocol shapes (for reference):
  - `new_conversation` params often include configuration fields, e.g. `{ "model": "o4-mini", "cwd": "/path", "approvalPolicy": "on-request", "sandbox": "workspace-write" }`.
  - `send_user_message`/`send_user_turn` usually include `items` with text/image entries, e.g. `{ "conversationId": "<cid>", "items": [{"type": "text", "text": "..."}] }`.

Binary handshake probe (sanity-check `CODEX_BIN`)
- Ensure `CODEX_BIN` points to a real Codex CLI that supports `mcp` and stays running.
- Newline-delimited JSON probe (expects one initialize response and a follow-up response):
  - Initialize:
    - `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0.1.0"}}}`
  - Follow-up (example):
    - `{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}`
  - Send each as its own line into `"$CODEX_BIN" mcp`; expect two response lines and the process to remain alive.
- If Codex expects Content-Length framing, use `Content-Length` headers around each JSON body.

Logging and diagnostics
- Set `RUST_LOG=info,codex_orchestrator=debug` to capture deserialization errors (invalid params) vs transport errors (child exited/broken pipe).
- The orchestrator forwards downstream notifications as `codex/event` with an opaque JSON payload; approvals are relayed as requests and require a decision within a timeout.

Notes on binary discovery
- If relying on a test/stub, set `CODEX_BIN` explicitly.

Compatibility checklist
- Params: send JSON objects, not strings.
- Protocol fields: ensure required fields (e.g., `items`) are present for conversation ops if using a real Codex agent.
- Binary: `CODEX_BIN` is executable, supports `mcp`, replies to `initialize`, remains alive, and responds to a second request.
- Framing: match Codex’s framing (newline vs Content-Length) if probing manually.
