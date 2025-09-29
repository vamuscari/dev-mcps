# codex-dap

Minimal MCP↔DAP (Debug Adapter Protocol) bridge. The binary reads/writes framed JSON on stdin/stdout using `Content-Length` headers and exposes a small set of `dap_*` MCP tools for driving a debug adapter.

## Configure
- Set `DAP_ADAPTER_CMD` to the debug adapter command (e.g., `debugpy-adapter`, `js-debug-adapter`, `lldb-vscode`).
- Tools also accept `adapterCommand` to override per call.

## Tools (subset)
- Core: `dap_initialize`, `dap_call`.
- Session: `dap_launch`, `dap_attach`, `dap_configuration_done`, `dap_disconnect`.
- Control: `dap_continue`, `dap_next`, `dap_step_in`, `dap_step_out`.
- Introspection: `dap_threads`, `dap_stack_trace`, `dap_scopes`, `dap_variables`, `dap_evaluate`.
- Breakpoints: `dap_set_breakpoints` (`source.path` + `breakpoints` or `lines`).

`tools/list` probes adapter capabilities (via `initialize`) and filters a few gated tools (e.g., `dap_configuration_done`).

## Build, Run, Test
- Build: `cargo build -p codex-dap`
- Run: `cargo run -p codex-dap`
- Test: `cargo test -p codex-dap`

This is a minimal, request/response bridge. Adapter events are currently ignored in responses; future work may surface them as notifications.
