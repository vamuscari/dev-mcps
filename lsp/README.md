# mcp-lsp

Minimal JSON‑RPC 2.0 bridge with a small, spec‑aligned LSP surface to enable editor/tool integration. The binary reads and writes framed JSON messages on stdin/stdout and currently focuses on correct LSP lifecycle and text document synchronization.

All MCP server projects in this repo build on the [Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).
They target the [Codex](https://github.com/openai/codex) toolchain and are only tested there, though they may work with other MCP hosts.

## LSP Specification Notes

- Framing
- Uses `Content-Length` headers with `\r\n\r\n` separator and UTF‑8 JSON bodies (per LSP Base Protocol).
- Lifecycle and Ordering
  - Client sends `initialize`; server must not send requests/notifications before responding (except `window/*` messages and `$/progress` bound to a provided token). After `InitializeResult`, client sends `initialized`.
  - `shutdown` request returns `null`; after that only `exit` is accepted. Server exits with code 0 if `shutdown` was received, else 1.
- Capabilities and Encodings
  - Client may advertise `general.positionEncodings` (e.g., `utf-16`, `utf-8`, `utf-32`). Server selects one and returns it via `capabilities.positionEncoding` (defaults to `utf-16`).
- Text Document Sync
  - Client support for `textDocument/didOpen|didChange|didClose` is mandatory; server should implement all or none. This project currently advertises Full sync (1) and handles those notifications.
- Diagnostics
  - Server owns diagnostics; pushes via `textDocument/publishDiagnostics`. New pushes replace prior state; push an empty array to clear. This project scaffolds push diagnostics and clears on open/change/close.
- Cancellation and Progress
  - Cancelling requests uses `$/cancelRequest` and should result in an error with code `RequestCancelled`. Generic progress (including partial results) is reported via `$/progress` tokens.
- Error Codes (selection)
  - `ServerNotInitialized` (-32002) before initialize, `ContentModified` (-32801) to invalidate work, `RequestCancelled` (-32800) on cancel.

## Spec References

- Current (3.17): https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
- Upcoming (3.18, under development): https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/

## LSIF Support

LSIF functionality has moved into a dedicated MCP server: `mcp-lsif` (see `lsif/`). Use that server for LSIF tools such as `lsif_load`, `lsif_definition`, and `lsif_references`.

### What’s notable in 3.18

- SnippetTextEdit in workspace edits
  - Server can return snippet edits in `WorkspaceEdit` (guarded by `workspace.workspaceEdit.snippetEditSupport`).
- Markup in Diagnostic.message
  - `Diagnostic.message` may be `MarkupContent` (guarded by `textDocument.diagnostic.markupMessageSupport`).
- Text Document Content Provider
  - `workspace/textDocumentContent` request and related capabilities/options to fetch readonly content for schemes.

This repository targets correctness against 3.17 while tracking 3.18 additions behind client capability gates.

## Project Status and Behavior

- Implements:
  - Initialize → Initialized → Shutdown → Exit, with correct ordering and exit codes.
  - Text sync (Full) for `didOpen`/`didChange`/`didClose` and push diagnostics (currently empty scaffold).
- Out of scope (yet):
  - Incremental sync, pull diagnostics, progress streaming, capability dynamic registration.
  - LSIF indexer/ingestion (future: add an optional LSIF importer to answer read‑only queries offline).

## Build & Run

- Build: `cargo build`
- Run: `cargo run`
- Formatting / Lint: `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`

The server reads framed JSON from stdin and writes framed responses/notifications to stdout.

### Tools and LSIF usage

- List available tools:
  - Send a JSON‑RPC request with `method` = `tools/list`.
- Call a tool:
  - Use `method` = `tools/call` with params `{ "name": <tool_name>, "arguments": { ... } }`.
- LSP tools (uniform names; filtered by server capabilities on `tools/list` if `LSP_SERVER_CMD` is set):
  - Core position/document: `lsp_hover`, `lsp_declaration`, `lsp_definition`, `lsp_type_definition`, `lsp_implementation`, `lsp_references`, `lsp_completion`, `lsp_signature_help`, `lsp_document_highlight`, `lsp_document_symbol`.
  - Formatting and edits: `lsp_formatting`, `lsp_range_formatting`, `lsp_on_type_formatting`, `lsp_prepare_rename`, `lsp_rename`, `lsp_code_action`.
  - Navigation and structure: `lsp_folding_range`, `lsp_selection_range`, `lsp_linked_editing_range`, `lsp_moniker`.
  - Hierarchies: `lsp_call_hierarchy_prepare`, `lsp_call_hierarchy_incoming_calls`, `lsp_call_hierarchy_outgoing_calls`, `lsp_type_hierarchy_prepare`, `lsp_type_hierarchy_supertypes`, `lsp_type_hierarchy_subtypes`.
  - Semantic tokens: `lsp_semantic_tokens_full`, `lsp_semantic_tokens_full_delta`, `lsp_semantic_tokens_range`.
  - Color: `lsp_document_color`, `lsp_color_presentation`.
  - Hints/values: `lsp_inlay_hint`, `lsp_inlay_hint_resolve`, `lsp_inline_value`.
  - Workspace: `lsp_workspace_symbol`, `lsp_execute_command`.
  - Resolve helpers: `lsp_completion_item_resolve`, `lsp_code_action_resolve`, `lsp_code_lens_resolve`, `lsp_document_link_resolve`.
  - Diagnostics (pull, proposed in 3.17/3.18): `lsp_text_document_diagnostic`, `lsp_workspace_diagnostic`.
  - Generic: `lsp_call` for any method with raw `params`.
  - Most position-based tools accept: `{ "uri": "file:///...", "position": { "line": N, "character": M }, "serverCommand?": "..." }`.
  - `lsp_references` adds `includeDeclaration?: boolean` (sets `context.includeDeclaration`). `lsp_completion` optionally accepts `context`. Tools also accept `{ "params": <exact LSP params> }` to pass through unchanged.

Capability filtering: `tools/list` probes the configured LSP (`LSP_SERVER_CMD`) and returns only LSP tools the server advertises (plus `lsp_call`).

Additional 3.18 features now supported
- Workspace symbol resolve: `lsp_workspace_symbol_resolve` (if `workspaceSymbolProvider.resolveProvider`).
- File operations (will-requests): `lsp_will_create_files`, `lsp_will_rename_files`, `lsp_will_delete_files` (if `workspace.fileOperations.*`).
- Text Document Content Provider: `lsp_text_document_content` (if `workspace.textDocumentContentProvider`).
- Diagnostic pull requests: `lsp_text_document_diagnostic`, `lsp_workspace_diagnostic`.
- Parameter builders extended for `lsp_inline_value` (requires `context`) and `lsp_signature_help` (optional `context`).
For LSIF usage and examples, see `lsif/README.md`.

 
