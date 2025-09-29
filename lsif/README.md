# codex-lsif

Minimal MCP/JSON-RPC server dedicated to LSIF (Language Server Index Format). It exposes a small set of LSIF-backed tools over stdin/stdout framing.

- Tools:
  - `lsif_load` — `{ "path": "/path/to/index.lsif" }` JSONL loader
  - `lsif_definition` — `{ "uri", "position": { "line", "character" } }`
  - `lsif_references` — previous + `includeDeclarations?: boolean`
  - `lsif_hover` — placeholder; returns error in minimal ingester

- Protocol:
  - `initialize` → returns `{ protocolVersion, serverInfo, capabilities.tools }`
  - `tools/list` → lists tools and input schemas
  - `tools/call` → invokes LSIF tools (see above)
  - `shutdown` → returns null; `exit` notification expected by host

Build, run, test:
- Build: `cargo build -p codex-lsif`
- Run: `cargo run -p codex-lsif`
- Test: `cargo test -p codex-lsif`

Communication uses MCP-standard Content-Length framing over stdin/stdout.
