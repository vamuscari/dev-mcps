# Repository Guidelines

## Project Structure & Module Organization
- Rust binary crate (`codex-lsp`). Entry: `src/main.rs`.
- Framing utilities: `src/framed.rs` (Content-Length based message IO).
- JSON‑RPC types: `src/rpc.rs` (requests, responses, errors, tool structs).
- Add modules under `src/<module>.rs` and register with `mod <module>;` in `src/main.rs`. Keep modules small and purpose‑driven.

## Build and Development Commands
- `cargo build` — compile in debug mode.
- `cargo run` — build and run the JSON‑RPC server (reads framed messages from stdin).
- `cargo fmt` / `cargo fmt -- --check` — format code / verify formatting.
- `cargo clippy --all-targets --all-features -- -D warnings` — lint with warnings as errors.

## Coding Style & Naming Conventions
- Rust 2021 edition; use `rustfmt` defaults (4‑space indent, no trailing whitespace).
- Naming: modules/files `snake_case`; types/enums `CamelCase`; functions/vars `snake_case`; consts `SCREAMING_SNAKE_CASE`.
- Errors: prefer `anyhow::Result` at boundaries; define domain errors with `thiserror`; use `?` over `unwrap/expect` outside tests.
- Serde: derive via `serde`; keep JSON fields stable and explicit with attributes when needed.

## Testing
Tests have been removed from this repository.

## Commit & Pull Request Guidelines
- Use Conventional Commits where possible: `feat:`, `fix:`, `refactor:`, `chore:`, `test:`, `docs:`.
- Subject in imperative mood (≤72 chars); body explains the why and notable trade‑offs.
- Before opening a PR: `cargo fmt` and `cargo clippy -- -D warnings` must pass.
- PRs include a clear summary, rationale, and any protocol/tooling notes; link issues when relevant.

## Architecture Notes
- Minimal JSON‑RPC 2.0 LSP bridge.
- Framing via `Content-Length` headers (`src/framed.rs`).
- RPC data models in `src/rpc.rs`.
- MCP servers build on the [Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).
- Designed and tested against the [Codex](https://github.com/openai/codex) toolchain; other hosts may work but are unverified.
- Extend functionality by: (1) adding to `tools()`; (2) handling in `handle_tools_call`; (3) updating `dispatch` for new methods.
