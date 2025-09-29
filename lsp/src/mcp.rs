use crate::{handle_tools_call, tools, with_language_pool, LanguageServerPool, Tool};
use anyhow::{anyhow, Result};
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ErrorCode, ErrorData, ListToolsResult,
        PaginatedRequestParam, ServerCapabilities, ServerInfo, Tool as McpTool,
    },
    service::{RequestContext, RoleServer, ServiceExt},
    ServerHandler,
};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::task;

async fn with_language_pool_async<F, R>(f: F) -> Result<R>
where
    F: FnOnce(&mut LanguageServerPool) -> Result<R> + Send + 'static,
    R: Send + 'static,
{
    task::spawn_blocking(move || with_language_pool(f))
        .await
        .map_err(|e| anyhow!("language pool task panicked: {e}"))?
}

fn lsp_capability_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Object(_) => true,
        _ => false,
    }
}

fn filter_tools_by_capabilities(all: Vec<Tool>, caps: Option<Value>) -> Vec<Tool> {
    let Some(caps) = caps else {
        return all;
    };
    let caps_obj = caps.as_object().cloned().unwrap_or_default();
    let has = |k: &str| caps_obj.get(k).map(lsp_capability_truthy).unwrap_or(false);
    let resolve_flag = |k: &str| {
        caps_obj
            .get(k)
            .and_then(|v| v.get("resolveProvider"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false)
    };
    let semantic_full = caps_obj
        .get("semanticTokensProvider")
        .and_then(|v| v.get("full"))
        .cloned();
    let semantic_range = caps_obj
        .get("semanticTokensProvider")
        .and_then(|v| v.get("range"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let semantic_delta = if let Some(Value::Object(o)) = &semantic_full {
        o.get("delta").and_then(|b| b.as_bool()).unwrap_or(false)
    } else {
        false
    };
    let rename_prepare = caps_obj
        .get("renameProvider")
        .and_then(|v| v.get("prepareProvider"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let diag = caps_obj.get("diagnosticProvider").cloned();
    let diag_workspace = diag
        .as_ref()
        .and_then(|v| v.get("workspaceDiagnostics"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let workspace = caps_obj.get("workspace").cloned();
    let ws_obj = workspace.as_ref().and_then(|v| v.as_object());
    let file_ops = ws_obj.and_then(|w| w.get("fileOperations"));
    let text_doc_content_provider = ws_obj
        .and_then(|w| w.get("textDocumentContentProvider"))
        .map(lsp_capability_truthy)
        .unwrap_or(false);

    let mut allowed = HashSet::<String>::new();
    if has("hoverProvider") {
        allowed.insert("lsp_hover".into());
    }
    if has("declarationProvider") {
        allowed.insert("lsp_declaration".into());
    }
    if has("definitionProvider") {
        allowed.insert("lsp_definition".into());
    }
    if has("typeDefinitionProvider") {
        allowed.insert("lsp_type_definition".into());
    }
    if has("implementationProvider") {
        allowed.insert("lsp_implementation".into());
    }
    if has("referencesProvider") {
        allowed.insert("lsp_references".into());
    }
    if caps_obj.get("completionProvider").is_some() {
        allowed.insert("lsp_completion".into());
        if resolve_flag("completionProvider") {
            allowed.insert("lsp_completion_item_resolve".into());
        }
    }
    if caps_obj.get("signatureHelpProvider").is_some() {
        allowed.insert("lsp_signature_help".into());
    }
    if has("documentHighlightProvider") {
        allowed.insert("lsp_document_highlight".into());
    }
    if has("documentSymbolProvider") {
        allowed.insert("lsp_document_symbol".into());
    }
    if has("codeActionProvider") {
        allowed.insert("lsp_code_action".into());
        if resolve_flag("codeActionProvider") {
            allowed.insert("lsp_code_action_resolve".into());
        }
    }
    if caps_obj.get("codeLensProvider").is_some() {
        allowed.insert("lsp_code_lens".into());
        if resolve_flag("codeLensProvider") {
            allowed.insert("lsp_code_lens_resolve".into());
        }
    }
    if caps_obj.get("documentLinkProvider").is_some() {
        allowed.insert("lsp_document_link".into());
        if resolve_flag("documentLinkProvider") {
            allowed.insert("lsp_document_link_resolve".into());
        }
    }
    if has("colorProvider") {
        allowed.insert("lsp_document_color".into());
        allowed.insert("lsp_color_presentation".into());
    }
    if has("documentFormattingProvider") {
        allowed.insert("lsp_formatting".into());
    }
    if has("documentRangeFormattingProvider") {
        allowed.insert("lsp_range_formatting".into());
    }
    if caps_obj.get("documentOnTypeFormattingProvider").is_some() {
        allowed.insert("lsp_on_type_formatting".into());
    }
    if has("renameProvider") {
        allowed.insert("lsp_rename".into());
        if rename_prepare {
            allowed.insert("lsp_prepare_rename".into());
        }
    }
    if has("foldingRangeProvider") {
        allowed.insert("lsp_folding_range".into());
    }
    if has("selectionRangeProvider") {
        allowed.insert("lsp_selection_range".into());
    }
    if has("linkedEditingRangeProvider") {
        allowed.insert("lsp_linked_editing_range".into());
    }
    if has("monikerProvider") {
        allowed.insert("lsp_moniker".into());
    }
    if has("inlineValueProvider") {
        allowed.insert("lsp_inline_value".into());
    }
    if has("inlayHintProvider") {
        allowed.insert("lsp_inlay_hint".into());
        if resolve_flag("inlayHintProvider") {
            allowed.insert("lsp_inlay_hint_resolve".into());
        }
    }
    if caps_obj.get("callHierarchyProvider").is_some() {
        allowed.insert("lsp_call_hierarchy_prepare".into());
        allowed.insert("lsp_call_hierarchy_incoming_calls".into());
        allowed.insert("lsp_call_hierarchy_outgoing_calls".into());
    }
    if caps_obj.get("typeHierarchyProvider").is_some() {
        allowed.insert("lsp_type_hierarchy_prepare".into());
        allowed.insert("lsp_type_hierarchy_supertypes".into());
        allowed.insert("lsp_type_hierarchy_subtypes".into());
    }
    if caps_obj.get("semanticTokensProvider").is_some() {
        if matches!(
            semantic_full,
            Some(Value::Bool(true)) | Some(Value::Object(_))
        ) {
            allowed.insert("lsp_semantic_tokens_full".into());
        }
        if semantic_delta {
            allowed.insert("lsp_semantic_tokens_full_delta".into());
        }
        if semantic_range {
            allowed.insert("lsp_semantic_tokens_range".into());
        }
    }
    if has("workspaceSymbolProvider") {
        allowed.insert("lsp_workspace_symbol".into());
        if resolve_flag("workspaceSymbolProvider") {
            allowed.insert("lsp_workspace_symbol_resolve".into());
        }
    }
    if caps_obj.get("executeCommandProvider").is_some() {
        allowed.insert("lsp_execute_command".into());
    }
    if let Some(fops) = file_ops.and_then(|v| v.as_object()) {
        if fops.get("willCreate").is_some() {
            allowed.insert("lsp_will_create_files".into());
        }
        if fops.get("willRename").is_some() {
            allowed.insert("lsp_will_rename_files".into());
        }
        if fops.get("willDelete").is_some() {
            allowed.insert("lsp_will_delete_files".into());
        }
    }
    if text_doc_content_provider {
        allowed.insert("lsp_text_document_content".into());
    }
    if diag.is_some() {
        allowed.insert("lsp_text_document_diagnostic".into());
        if diag_workspace {
            allowed.insert("lsp_workspace_diagnostic".into());
        }
    }

    all.into_iter()
        .filter(|t| {
            let n = t.name.as_str();
            if n == "lsp_call" {
                return true;
            }
            if n.starts_with("lsp_") {
                return allowed.contains(n);
            }
            true
        })
        .collect()
}

fn convert_tool_to_mcp(tool: Tool) -> McpTool {
    let schema = Arc::new(
        tool.input_schema
            .as_object()
            .cloned()
            .expect("tool schema must be an object"),
    );
    if let Some(desc) = tool.description {
        McpTool::new(tool.name, desc, schema)
    } else {
        let mut tool = McpTool::new(tool.name, String::new(), schema);
        tool.description = None;
        tool
    }
}

async fn list_available_tools() -> Result<Vec<McpTool>> {
    let all = tools();
    let caps = with_language_pool_async(|pool| pool.probe_default_capabilities()).await?;
    let filtered = filter_tools_by_capabilities(all, caps);
    Ok(filtered.into_iter().map(convert_tool_to_mcp).collect())
}

fn server_info() -> ServerInfo {
    ServerInfo {
        instructions: Some(
            "Bridge MCP tools to language servers via the Language Server Protocol.".to_string(),
        ),
        capabilities: ServerCapabilities::builder().enable_tools().build(),
        ..ServerInfo::default()
    }
}

fn internal_error(context: &str, err: anyhow::Error) -> ErrorData {
    let data = json!({ "details": format!("{:#}", err) });
    ErrorData::internal_error(format!("{context}: {err}"), Some(data))
}

async fn call_tool_via_mcp(request: CallToolRequestParam) -> Result<CallToolResult, ErrorData> {
    let name = request.name.clone().into_owned();
    let mut params = Map::new();
    params.insert("name".into(), Value::String(name));
    let arguments = request
        .arguments
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));
    params.insert("arguments".into(), arguments);
    let response = handle_tools_call(Some(Value::Object(params))).await;
    if let Some(error) = response.error {
        return Err(ErrorData::new(
            ErrorCode(error.code as i32),
            error.message,
            error.data,
        ));
    }
    let result = response
        .result
        .ok_or_else(|| ErrorData::internal_error("Tool call missing result", None))?;
    Ok(CallToolResult::structured(result))
}

struct CodexLspServer;

impl ServerHandler for CodexLspServer {
    fn get_info(&self) -> ServerInfo {
        server_info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = list_available_tools()
            .await
            .map_err(|err| internal_error("Failed to list tools", err))?;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        call_tool_via_mcp(request).await
    }
}

pub async fn run() -> Result<()> {
    let server = CodexLspServer;
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
