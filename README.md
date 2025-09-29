# codex-mcps

Workspace of Rust Model Context Protocol (MCP) servers and utilities. Each crate
implements a stdin/stdout JSON-RPC service that bridges MCP to another protocol or
coordinates Codex agents.
They target the [Codex](https://github.com/openai/codex) toolchain and are only tested there, though they may work with other MCP hosts.

All servers build on the [Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
for shared MCP transport and data types.

## Servers
- [codex-lsp](lsp/README.md) — MCP ↔ Language Server Protocol bridge with tool wrappers for common LSP features.
- [codex-dap](dap/README.md) — MCP ↔ Debug Adapter Protocol bridge for debugger tooling.
- [codex-lsif](lsif/README.md) — LSIF-backed MCP tools for offline code intelligence queries.
- [codex-orchestrator](orchestrator/README.md) — Agent supervisor that spawns Codex MCP agents and proxies conversation tools.

Shared utilities live in [`common/`](common/) and support Content-Length framed I/O, transport helpers, and test fixtures.

## Getting Started
- Prerequisites: Rust 1.75+; optional Node.js for `@modelcontextprotocol/inspector`.
- Build: `cargo build --workspace`
- Run a server: `cargo run -p codex-lsp` (or `codex-dap`, `codex-lsif`, `codex-orchestrator`).
- Test: `cargo test --workspace`
- Lint/format: `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`

## Contributing
Follow the workflow, testing, and style guidance in [`AGENTS.md`](AGENTS.md).

## License
MIT — see [`LICENSE`](LICENSE).
