# Repository Guidelines

## Project Structure & Module Organization
- `lsp/` — Rust MCP↔LSP bridge (stubbed LSP calls). Binary crate `codex-lsp`.
- `lsif/` — 
- `dap/` — 
- `orchestrator/` — Rust MCP orchestrator. Binary crate `codex-orchestrator`.
- `Cargo.toml` (workspace) at repo root.

## Build, Test, and Development Commands
- Build (workspace): `cargo build --release`
- Run (stdio MCP):
  - LSP bridge: `cargo run -p codex-lsp`
  - Orchestrator: `cargo run -p codex-orchestrator`
- Test: `cargo test` (workspace)

## Coding Style & Naming Conventions
- Language: Rust 2021; keep dependencies minimal.
- Formatting: `cargo fmt --all`; lint with `cargo clippy --all-targets --all-features -D warnings`.
- Crate/module names: lowercase with underscores; types use `CamelCase`; functions `snake_case`.
- Prefer small modules (`framed.rs`, `rpc.rs`) and explicit error types where helpful.

## Testing Guidelines
- Use `cargo test`; keep tests hermetic and fast.
- Favor table‑style tests with clear inputs/outputs.
- Add integration tests for framed I/O and tool dispatch as features mature.

## Commit & Pull Request Guidelines
- Commits: clear, present tense (e.g., "feat(lsp-rs): add tools/list"). Conventional Commits welcome.
- PRs: concise description, linked issues, repro steps, and relevant env vars (`CODEX_BIN`, planned `ORCH_AUTO_APPROVE`). Include inspector command lines if useful.
- Keep changes minimal and crate‑scoped; update docs/tests when tool schemas or behavior change.

## Security & Configuration Tips
- Do not commit secrets. Use environment variables for configuration.
- Orchestrator spawns external binaries; validate inputs and prefer allow‑lists.
- For future LSP support, ensure target language servers are on `PATH`.

## Architecture Notes
- MCP servers build on the [Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).
- Designed and tested against the [Codex](https://github.com/openai/codex) toolchain; other hosts may work but are unverified.
