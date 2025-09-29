# codex-orchestrator

MCP server that manages Codex agent processes. It exposes tools for spawning, listing,
and terminating agents while proxying conversation-related operations back to each
agent over JSON messages.

## Tools
- `spawn_agent` — Start an MCP-capable Codex agent process. Accepts optional `id` and `cwd`.
- `list_agents` — Return the identifiers of all running agents started by the orchestrator.
- `kill_agent` — Terminate a managed agent by `agentId`.
- `new_conversation` — Forward a conversation creation request to the selected agent.
- `send_user_message` — Forward a user message payload to the selected agent.
- `interrupt` — Forward an interrupt instruction to the selected agent.

The forwarded tools expect an `agentId` (from `spawn_agent`) and optional `params`
object that is passed through to the child process untouched. Agent stdout lines are
relayed as `window/logMessage` notifications with logger `codex/event`.

## Configuration
- `CODEX_BIN` — Override the command used to spawn agents. Defaults to `codex` or
  `codex-test` when available on `PATH`.

## Build, Run, Test
- Build: `cargo build -p codex-orchestrator`
- Run: `cargo run -p codex-orchestrator`
- Test: `cargo test -p codex-orchestrator`

The server communicates via Content-Length framed JSON over stdin/stdout and
implements the MCP initialize → tools → shutdown lifecycle.
