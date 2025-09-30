# codex-orchestrator

MCP server that manages Codex agent processes. It exposes tools for spawning,
listing, and terminating agents while proxying conversation operations back to
each agent via simple JSON messages.

## Usage Flow
- Spawn an agent with `spawn_agent` → receive `agentId`.
- Start a conversation with `new_conversation { agentId, params }`.
- Send messages with `send_user_message { agentId, params }`.
- Optionally `interrupt` an in-flight conversation.
- Use `list_agents` to inspect and `kill_agent` to terminate.

Notes
- `params` mirrors Codex CLI tool inputs: for `new_conversation` include
  `prompt` (string). For `send_user_message` include `conversationId` (string)
  and `message` (string). As a convenience, `new_conversation` also accepts
  `topic` or `message` as an alias for `prompt`, and `send_user_message` accepts
  `prompt` as an alias for `message`.
- Codex events (including approvals) are relayed as rmcp logging notifications
  `notifications/message` with logger `codex/event`.
- Set `CODEX_BIN` to override the agent binary; defaults to `codex` or
  `codex-test` if found on `PATH`.

## Tools
- `spawn_agent`
  - Description: Start an MCP-capable Codex agent process. Returns `{ agentId }`.
  - Args: `{ id?: string, cwd?: string }`
- `list_agents`
  - Description: List identifiers of running agents started by the orchestrator.
  - Args: `{}`
- `kill_agent`
  - Description: Terminate a managed agent.
  - Args: `{ agentId: string }`
- `new_conversation`
  - Description: Forwarded to the agent as `newConversation`.
  - Args: `{ agentId: string, params?: object }`
- `send_user_message`
  - Description: Forwarded to the agent as `sendUserMessage`.
  - Args: `{ agentId: string, params?: object }`
- `interrupt`
  - Description: Forwarded as `interruptConversation` (if supported by the agent).
  - Args: `{ agentId: string, params?: object }`

### Approvals
- Overview
  - When Codex requests approval (e.g., applyPatchApproval, execCommandApproval),
    the orchestrator emits a logging notification (`notifications/message`) with
    logger `codex/event` and a payload:
    `{ kind: "approval_request", agentId, requestId, method, params }`.
  - Pending approvals are addressable via a composite key: `"<agentId>:<requestId>"`.
  - Decisions default to `deny` after 60 seconds if not provided.

- `list_pending_approvals`
  - Description: List approval keys currently waiting on a decision.
  - Args: `{}`
  - Result: `{ "keys": ["agent-1:42", "agent-2:7", ...] }`

- `decide_approval`
  - Description: Resolve a pending approval with a decision.
  - Args: `{ "key": "agent-1:42", "decision": "allow" | "deny" }`
  - Result: `{ "ok": true }`

## Examples
- Spawn an agent
  - Args: `{ "id": "dev-agent", "cwd": "/path/to/project" }`
  - Result: `{ "agentId": "dev-agent" }`
- Create a conversation
  - Args: `{ "agentId": "dev-agent", "params": { "prompt": "Review dap directory" } }`
- Send a message
  - Args: `{ "agentId": "dev-agent", "params": { "conversationId": "c1", "message": "hello" } }`
- Interrupt a conversation
  - Args: `{ "agentId": "dev-agent", "params": { "conversationId": "c1" } }`

## Configuration
- `CODEX_BIN` — Override the command used to spawn agents. Defaults to `codex` or
  `codex-test` when available on `PATH`.

## Build, Run, Test
- Build: `cargo build -p codex-orchestrator`
- Run: `cargo run -p codex-orchestrator`
- Test: `cargo test -p codex-orchestrator`

The server communicates via line-delimited JSON (rmcp codec) over stdin/stdout
and implements the MCP initialize → tools → shutdown lifecycle.

## Protocol Types
- The orchestrator uses lightweight, local Rust types for Codex MCP
  requests/responses (`src/protocol_types.rs`).
- No TypeScript bindings are generated; the `ts-rs` dependency has been removed.
