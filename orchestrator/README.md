# codex-orchestrator

MCP server that manages Codex agent processes. It exposes tools for spawning,
listing, and terminating agents while proxying conversation operations back to
each agent via simple JSON messages.

## Usage Flow
- Spawn an agent with `spawn_agent` → receive `agentId`.
- Start a conversation with `new_conversation { agentId, params }`.
- Send messages with `send_user_message` or `send_user_turn`.
- Optionally `interrupt` an in-flight conversation.
- Use `list_agents` to inspect and `kill_agent` to terminate.

Notes
- `params` mirrors Codex CLI tool inputs: for `new_conversation` include
  `prompt` (string). For `send_user_message` include `conversationId` (string)
  and `message` (string). As a convenience, `new_conversation` also accepts
  `topic` or `message` as an alias for `prompt`, and `send_user_message` accepts
  `prompt` as an alias for `message`.
- **IMPORTANT**: `new_conversation` only creates conversation metadata. The Codex agent
  does NOT process the initial prompt automatically. You MUST call `send_user_turn`
  or `send_user_message` after `new_conversation` to trigger agent processing and get responses.
- **Agent Responses & Events**: Codex agent responses and events are sent as MCP notifications
  (`notifications/message`) with logger `codex/event`. These notifications include:
  - Agent text responses
  - Tool calls and results
  - Approval requests (`kind: "approval_request"`)
  - Other agent events

  **Note**: MCP clients may not display these notifications by default. To see agent responses,
  you need to either:
  1. Configure your MCP client to display/log notifications with logger `codex/event`
  2. Poll the conversation rollout files directly (see `list_conversations` for paths)
  3. Implement a custom notification handler in your client
- Set `CODEX_BIN` to override the agent binary; defaults to `codex` on `PATH`.

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
- `send_user_turn`
  - Description: Forwarded to the agent as `sendUserTurn`. Auto-fills required fields with sensible defaults.
  - Args: `{ agentId: string, params?: object | string }`
  - Required in params: `conversationId` (or inferred from last conversation), `text` or `items`
  - Auto-filled if missing: `cwd` (current dir), `approvalPolicy` ("never"), `sandboxPolicy` (read-only), `model` ("gpt-4"), `summary` ("auto")
- `interrupt`
  - Description: Forwarded as `interruptConversation` (if supported by the agent).
  - Args: `{ agentId: string, params?: object }`
- `list_conversations`
  - Description: List recorded Codex conversations (rollouts) with optional pagination.
  - Args: `{ agentId: string, params?: { pageSize?: number, cursor?: string } }`
  - Result: `{ items: [{ conversationId, path, preview, timestamp }], nextCursor?: string }`
- `resume_conversation`
  - Description: Resume a recorded Codex conversation from a rollout file.
  - Args: `{ agentId: string, params: { path: string, overrides?: object } }`
  - Result: `{ conversationId, model, initialMessages?: [...] }`
- `archive_conversation`
  - Description: Archive (mark as finished) a Codex conversation.
  - Args: `{ agentId: string, params: { conversationId: string } }`
  - Result: `{ ok: true }`
- `get_conversation_events`
  - Description: Read events from a conversation rollout file (useful when notifications aren't visible).
  - Args: `{ rolloutPath: string, limit?: number }`
  - Result: `{ events: [...], count: number }`

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
- List conversations
  - Args: `{ "agentId": "dev-agent", "params": { "pageSize": 10 } }`
  - Result: `{ "items": [{ "conversationId": "c1", "path": "/path/to/rollout.jsonl", "preview": "Review dap...", "timestamp": "2024-01-01T12:00:00Z" }] }`
- Resume a conversation
  - Args: `{ "agentId": "dev-agent", "params": { "path": "/path/to/rollout.jsonl" } }`
  - Result: `{ "conversationId": "c1", "model": "gpt-4" }`
- Archive a conversation
  - Args: `{ "agentId": "dev-agent", "params": { "conversationId": "c1" } }`
  - Result: `{ "ok": true }`
- Get conversation events (poll for agent responses)
  - Args: `{ "rolloutPath": "/Users/user/.codex/sessions/2024/01/01/rollout-c1.jsonl", "limit": 20 }`
  - Result: `{ "events": [...], "count": 20 }`

## Configuration
- `CODEX_BIN` — Override the command used to spawn agents. Defaults to `codex` when available on `PATH`.

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
