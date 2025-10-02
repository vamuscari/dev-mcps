# Project Review

## dap

**Overview**
- Purpose: A minimal MCP↔DAP bridge that speaks Content-Length framed JSON over stdio and exposes a curated set of `dap_*` tools for MCP clients to drive a debug adapter.
- Current state: Single-session manager spawns the adapter, sends `initialize`, and synchronously issues DAP requests; MCP server exposes tools and filters some based on reported capabilities. Events are ignored by design.
- Design choices: Blocking DAP I/O behind `tokio::task::spawn_blocking` with a `Mutex` to serialize access (dap/src/mcp.rs:85,100); explicit tool schemas and structured mapping to DAP commands (dap/src/main.rs:81,196); environment-driven adapter command with per-call override (dap/src/da.rs:20–29,62–74).

**Strengths**
- Minimal dependencies and clean module split; follows repo’s “small modules” guidance (dap/Cargo.toml:6–11).
- Robust framing implementation for Content-Length I/O; tolerant of extra headers and case (dap/src/da.rs:32–37,39–60).
- Clear, typed mapping from MCP tools to DAP requests with basic validation (e.g., `threadId`, `frameId`) (dap/src/main.rs:232–336).
- Capability-aware gating of `dap_configuration_done` (dap/src/main.rs:170–176).
- Blocking operations safely offloaded to blocking pool; avoids stalling async runtime (dap/src/mcp.rs:85–106).

**Risks & Gaps**
- Tools listing returns all tools when capabilities are unknown, inadvertently exposing `dap_configuration_done` without confirming support (dap/src/main.rs:145–148 vs. 170–176).
- No read/write timeouts; a misbehaving adapter can hang calls indefinitely (dap/src/da.rs:42–59,152–170).
- Adapter process lifecycle lacks graceful teardown; `dap_disconnect` does not close/kill the child (dap/src/main.rs:298–313; no drop/shutdown in dap/src/da.rs).
- `adapterCommand` allows arbitrary process spawn per-call; no allow-list or validation (dap/src/mcp.rs:24–26; dap/src/da.rs:66–74).
- Override semantics unclear: once started, a different `adapterCommand` is ignored silently (dap/src/da.rs:62–69).
- Initialize uses static `adapterID` = `mcp-dap`, not the actual adapter; may confuse some adapters/telemetry (dap/src/da.rs:93–99).
- Events are entirely ignored; misses critical signals like `initialized`, `output`, and async errors (dap/src/da.rs:120–122; dap/README.md:23).
- Minor polish: dev-dependency duplicates main dep (`serde_json`) and unused tokio feature `sync` (dap/Cargo.toml:11,13–14).

**Recommendations (prioritized)**
- Fix tool gating default: when capabilities are `None`, conservatively exclude `dap_configuration_done` (dap/src/main.rs:145–148); only add it when explicitly supported (170–176).
- Add per-call timeout around blocking tasks using `tokio::time::timeout` at the call sites to bound hangs (dap/src/mcp.rs:85–106,100–106).
- Implement graceful shutdown: on `dap_disconnect`, wait for response and terminate the child; also add `Drop` for `DapAdapterManager` to kill the process (dap/src/da.rs:134–171).
- Enforce an adapter allow-list via env (e.g., `DAP_ALLOWED_CMDS`) and reject unknown `adapterCommand` to reduce spawn risk (dap/src/mcp.rs:24–26).
- Define behavior when a new `adapterCommand` is provided after start: either error clearly or restart adapter; document it (dap/src/da.rs:62–74).
- Surface events: run a background reader to queue events and add a `dap_events` tool, or minimally capture `initialized` to guide `configurationDone` ordering (dap/src/da.rs:120–122).
- Make schemas stricter: set `dap_call.arguments` to `object`; tighten `launch/attach.arguments` to `object` (dap/src/main.rs:21–39).
- Polish initialize payload: set `adapterID` to actual adapter (when known) and add `clientName` (dap/src/da.rs:93–99).
- Trim dependencies: drop `tokio`’s `sync` feature and the redundant `serde_json` dev-dep (dap/Cargo.toml:11,13–14).

**Tests to add**
- Framing round-trip: multiple headers and mixed-case `Content-Length` (dap/src/da.rs:39–60).
- Capability gating: ensure `dap_configuration_done` absent when capabilities `None`, present when `supportsConfigurationDoneRequest=true` (dap/src/main.rs:145–176).
- Breakpoints mapping: `lines` → `breakpoints` object conversion (dap/src/main.rs:215–225).
- Required params: errors for missing `threadId`, `frameId`, `expression` (dap/src/main.rs:232–336).
- Timeout behavior: simulate stalled adapter and assert `timeout` error mapping (dap/src/mcp.rs:85–106).
- Adapter override semantics: assert error or restart when `adapterCommand` changes (dap/src/da.rs:62–74).
- Disconnect lifecycle: verify child termination on `dap_disconnect` (dap/src/main.rs:298–313).

## lsp

**Overview**
- Purpose: MCP server that bridges Codex tools to LSP servers over JSON-RPC, spawning language servers and forwarding LSP requests/notifications with minimal framing and lifecycle handling.
- Current state: Feature-complete first pass covering most common LSP methods, capability-filtered tool listing, and a pragmatic LSP client with Content-Length framing and auto/newline detection. Server selection via env defaults and per-language/extension maps. No tests.
- Notable choices: Centralized pool with per-command LSP managers and a global mutex; auto didOpen for ad-hoc requests with file inlining up to 2 MiB; capability-driven tool exposure; flexible server mapping via `LSP_SERVER_MAP`.

**Strengths**
- Capability-driven tools: Filters exposed tools based on server capabilities, including 3.18 proposals (diagnostics, file ops) when advertised (lsp/src/mcp.rs:34, 219).
- Solid LSP lifecycle: Initialize → initialized → graceful shutdown/exit with fallback kill (lsp/src/ls.rs:540, 592, 459).
- Framing robustness: Content-Length by default with auto/newline detection and env override (lsp/src/ls.rs:24, 45).
- Sensible server resolution: Explicit per-call override, doc association, language/extension maps, and env fallback (lsp/src/main.rs:994, 1044, 1072, 1133).
- Clear tool schemas and uniform “lsp_*” surface (lsp/src/main.rs:1223).
- Practical error mapping: Parameter validation and uniform tool error code with structured data (lsp/src/main.rs:65, 69, 1992).

**Risks & Gaps**
- Concurrency bottleneck: A single global mutex serializes all LSP traffic across servers (lsp/src/main.rs:1217). This limits parallelism and can stall multi-language use.
- Ephemeral didOpen leak: Auto-opened docs aren’t auto-closed, potentially accumulating open docs (lsp/src/main.rs:2088, 2103; 1044).
- Position encoding: Client advertises `utf-16` but no adaptation to server-selected encoding in responses/requests (lsp/src/ls.rs:67, 98).
- Security: `serverCommand`/`LSP_SERVER_MAP` executes arbitrary commands; no allow-list or validation (lsp/src/main.rs:994, 1023; lsp/src/ls.rs:508).
- Cancel/progress: No `$/cancelRequest` or progress token support; long-running requests can’t be canceled (lsp/src/ls.rs:664).
- Docs drift: lsp/AGENTS.md mentions modules not present (`framed.rs`, `rpc.rs`), creating confusion.
- Tests absent: `lsp/tests` is empty; integration/unit coverage is needed.

**Recommendations (prioritized)**
- Split pool locking by server: Replace one global mutex with per-command manager locks to enable parallel LSP requests (lsp/src/main.rs:1217).
- Auto-close ephemeral documents: If `lsp_call` opened a doc, send didClose after request or implement TTL-based cleanup (lsp/src/main.rs:2088, 2103).
- Harden command execution: Add allow-list and/or restrict `serverCommand` to known binaries; validate `LSP_SERVER_MAP` keys and values (lsp/src/main.rs:994, 1023; lsp/src/ls.rs:508).
- Respect position encoding: Read server `capabilities.positionEncoding` and adapt conversions; document expectations (lsp/src/ls.rs:67).
- Implement cancel: Track outstanding IDs and support `$/cancelRequest`, propagate to managers (lsp/src/ls.rs:664).
- Align docs vs code: Update lsp/AGENTS.md to reflect current modules or extract framing/RPC structs into small files as documented.
- Bound log verbosity: Gate `eprintln!` with env flag; include server label consistently (lsp/src/ls.rs:679; lsp/src/main.rs:2127).
- Expand capability gates: Ensure all 3.18 features are behind explicit checks already present (audit existing checks; lsp/src/mcp.rs:66–223).
- Improve URI normalization tests and Windows paths (lsp/src/main.rs:1133).
- Add health command: MCP tool for “capabilities + server state” to aid debugging.

**Tests to add**
- URI normalization and extension detection (lsp/src/main.rs:1079, 1133).
- Server map overrides parsing (`LSP_SERVER_MAP`) including quoted commands (lsp/src/main.rs:925, 939).
- Command parser with quotes/escapes (lsp/src/ls.rs:116, 142).
- didOpen builder: size cap, missing file errors, language inference (lsp/src/main.rs:1167, 1172, 1193).
- Framing detection: newline vs Content-Length parsing (lsp/src/ls.rs:24, 400).
- Initialize/initialized handshake and capability capture (lsp/src/ls.rs:540, 592).
- Tool invocation builders for diagnostics/workspace symbol resolve (lsp/src/main.rs:527, 1568, 1572).
- Capability-based tool filtering (lsp/src/mcp.rs:34).

## lsif

**Overview**
- Purpose: Minimal MCP server exposing LSIF-backed code intelligence over stdio framing, with tools for load/definition/references/hover (stubbed) per README (`lsif/README.md:5`–`lsif/README.md:10`). Server wiring via rmcp `ServerHandler` with tools listed and dispatched (`lsif/src/main.rs:23`, `lsif/src/main.rs:60`, `lsif/src/main.rs:147`).
- State: Loads LSIF JSONL into an in-memory index, resolves definitions/references, returns locations; hover is unimplemented though tool is exposed (`lsif/src/lsif.rs:395`–`lsif/src/lsif.rs:397`).
- Design: Single global index protected by `OnceLock<Mutex<LSIFIndex>>` (`lsif/src/lsif.rs:298`), reset on load (`lsif/src/lsif.rs:311`). Range selection picks smallest containing span and prefers resultSet edges when present (`lsif/src/lsif.rs:227`, `lsif/src/lsif.rs:352`–`lsif/src/lsif.rs:361`).

**Strengths**
- Clean MCP integration and clear tool schemas (`lsif/src/main.rs:60`–`lsif/src/main.rs:121`).
- LSIF ingestion covers key vertices/edges: document, range, resultSet, contains, next, definition/references, and item split (`lsif/src/lsif.rs:75`–`lsif/src/lsif.rs:221`).
- Pragmatic fallback in definition to use references when definitions absent (`lsif/src/lsif.rs:356`–`lsif/src/lsif.rs:364`).
- Simple, readable data model for spans and location JSON (`lsif/src/lsif.rs:14`, `lsif/src/lsif.rs:336`).

**Risks & Gaps**
- Concurrency: `Mutex` serializes all reads; a long `lsif_load` blocks all calls; read-heavy workload would benefit from `RwLock` (`lsif/src/lsif.rs:298`, `lsif/src/lsif.rs:309`–`lsif/src/lsif.rs:334`).
- Performance: `find_best_range` scans all ranges in a document (O(n)); may be slow on large indexes (`lsif/src/lsif.rs:227`–`lsif/src/lsif.rs:249`).
- LSIF “next” handling only maps range→resultSet; does not follow resultSet→resultSet chains (common in LSIF), risking missed results (`lsif/src/lsif.rs:160`–`lsif/src/lsif.rs:167`, `lsif/src/lsif.rs:252`–`lsif/src/lsif.rs:257`).
- Hover: tool exposed but unimplemented; edges not wired and results unused (`lsif/src/lsif.rs:192`–`lsif/src/lsif.rs:195`, `lsif/src/lsif.rs:395`–`lsif/src/lsif.rs:397`, `lsif/src/lsif.rs:46`).
- URI handling: exact-string match on URIs; no normalization for `file://` vs paths, case, or platform specifics (`lsif/src/lsif.rs:32`–`lsif/src/lsif.rs:34`, `lsif/src/lsif.rs:227`–`lsif/src/lsif.rs:229`).
- Error handling: loader silently skips malformed JSON lines; no counters/diagnostics returned to the caller (`lsif/src/lsif.rs:319`–`lsif/src/lsif.rs:323`).
- Security: `lsif_load` opens arbitrary paths without validation or size limits (`lsif/src/lsif.rs:312`).

**Recommendations (Prioritized)**
- Replace `Mutex` with `RwLock` for the global index to permit concurrent reads; write-lock for loads (`lsif/src/lsif.rs:298`). Rationale: improves throughput under typical read-heavy usage.
- Implement resultSet chain resolution: track `result_set_next: HashMap<i64, i64>` and chase it when resolving def/ref results (`lsif/src/lsif.rs:160`, `lsif/src/lsif.rs:252`). Rationale: correctness on LSIF graphs with chained resultSets.
- Add basic hover wiring: map `textDocument/hover` edges to `rset_to_hover`/`range_to_hover`, return stored `hover_results` (`lsif/src/lsif.rs:192`, `lsif/src/lsif.rs:46`, `lsif/src/lsif.rs:395`). Rationale: fulfill exposed API.
- Add per-document index to accelerate range lookup (e.g., `BTreeMap<(line, char), Vec<(Span, rid)>>` or bucket by line). Rationale: avoid O(n) scans (`lsif/src/lsif.rs:227`).
- Normalize URIs: accept file paths and `file://` URIs; canonicalize before lookup (`lsif/src/lsif.rs:227`). Rationale: cross-environment robustness.
- Return loader diagnostics: count vertices/edges/lines, errors, and expose in `lsif_load` response (`lsif/src/main.rs:151`–`lsif/src/main.rs:158`, `lsif/src/lsif.rs:319`). Rationale: observability.
- Validate/limit file size or offer streaming chunking; optionally deny non-local paths by policy. Rationale: safety (`lsif/src/lsif.rs:312`).
- Document definition fallback behavior in README and `server_info` (`lsif/src/lsif.rs:356`–`lsif/src/lsif.rs:364`, `lsif/src/main.rs:42`). Rationale: surprising semantics.

**Tests To Add**
- Load + definition path on a tiny LSIF fixture (one symbol with def and refs).
- References with and without `includeDeclarations` toggled.
- Range selection: nested ranges pick smallest containing span.
- ResultSet chain traversal test (range→resultSet→resultSet→result).
- Hover wiring end-to-end (after implementation).
- URI normalization: path vs `file://` refer to same document.

## orchestrator

**Overview**
- Purpose: MCP server that spawns and manages Codex agent processes, proxies conversation APIs, and forwards events back to the MCP client. Separation between MCP surface (`mcp`) and agent lifecycle/RPC (`codex::Manager`) is clear.
- Current state: End-to-end flow works with a robust stub agent and solid coverage for conversations, approvals, and notifications. StdIO JSON-RPC handshake and per‑agent read loops are implemented. Event forwarding uses MCP logging notifications.
- Notable choices: Global upstream peer via `OnceCell` for notifications (`orchestrator/src/mcp.rs:17`), flexible param normalization from strings (`orchestrator/src/mcp.rs:43`), auto‑defaults for `sendUserTurn` (`orchestrator/src/codex.rs:170`), and approval routing with a pending map and timeout (`orchestrator/src/codex.rs:440`).

**Strengths**
- Clear layering and small modules; minimal deps aligned with repo guidelines.
- Concurrency hygiene: per‑agent reader/writer locks, `oneshot` for RPC matching, RwLock for agent map (`orchestrator/src/codex.rs:27`).
- Robust test suite with a stub Codex binary; covers pagination, lifecycle, approvals, and multi‑conversation scenarios.
- Initialization handles notifications before the read loop starts (`orchestrator/src/codex.rs:357`) and sends `notifications/initialized` (`orchestrator/src/codex.rs:372`).
- Event forwarding standardized via MCP logging with a `codex/event` logger (`orchestrator/src/mcp.rs:360`).

**Risks & Gaps**
- Inconsistent tool results: some return textified JSON vs structured results (e.g., `spawn_agent`, `list_agents` use text; `new_conversation` uses structured) leading to client parsing ambiguity (`orchestrator/src/mcp.rs:172`, `orchestrator/src/mcp.rs:183`).
- Global `UPSTREAM_PEER` makes only a single client practical; re‑init or multi‑client scenarios could be brittle (`orchestrator/src/mcp.rs:17`, `orchestrator/src/main.rs:34`).
- `kill_agent` uses `try_lock` and may fail to kill under contention, leaving zombies (`orchestrator/src/codex.rs:120`).
- Approval timeout and `sendUserTurn` defaults are hard‑coded; not configurable (`orchestrator/src/codex.rs:176`, `orchestrator/src/codex.rs:440`).
- Potential request param mismatch: `rpc_call` wraps non‑object params as `{value: ...}` which some Codex methods may not accept (`orchestrator/src/codex.rs:483`).
- `protocol_types.rs` is unused scaffolding; can drift and confuse (`orchestrator/src/protocol_types.rs:1`).
- Minor: test timeout error message mismatches 180s default (`orchestrator/tests/util.rs:16`).

**Resolved Regression**
- P0: Pending RPCs hang when agent exits. Killing or crashing an agent caused concurrent tool calls to wait forever because the read loop exited without draining `agent.pending`. Root cause: on EOF or transport error, the loop `break`-ed without resolving or dropping the oneshot senders, so `rpc_call` receivers never completed. Fix: when the stream ends or a read error occurs, drain `pending` and send an error to each waiter so callers complete promptly. See `orchestrator/src/codex.rs:387` (EOF) and `orchestrator/src/codex.rs:504` (read error).

**Recommendations (Prioritized)**
- Unify tool outputs to structured results for all tools; remove ad‑hoc text JSON (`spawn_agent`, `list_agents`, `kill_agent`) to match others (reduces client complexity). See `orchestrator/src/mcp.rs:172`, `orchestrator/src/mcp.rs:183`, `orchestrator/src/mcp.rs:199`.
- Make approval timeout and `sendUserTurn` defaults configurable via env (e.g., `ORCH_APPROVAL_TIMEOUT`, `ORCH_DEFAULT_MODEL`, `ORCH_SANDBOX_POLICY`) and document in `README.md` (improves portability). Defaults currently at `orchestrator/src/codex.rs:170` and `orchestrator/src/codex.rs:440`.
- Replace `try_lock()` in `kill_agent` with `lock().await` and handle process exit/cleanup robustly (`orchestrator/src/codex.rs:120`).
- Implement upstream approval request helpers or remove dead APIs (`request_apply_patch_approval`, `request_exec_command_approval`) to avoid confusion (`orchestrator/src/mcp.rs:373`, `orchestrator/src/mcp.rs:381`).
- Add a Manager drop/shutdown path to terminate all child agents gracefully on server shutdown (prevents leaks).
- Ensure `rpc_call` param shaping matches Codex methods; avoid `{value: ...}` unless the method expects it; rely on `normalize_params` upstream (`orchestrator/src/codex.rs:483`, `orchestrator/src/mcp.rs:43`).
- Consider scoping `UPSTREAM_PEER` to the service or session; at minimum document single‑client support and error on reassignment (`orchestrator/src/mcp.rs:19`).
- Remove or integrate `protocol_types.rs` and keep types aligned with actual wire shapes to prevent type drift (`orchestrator/src/protocol_types.rs:1`).
- Add coarse metrics/logs including agent id on key paths for operability.

**Tests To Add (Follow‑ups)**
- Regression: killing an agent with an in‑flight forwarded call completes with an error (restores prior `kill_agent_completes_with_pending_forward` behavior).

**Tests To Add**
- Structured tool result shape for all tools (no textified JSON).
- `kill_agent` under send/approval load; verify proper shutdown.
- Config overrides via env for model/sandbox/timeout are honored.
- Error propagation: non‑object `params` for each tool, invalid binary path (`CODEX_BIN`) failure path.
- Multi‑client/event storms: ensure correct upstream notification behavior and no panics on missing peer.
