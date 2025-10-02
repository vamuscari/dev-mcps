mod ls;
mod mcp;
use anyhow::{anyhow, Context, Result};
use ls::LanguageServerManager;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::{Mutex, OnceLock};
use tokio::task;
use url::Url;

#[derive(Clone)]
pub(crate) struct Tool {
    name: String,
    description: Option<String>,
    input_schema: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ErrorObject {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl ErrorObject {
    fn new(code: i64, message: &str, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.to_string(),
            data,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct JsonRpcResponse {
    result: Option<Value>,
    error: Option<ErrorObject>,
}

impl JsonRpcResponse {
    fn result(result: Value) -> Self {
        Self {
            result: Some(result),
            error: None,
        }
    }

    fn error(error: ErrorObject) -> Self {
        Self {
            result: None,
            error: Some(error),
        }
    }
}

struct LspInvocation {
    method: &'static str,
    params: Value,
    server_cmd: Option<String>,
    uri_hint: Option<String>,
}

fn invalid_params_error(message: &str) -> ErrorObject {
    ErrorObject::new(-32602, message, None)
}

fn unsupported_tool_error(tool: &str) -> ErrorObject {
    ErrorObject::new(
        -32601,
        "Unsupported lsp tool",
        Some(json!({ "tool": tool })),
    )
}

fn require_string_field(args: &Map<String, Value>, key: &str) -> Result<String, ErrorObject> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .ok_or_else(|| invalid_params_error(&format!("Missing required field: {key}")))
}

fn require_object_field(args: &Map<String, Value>, key: &str) -> Result<Value, ErrorObject> {
    match args.get(key) {
        Some(Value::Object(_)) => Ok(args[key].clone()),
        Some(_) => Err(invalid_params_error(&format!(
            "Field '{key}' must be an object"
        ))),
        None => Err(invalid_params_error(&format!(
            "Missing required field: {key}"
        ))),
    }
}

fn require_array_field(args: &Map<String, Value>, key: &str) -> Result<Value, ErrorObject> {
    match args.get(key) {
        Some(Value::Array(_)) => Ok(args[key].clone()),
        Some(_) => Err(invalid_params_error(&format!(
            "Field '{key}' must be an array"
        ))),
        None => Err(invalid_params_error(&format!(
            "Missing required field: {key}"
        ))),
    }
}

fn require_value_field(args: &Map<String, Value>, key: &str) -> Result<Value, ErrorObject> {
    args.get(key)
        .cloned()
        .ok_or_else(|| invalid_params_error(&format!("Missing required field: {key}")))
}

fn canonical_uri(args: &Map<String, Value>) -> Result<String, ErrorObject> {
    let raw = require_string_field(args, "uri")?;
    Ok(LanguageServerPool::normalize_uri(&raw))
}

fn build_lsp_invocation(
    tool: &str,
    args: &Map<String, Value>,
    server_cmd: Option<String>,
) -> Result<LspInvocation, ErrorObject> {
    let make_invocation =
        |method: &'static str, params: Value, uri_hint: Option<String>| -> LspInvocation {
            LspInvocation {
                method,
                params,
                server_cmd: server_cmd.clone(),
                uri_hint,
            }
        };

    match tool {
        "lsp_hover"
        | "lsp_definition"
        | "lsp_type_definition"
        | "lsp_implementation"
        | "lsp_document_highlight"
        | "lsp_linked_editing_range"
        | "lsp_moniker"
        | "lsp_prepare_rename"
        | "lsp_declaration" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            let method = match tool {
                "lsp_hover" => "textDocument/hover",
                "lsp_definition" => "textDocument/definition",
                "lsp_type_definition" => "textDocument/typeDefinition",
                "lsp_implementation" => "textDocument/implementation",
                "lsp_document_highlight" => "textDocument/documentHighlight",
                "lsp_linked_editing_range" => "textDocument/linkedEditingRange",
                "lsp_moniker" => "textDocument/moniker",
                "lsp_prepare_rename" => "textDocument/prepareRename",
                "lsp_declaration" => "textDocument/declaration",
                _ => unreachable!(),
            };
            Ok(make_invocation(
                method,
                json!({
                    "textDocument": {"uri": uri},
                    "position": position
                }),
                Some(uri),
            ))
        }
        "lsp_completion" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            let mut payload = json!({
                "textDocument": {"uri": uri},
                "position": position
            });
            if let Some(context) = args.get("context") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("context".into(), context.clone());
                }
            }
            Ok(make_invocation(
                "textDocument/completion",
                payload,
                Some(uri),
            ))
        }
        "lsp_signature_help" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            let mut payload = json!({
                "textDocument": {"uri": uri},
                "position": position
            });
            if let Some(context) = args.get("context") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("context".into(), context.clone());
                }
            }
            Ok(make_invocation(
                "textDocument/signatureHelp",
                payload,
                Some(uri),
            ))
        }
        "lsp_references" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            let include = args
                .get("includeDeclaration")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(make_invocation(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                    "context": {"includeDeclaration": include}
                }),
                Some(uri),
            ))
        }
        "lsp_selection_range" => {
            let uri = canonical_uri(args)?;
            let positions = require_array_field(args, "positions")?;
            Ok(make_invocation(
                "textDocument/selectionRange",
                json!({
                    "textDocument": {"uri": uri},
                    "positions": positions
                }),
                Some(uri),
            ))
        }
        "lsp_folding_range" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "textDocument/foldingRange",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_document_symbol" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "textDocument/documentSymbol",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_workspace_symbol" => {
            let query = require_string_field(args, "query")?;
            Ok(make_invocation(
                "workspace/symbol",
                json!({ "query": query }),
                None,
            ))
        }
        "lsp_workspace_symbol_resolve" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("workspaceSymbol/resolve", item, None))
        }
        "lsp_rename" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            let new_name = require_string_field(args, "newName")?;
            Ok(make_invocation(
                "textDocument/rename",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                    "newName": new_name
                }),
                Some(uri),
            ))
        }
        "lsp_code_action" => {
            let uri = canonical_uri(args)?;
            let range = require_object_field(args, "range")?;
            let context = require_value_field(args, "context")?;
            Ok(make_invocation(
                "textDocument/codeAction",
                json!({
                    "textDocument": {"uri": uri},
                    "range": range,
                    "context": context
                }),
                Some(uri),
            ))
        }
        "lsp_code_action_resolve" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("codeAction/resolve", item, None))
        }
        "lsp_completion_item_resolve" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("completionItem/resolve", item, None))
        }
        "lsp_code_lens" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "textDocument/codeLens",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_code_lens_resolve" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("codeLens/resolve", item, None))
        }
        "lsp_document_link" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "textDocument/documentLink",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_document_link_resolve" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("documentLink/resolve", item, None))
        }
        "lsp_document_color" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "textDocument/documentColor",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_color_presentation" => {
            let uri = canonical_uri(args)?;
            let color = require_value_field(args, "color")?;
            let range = require_object_field(args, "range")?;
            Ok(make_invocation(
                "textDocument/colorPresentation",
                json!({
                    "textDocument": {"uri": uri},
                    "color": color,
                    "range": range
                }),
                Some(uri),
            ))
        }
        "lsp_formatting" => {
            let uri = canonical_uri(args)?;
            let options = require_object_field(args, "options")?;
            Ok(make_invocation(
                "textDocument/formatting",
                json!({
                    "textDocument": {"uri": uri},
                    "options": options
                }),
                Some(uri),
            ))
        }
        "lsp_range_formatting" => {
            let uri = canonical_uri(args)?;
            let range = require_object_field(args, "range")?;
            let options = require_object_field(args, "options")?;
            Ok(make_invocation(
                "textDocument/rangeFormatting",
                json!({
                    "textDocument": {"uri": uri},
                    "range": range,
                    "options": options
                }),
                Some(uri),
            ))
        }
        "lsp_on_type_formatting" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            let ch = require_string_field(args, "ch")?;
            let options = require_object_field(args, "options")?;
            Ok(make_invocation(
                "textDocument/onTypeFormatting",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                    "ch": ch,
                    "options": options
                }),
                Some(uri),
            ))
        }
        "lsp_inline_value" => {
            let uri = canonical_uri(args)?;
            let range = require_object_field(args, "range")?;
            let context = require_value_field(args, "context")?;
            Ok(make_invocation(
                "textDocument/inlineValue",
                json!({
                    "textDocument": {"uri": uri},
                    "range": range,
                    "context": context
                }),
                Some(uri),
            ))
        }
        "lsp_inlay_hint" => {
            let uri = canonical_uri(args)?;
            let range = require_object_field(args, "range")?;
            Ok(make_invocation(
                "textDocument/inlayHint",
                json!({
                    "textDocument": {"uri": uri},
                    "range": range
                }),
                Some(uri),
            ))
        }
        "lsp_inlay_hint_resolve" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("inlayHint/resolve", item, None))
        }
        "lsp_call_hierarchy_prepare" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            Ok(make_invocation(
                "textDocument/prepareCallHierarchy",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position
                }),
                Some(uri),
            ))
        }
        "lsp_call_hierarchy_incoming_calls" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("callHierarchy/incomingCalls", item, None))
        }
        "lsp_call_hierarchy_outgoing_calls" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("callHierarchy/outgoingCalls", item, None))
        }
        "lsp_type_hierarchy_prepare" => {
            let uri = canonical_uri(args)?;
            let position = require_object_field(args, "position")?;
            Ok(make_invocation(
                "textDocument/prepareTypeHierarchy",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position
                }),
                Some(uri),
            ))
        }
        "lsp_type_hierarchy_supertypes" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("typeHierarchy/supertypes", item, None))
        }
        "lsp_type_hierarchy_subtypes" => {
            let item = require_object_field(args, "item")?;
            Ok(make_invocation("typeHierarchy/subtypes", item, None))
        }
        "lsp_semantic_tokens_full" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "textDocument/semanticTokens/full",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_semantic_tokens_full_delta" => {
            let uri = canonical_uri(args)?;
            let prev = require_string_field(args, "previousResultId")?;
            Ok(make_invocation(
                "textDocument/semanticTokens/full/delta",
                json!({
                    "textDocument": {"uri": uri},
                    "previousResultId": prev
                }),
                Some(uri),
            ))
        }
        "lsp_semantic_tokens_range" => {
            let uri = canonical_uri(args)?;
            let range = require_object_field(args, "range")?;
            Ok(make_invocation(
                "textDocument/semanticTokens/range",
                json!({
                    "textDocument": {"uri": uri},
                    "range": range
                }),
                Some(uri),
            ))
        }
        "lsp_execute_command" => {
            let command = require_string_field(args, "command")?;
            let params = if let Some(arguments) = args.get("arguments") {
                json!({ "command": command, "arguments": arguments.clone() })
            } else {
                json!({ "command": command })
            };
            Ok(make_invocation("workspace/executeCommand", params, None))
        }
        "lsp_will_create_files" => {
            let files = require_array_field(args, "files")?;
            Ok(make_invocation(
                "workspace/willCreateFiles",
                json!({ "files": files }),
                None,
            ))
        }
        "lsp_will_rename_files" => {
            let files = require_array_field(args, "files")?;
            Ok(make_invocation(
                "workspace/willRenameFiles",
                json!({ "files": files }),
                None,
            ))
        }
        "lsp_will_delete_files" => {
            let files = require_array_field(args, "files")?;
            Ok(make_invocation(
                "workspace/willDeleteFiles",
                json!({ "files": files }),
                None,
            ))
        }
        "lsp_text_document_content" => {
            let uri = canonical_uri(args)?;
            Ok(make_invocation(
                "workspace/textDocumentContent",
                json!({ "textDocument": {"uri": uri} }),
                Some(uri),
            ))
        }
        "lsp_text_document_diagnostic" => {
            let uri = canonical_uri(args)?;
            let mut payload = json!({ "textDocument": {"uri": uri} });
            if let Some(idf) = args.get("identifier") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("identifier".into(), idf.clone());
                }
            }
            if let Some(prev) = args.get("previousResultId") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("previousResultId".into(), prev.clone());
                }
            }
            Ok(make_invocation(
                "textDocument/diagnostic",
                payload,
                Some(uri),
            ))
        }
        "lsp_workspace_diagnostic" => {
            let mut payload = json!({});
            if let Some(prev) = args.get("previousResultIds") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("previousResultIds".into(), prev.clone());
                }
            }
            if let Some(idf) = args.get("identifier") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("identifier".into(), idf.clone());
                }
            }
            Ok(make_invocation("workspace/diagnostic", payload, None))
        }
        _ => Err(unsupported_tool_error(tool)),
    }
}

async fn handle_lsp_call(
    mut args: Map<String, Value>,
    server_cmd: Option<String>,
) -> JsonRpcResponse {
    let method = match args.remove("method").and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    }) {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(invalid_params_error("Missing required field: method"))
        }
    };

    let params_value = args
        .remove("params")
        .map(parse_params_value)
        .unwrap_or_else(|| json!({}));

    let uri_hint = args
        .remove("uri")
        .and_then(|v| match v {
            Value::String(s) => Some(LanguageServerPool::normalize_uri(&s)),
            _ => None,
        })
        .or_else(|| uri_from_params(&params_value).map(|s| LanguageServerPool::normalize_uri(&s)));

    let language_hint = if method == "textDocument/didOpen" {
        language_from_did_open(&params_value)
    } else {
        None
    };
    let is_open = method == "textDocument/didOpen";
    let is_close = method == "textDocument/didClose";

    let method_for_request = method.clone();
    let params_for_request = params_value.clone();
    let uri_hint_for_request = uri_hint.clone();
    let language_hint_for_request = language_hint.clone();
    let server_cmd_for_request = server_cmd.clone();

    let result = task::spawn_blocking(move || {
        with_language_pool(|pool| {
            let cmd = pool.resolve_command(
                server_cmd_for_request.as_deref(),
                uri_hint_for_request.as_deref(),
                language_hint_for_request.as_deref(),
            )?;
            if is_open {
                if let Some(uri) = uri_hint_for_request.as_deref() {
                    pool.associate_document(uri, &cmd);
                }
            }
            let need_open = if let Some(uri) = uri_hint_for_request.as_deref() {
                !(is_open || is_close || pool.has_document(uri))
            } else {
                false
            };
            let open_params = if need_open {
                if let Some(uri) = uri_hint_for_request.as_ref() {
                    Some(pool.build_did_open_params(uri, language_hint_for_request.as_deref())?)
                } else {
                    None
                }
            } else {
                None
            };
            let outcome = pool.with_manager(&cmd, |lsm| {
                if let Some(payload) = open_params.as_ref() {
                    lsm.notify("textDocument/didOpen", payload.clone(), Some(cmd.as_str()))?;
                }
                lsm.request(
                    &method_for_request,
                    params_for_request.clone(),
                    Some(cmd.as_str()),
                )
            })?;
            if need_open {
                if let Some(uri) = uri_hint_for_request.as_ref() {
                    pool.associate_document(uri, &cmd);
                }
            }
            if is_close {
                if let Some(uri) = uri_hint_for_request.as_ref() {
                    pool.release_document(uri);
                }
            }
            Ok(outcome)
        })
    })
    .await;

    match result {
        Ok(Ok(value)) => JsonRpcResponse::result(json!({
            "tool": "lsp_call",
            "status": "ok",
            "result": value
        })),
        Ok(Err(e)) => {
            let data = build_error_data(
                "lsp_call",
                Some(&method),
                uri_hint.as_deref(),
                server_cmd.as_deref(),
                &e,
            );
            if let Ok(json_data) = serde_json::to_string(&data) {
                eprintln!("mcp-lsp: tool 'lsp_call' failed -> {}", json_data);
            }
            let message = format_tool_error_message("lsp_call", Some(&method), &e);
            JsonRpcResponse::error(ErrorObject::new(-32050, &message, Some(data)))
        }
        Err(join_err) => {
            let err = anyhow::Error::new(join_err);
            let data = build_error_data(
                "lsp_call",
                Some(&method),
                uri_hint.as_deref(),
                server_cmd.as_deref(),
                &err,
            );
            if let Ok(json_data) = serde_json::to_string(&data) {
                eprintln!("mcp-lsp: tool 'lsp_call' failed -> {}", json_data);
            }
            let message = format_tool_error_message("lsp_call", Some(&method), &err);
            JsonRpcResponse::error(ErrorObject::new(-32050, &message, Some(data)))
        }
    }
}

async fn handle_lsp_notify(
    mut args: Map<String, Value>,
    server_cmd: Option<String>,
) -> JsonRpcResponse {
    let method = match args.remove("method").and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    }) {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(invalid_params_error("Missing required field: method"))
        }
    };

    let params_value = args.remove("params").unwrap_or(json!({}));
    let uri_hint = args
        .remove("uri")
        .and_then(|v| match v {
            Value::String(s) => Some(LanguageServerPool::normalize_uri(&s)),
            _ => None,
        })
        .or_else(|| uri_from_params(&params_value).map(|s| LanguageServerPool::normalize_uri(&s)));
    let language_hint = if method == "textDocument/didOpen" {
        language_from_did_open(&params_value)
    } else {
        None
    };
    let is_open = method == "textDocument/didOpen";
    let is_close = method == "textDocument/didClose";

    let method_for_request = method.clone();
    let params_for_request = params_value.clone();
    let uri_hint_for_request = uri_hint.clone();
    let language_hint_for_request = language_hint.clone();
    let server_cmd_for_request = server_cmd.clone();

    let result = task::spawn_blocking(move || {
        with_language_pool(|pool| {
            let cmd = pool.resolve_command(
                server_cmd_for_request.as_deref(),
                uri_hint_for_request.as_deref(),
                language_hint_for_request.as_deref(),
            )?;
            pool.with_manager(&cmd, |lsm| {
                lsm.notify(
                    &method_for_request,
                    params_for_request.clone(),
                    Some(cmd.as_str()),
                )
            })?;
            if is_open {
                if let Some(uri) = uri_hint_for_request.as_ref() {
                    pool.associate_document(uri, &cmd);
                }
            }
            if is_close {
                if let Some(uri) = uri_hint_for_request.as_ref() {
                    pool.release_document(uri);
                }
            }
            Ok(())
        })
    })
    .await;

    match result {
        Ok(Ok(())) => JsonRpcResponse::result(json!({
            "tool": "lsp_notify",
            "status": "ok"
        })),
        Ok(Err(e)) => {
            let data = build_error_data(
                "lsp_notify",
                Some(&method),
                uri_hint.as_deref(),
                server_cmd.as_deref(),
                &e,
            );
            if let Ok(json_data) = serde_json::to_string(&data) {
                eprintln!("mcp-lsp: tool 'lsp_notify' failed -> {}", json_data);
            }
            let message = format_tool_error_message("lsp_notify", Some(&method), &e);
            JsonRpcResponse::error(ErrorObject::new(-32050, &message, Some(data)))
        }
        Err(join_err) => {
            let err = anyhow::Error::new(join_err);
            let data = build_error_data(
                "lsp_notify",
                Some(&method),
                uri_hint.as_deref(),
                server_cmd.as_deref(),
                &err,
            );
            if let Ok(json_data) = serde_json::to_string(&data) {
                eprintln!("mcp-lsp: tool 'lsp_notify' failed -> {}", json_data);
            }
            let message = format_tool_error_message("lsp_notify", Some(&method), &err);
            JsonRpcResponse::error(ErrorObject::new(-32050, &message, Some(data)))
        }
    }
}

/// Tracks running language servers and routes requests based on languageId/extension,
/// falling back to the most recently used server or environment overrides when
/// document hints are unavailable.
pub(crate) struct LanguageServerPool {
    default_cmd: Option<String>,
    managers: HashMap<String, LanguageServerManager>,
    doc_servers: HashMap<String, String>,
    lang_map: HashMap<String, String>,
    ext_map: HashMap<String, String>,
    ext_language_map: HashMap<String, String>,
    last_server: Option<String>,
}

impl LanguageServerPool {
    fn new() -> Self {
        let default_cmd = std::env::var("LSP_SERVER_CMD").ok();
        let (mut lang_map, mut ext_map, mut ext_language_map) = Self::built_in_server_map();
        Self::load_server_map_overrides(&mut lang_map, &mut ext_map, &mut ext_language_map);
        Self {
            default_cmd,
            managers: HashMap::new(),
            doc_servers: HashMap::new(),
            lang_map,
            ext_map,
            ext_language_map,
            last_server: None,
        }
    }

    fn built_in_server_map() -> (
        HashMap<String, String>,
        HashMap<String, String>,
        HashMap<String, String>,
    ) {
        let mut lang_map = HashMap::new();
        let mut ext_map = HashMap::new();
        let mut ext_language_map = HashMap::new();

        let language_defaults: &[(&str, &str)] = &[
            ("bash", "bash-language-server start"),
            ("c", "clangd"),
            ("cpp", "clangd"),
            ("go", "gopls"),
            ("javascript", "typescript-language-server --stdio"),
            ("javascriptreact", "typescript-language-server --stdio"),
            ("json", "vscode-json-language-server --stdio"),
            ("jsonc", "vscode-json-language-server --stdio"),
            ("markdown", "marksman"),
            ("python", "pylsp"),
            ("rust", "rust-analyzer"),
            ("shell", "bash-language-server start"),
            ("shellscript", "bash-language-server start"),
            ("toml", "taplo lsp"),
            ("typescript", "typescript-language-server --stdio"),
            ("typescriptreact", "typescript-language-server --stdio"),
            ("zig", "zls"),
            ("yaml", "yaml-language-server --stdio"),
        ];

        for (lang, cmd) in language_defaults {
            lang_map.insert((*lang).to_ascii_lowercase(), (*cmd).to_string());
        }

        let extension_defaults: &[(&str, &str)] = &[
            ("bash", "bash-language-server start"),
            ("c", "clangd"),
            ("cc", "clangd"),
            ("cpp", "clangd"),
            ("cxx", "clangd"),
            ("go", "gopls"),
            ("h", "clangd"),
            ("hpp", "clangd"),
            ("hh", "clangd"),
            ("js", "typescript-language-server --stdio"),
            ("jsx", "typescript-language-server --stdio"),
            ("json", "vscode-json-language-server --stdio"),
            ("jsonc", "vscode-json-language-server --stdio"),
            ("md", "marksman"),
            ("mdx", "marksman"),
            ("py", "pylsp"),
            ("pyi", "pylsp"),
            ("rs", "rust-analyzer"),
            ("sh", "bash-language-server start"),
            ("toml", "taplo lsp"),
            ("ts", "typescript-language-server --stdio"),
            ("tsx", "typescript-language-server --stdio"),
            ("yaml", "yaml-language-server --stdio"),
            ("yml", "yaml-language-server --stdio"),
            ("zig", "zls"),
        ];

        for (ext, cmd) in extension_defaults {
            ext_map.insert((*ext).to_ascii_lowercase(), (*cmd).to_string());
        }

        let extension_languages: &[(&str, &str)] = &[
            ("bash", "shell"),
            ("c", "c"),
            ("cc", "cpp"),
            ("cpp", "cpp"),
            ("cxx", "cpp"),
            ("go", "go"),
            ("h", "c"),
            ("hpp", "cpp"),
            ("hh", "cpp"),
            ("js", "javascript"),
            ("jsx", "javascriptreact"),
            ("json", "json"),
            ("jsonc", "json"),
            ("md", "markdown"),
            ("mdx", "markdown"),
            ("py", "python"),
            ("pyi", "python"),
            ("rs", "rust"),
            ("sh", "shell"),
            ("toml", "toml"),
            ("ts", "typescript"),
            ("tsx", "typescriptreact"),
            ("yaml", "yaml"),
            ("yml", "yaml"),
            ("zig", "zig"),
        ];
        for (ext, lang) in extension_languages {
            ext_language_map.insert((*ext).to_ascii_lowercase(), (*lang).to_string());
        }

        (lang_map, ext_map, ext_language_map)
    }

    fn load_server_map_overrides(
        lang_map: &mut HashMap<String, String>,
        ext_map: &mut HashMap<String, String>,
        ext_language_map: &mut HashMap<String, String>,
    ) {
        if let Ok(raw) = std::env::var("LSP_SERVER_MAP") {
            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                Self::populate_server_map(&value, lang_map, ext_map, ext_language_map);
            } else {
                eprintln!("warning: failed to parse LSP_SERVER_MAP as JSON");
            }
        }
    }

    fn populate_server_map(
        value: &Value,
        lang_map: &mut HashMap<String, String>,
        ext_map: &mut HashMap<String, String>,
        ext_language_map: &mut HashMap<String, String>,
    ) {
        if let Value::Object(obj) = value {
            for (key, val) in obj {
                if key.eq_ignore_ascii_case("languages") || key.eq_ignore_ascii_case("language") {
                    if let Value::Object(inner) = val {
                        for (lang, cmd) in inner {
                            if let Some(cmd_str) = cmd.as_str() {
                                lang_map.insert(lang.to_ascii_lowercase(), cmd_str.to_string());
                            }
                        }
                    }
                    continue;
                }
                if key.eq_ignore_ascii_case("extensions") || key.eq_ignore_ascii_case("extension") {
                    if let Value::Object(inner) = val {
                        for (ext, cmd) in inner {
                            if let Some(cmd_str) = cmd.as_str() {
                                let canonical = ext.trim_start_matches('.').to_ascii_lowercase();
                                ext_map.insert(canonical.clone(), cmd_str.to_string());
                                ext_language_map
                                    .entry(canonical.clone())
                                    .or_insert(canonical.clone());
                            }
                        }
                    }
                    continue;
                }
                if let Some(cmd_str) = val.as_str() {
                    if let Some(rest) = key.strip_prefix("lang:") {
                        lang_map.insert(rest.to_ascii_lowercase(), cmd_str.to_string());
                    } else if let Some(rest) = key.strip_prefix("ext:") {
                        let canonical = rest.trim_start_matches('.').to_ascii_lowercase();
                        ext_map.insert(canonical.clone(), cmd_str.to_string());
                        ext_language_map
                            .entry(canonical.clone())
                            .or_insert(canonical.clone());
                    } else if key.starts_with('.') {
                        let canonical = key.trim_start_matches('.').to_ascii_lowercase();
                        ext_map.insert(canonical.clone(), cmd_str.to_string());
                        ext_language_map
                            .entry(canonical.clone())
                            .or_insert(canonical.clone());
                    } else {
                        lang_map.insert(key.to_ascii_lowercase(), cmd_str.to_string());
                    }
                }
            }
        }
    }

    fn resolve_command(
        &mut self,
        explicit: Option<&str>,
        uri: Option<&str>,
        language: Option<&str>,
    ) -> Result<String> {
        if let Some(cmd) = explicit {
            return Ok(cmd.to_string());
        }
        if let Some(uri) = uri {
            let key = Self::normalize_uri(uri);
            if let Some(cmd) = self.doc_servers.get(&key) {
                return Ok(cmd.clone());
            }
        }
        if let Some(lang) = language {
            let key = lang.to_ascii_lowercase();
            if let Some(cmd) = self.lang_map.get(&key) {
                return Ok(cmd.clone());
            }
        }
        if let Some(uri) = uri {
            let key = Self::normalize_uri(uri);
            if let Some(ext) = Self::extension_from_uri(&key) {
                if let Some(cmd) = self.ext_map.get(&ext) {
                    return Ok(cmd.clone());
                }
            }
        }
        if let Some(cmd) = self.default_cmd.clone() {
            Ok(cmd)
        } else {
            Err(anyhow!(
                "No language server registered for this request. Install a supported server for the file type or configure overrides via LSP_SERVER_MAP/serverCommand."
            ))
        }
    }

    fn with_manager<F, T>(&mut self, cmd: &str, f: F) -> Result<T>
    where
        F: FnOnce(&mut LanguageServerManager) -> Result<T>,
    {
        let manager = self
            .managers
            .entry(cmd.to_string())
            .or_insert_with(|| LanguageServerManager::with_command(cmd.to_string()));
        self.last_server = Some(cmd.to_string());
        f(manager)
    }

    fn associate_document(&mut self, uri: &str, cmd: &str) {
        let key = Self::normalize_uri(uri);
        self.doc_servers.insert(key, cmd.to_string());
        self.last_server = Some(cmd.to_string());
    }

    fn release_document(&mut self, uri: &str) {
        let key = Self::normalize_uri(uri);
        let removed = self.doc_servers.remove(&key);
        if let Some(command) = removed {
            if self.doc_servers.values().any(|c| c == &command) {
                self.last_server = Some(command);
            } else {
                self.last_server = self.doc_servers.values().next().cloned();
            }
        }
    }

    fn shutdown_all(&mut self) -> Result<()> {
        for manager in self.managers.values_mut() {
            manager.shutdown()?;
        }
        self.managers.clear();
        self.doc_servers.clear();
        self.last_server = None;
        Ok(())
    }

    fn probe_default_capabilities(&mut self) -> Result<Option<Value>> {
        let Some(cmd) = self.default_cmd.clone() else {
            return Ok(None);
        };
        self.with_manager(&cmd, |lsm| lsm.capabilities(Some(&cmd)))
    }

    fn extension_from_uri(uri: &str) -> Option<String> {
        let path_part = uri.strip_prefix("file://").unwrap_or(uri);
        let path = std::path::Path::new(path_part);
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            return Some(ext.to_ascii_lowercase());
        }
        let segment = path_part
            .rsplit(|c: char| std::path::is_separator(c))
            .next()
            .unwrap_or(path_part);
        segment
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
    }

    fn path_from_uri(uri: &str) -> std::path::PathBuf {
        if let Ok(url) = Url::parse(uri) {
            if url.scheme() == "file" {
                if let Ok(path) = url.to_file_path() {
                    return path;
                }
            }
        }
        if let Some(stripped) = uri.strip_prefix("file://") {
            #[cfg(windows)]
            {
                let trimmed = if let Some(rest) = stripped.strip_prefix('/') {
                    let mut chars = rest.chars();
                    match (chars.next(), chars.next()) {
                        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => rest,
                        _ => stripped,
                    }
                } else {
                    stripped
                };
                return std::path::PathBuf::from(trimmed);
            }
            #[cfg(not(windows))]
            {
                return std::path::PathBuf::from(stripped);
            }
        }
        std::path::PathBuf::from(uri)
    }

    fn language_from_extension(&self, ext: &str) -> Option<String> {
        self.ext_language_map.get(ext).cloned()
    }

    fn has_document(&self, uri: &str) -> bool {
        let key = Self::normalize_uri(uri);
        self.doc_servers.contains_key(&key)
    }

    fn normalize_uri(uri: &str) -> String {
        if let Ok(url) = Url::parse(uri) {
            if url.scheme() == "file" {
                return url.to_string();
            }
        }

        let path = std::path::Path::new(uri);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(path)
        } else {
            path.to_path_buf()
        };

        Url::from_file_path(&abs)
            .map(|url| url.to_string())
            .unwrap_or_else(|_| {
                #[cfg(windows)]
                {
                    let mut path_str = abs.to_string_lossy().replace('\\', "/");
                    if !path_str.starts_with('/') {
                        path_str = format!("/{path_str}");
                    }
                    format!("file://{path_str}")
                }
                #[cfg(not(windows))]
                {
                    format!("file://{}", abs.to_string_lossy())
                }
            })
    }

    fn build_did_open_params(&self, uri: &str, language_hint: Option<&str>) -> Result<Value> {
        let canonical_uri = Self::normalize_uri(uri);
        let path = Self::path_from_uri(&canonical_uri);
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("stat document content for {:?}", path))?;
        const MAX_INLINE_DOC_BYTES: u64 = 2 * 1024 * 1024;
        if metadata.len() > MAX_INLINE_DOC_BYTES {
            return Err(anyhow!(
                "Document {} is {} bytes; mcp-lsp will not inline files larger than 2 MiB. Provide a smaller file or send the content explicitly via didOpen.",
                canonical_uri,
                metadata.len()
            ));
        }

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                return Err(anyhow!(
                    "Document {} is not present on disk. Save the buffer first or send your own textDocument/didOpen payload.",
                    canonical_uri
                ));
            }
            Err(err) => {
                return Err(anyhow!(err).context(format!("read document content for {:?}", path)));
            }
        };
        let language_id = language_hint
            .map(|s| s.to_string())
            .or_else(|| {
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(|ext| ext.to_ascii_lowercase())
                    .and_then(|ext| self.language_from_extension(&ext))
            })
            .unwrap_or_else(|| "plaintext".to_string());
        Ok(json!({
            "textDocument": {
                "uri": canonical_uri,
                "languageId": language_id,
                "version": 1,
                "text": text
            }
        }))
    }
}

pub(crate) fn with_language_pool<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&mut LanguageServerPool) -> Result<T>,
{
    static POOL: OnceLock<Mutex<LanguageServerPool>> = OnceLock::new();
    let lock = POOL.get_or_init(|| Mutex::new(LanguageServerPool::new()));
    let mut guard = lock.lock().expect("language server pool mutex poisoned");
    f(&mut guard)
}

pub(crate) fn tools() -> Vec<Tool> {
    const URI_DESC: &str = "Document URI. Use a file:// URI or absolute path inside the workspace.";
    const POSITION_DESC: &str = "Zero-based position {line, character}.";
    const RANGE_DESC: &str = "Range with zero-based start and end positions.";
    const SERVER_CMD_DESC: &str = "Optional override for the language server command. When omitted, mcp-lsp chooses based on languageId/extension or falls back to LSP_SERVER_CMD.";
    const SERVER_NOTE: &str =
        "Use `serverCommand` to override the configured language server for a single request.";

    let lsp_positional_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "position": {
                "type": "object",
                "description": POSITION_DESC,
                "properties": {
                    "line": {"type": "integer", "minimum": 0, "description": "Zero-based line."},
                    "character": {"type": "integer", "minimum": 0, "description": "Zero-based character within the line."}
                },
                "required": ["line", "character"]
            },
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri", "position"],
        "additionalProperties": false
    });

    let lsp_references_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "position": lsp_positional_schema
                .get("properties").unwrap()
                .get("position").unwrap()
                .clone(),
            "includeDeclaration": {
                "type": "boolean",
                "default": false,
                "description": "When true, include the declaration site in the response."
            },
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri", "position"],
        "additionalProperties": false
    });

    let lsp_call_schema = json!({
        "type": "object",
        "properties": {
            "method": {"type": "string", "description": "LSP method name (e.g. textDocument/hover)."},
            "params": {"description": "Arbitrary JSON params forwarded verbatim to the language server."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["method"],
        "additionalProperties": true
    });

    let lsp_notify_schema = json!({
        "type": "object",
        "properties": {
            "method": {"type": "string", "description": "LSP notification method name."},
            "params": {"description": "Notification params forwarded verbatim."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["method"],
        "additionalProperties": true
    });

    let lsp_doc_only_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri"],
        "additionalProperties": false
    });

    let lsp_positions_array_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "positions": {
                "type": "array",
                "description": "List of zero-based positions.",
                "items": lsp_positional_schema
                    .get("properties").unwrap()
                    .get("position").unwrap()
                    .clone(),
                "minItems": 1
            },
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri", "positions"],
        "additionalProperties": false
    });

    let lsp_range_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "range": {
                "type": "object",
                "description": RANGE_DESC,
                "properties": {
                    "start": lsp_positional_schema
                        .get("properties").unwrap()
                        .get("position").unwrap()
                        .clone(),
                    "end": lsp_positional_schema
                        .get("properties").unwrap()
                        .get("position").unwrap()
                        .clone()
                },
                "required": ["start", "end"]
            },
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri", "range"],
        "additionalProperties": false
    });

    let range_property = lsp_range_schema
        .get("properties")
        .unwrap()
        .get("range")
        .unwrap()
        .clone();
    let position_property = lsp_positional_schema
        .get("properties")
        .unwrap()
        .get("position")
        .unwrap()
        .clone();

    let lsp_query_schema = json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "Query string passed to the language server."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["query"],
        "additionalProperties": false
    });

    let lsp_rename_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "position": position_property.clone(),
            "newName": {"type": "string", "description": "Replacement identifier."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri", "position", "newName"],
        "additionalProperties": false
    });

    let lsp_execute_command_schema = json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "description": "Command identifier exposed by the language server."},
            "arguments": {"type": "array", "description": "Arguments array forwarded to the LSP."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["command"],
        "additionalProperties": true
    });

    let lsp_item_resolve_schema = json!({
        "type": "object",
        "properties": {
            "item": {"description": "Original item returned from a previous LSP call."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["item"],
        "additionalProperties": true
    });

    let lsp_files_array_schema = json!({
        "type": "object",
        "properties": {
            "files": {"type": "array", "description": "Array of file operation descriptors as defined by the LSP."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["files"],
        "additionalProperties": false
    });

    let lsp_text_document_diagnostic_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string", "description": URI_DESC},
            "identifier": {"type": "string", "description": "Optional diagnostic identifier."},
            "previousResultId": {"type": "string", "description": "Opaque identifier from a prior diagnostic pull."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "required": ["uri"],
        "additionalProperties": false
    });

    let lsp_workspace_diagnostic_schema = json!({
        "type": "object",
        "properties": {
            "identifier": {"type": "string", "description": "Optional diagnostic identifier."},
            "previousResultIds": {"description": "Array of { uri, value } descriptors from previous responses."},
            "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
        },
        "additionalProperties": false
    });

    let mut tools = Vec::new();

    let positional_tools = [
        (
            "lsp_hover",
            "Retrieve hover documentation or type information at the cursor",
            "textDocument/hover",
            None,
        ),
        (
            "lsp_definition",
            "Navigate to the definition of the symbol at the given position",
            "textDocument/definition",
            Some("Responses may contain multiple locations; all are forwarded as returned."),
        ),
        (
            "lsp_type_definition",
            "Locate the type definition for the symbol under the cursor",
            "textDocument/typeDefinition",
            None,
        ),
        (
            "lsp_implementation",
            "List concrete implementations for an interface or trait",
            "textDocument/implementation",
            None,
        ),
        (
            "lsp_completion",
            "Request completion items at the cursor",
            "textDocument/completion",
            Some("Include an optional `context` to forward trigger information."),
        ),
        (
            "lsp_signature_help",
            "Show signature help for the call at the cursor",
            "textDocument/signatureHelp",
            Some("You may supply an optional `context` to preserve triggering metadata."),
        ),
        (
            "lsp_document_highlight",
            "Highlight related occurrences of the symbol at the cursor",
            "textDocument/documentHighlight",
            None,
        ),
        (
            "lsp_linked_editing_range",
            "Discover linked ranges that should edit together (for example HTML tags)",
            "textDocument/linkedEditingRange",
            None,
        ),
        (
            "lsp_moniker",
            "Fetch symbol moniker information (for cross-repository navigation)",
            "textDocument/moniker",
            None,
        ),
        (
            "lsp_prepare_rename",
            "Check whether the symbol can be renamed and return the editable span",
            "textDocument/prepareRename",
            Some("Invoke before `lsp_rename` to surface server-provided ranges."),
        ),
        (
            "lsp_declaration",
            "Jump to the declaration of the symbol at the cursor",
            "textDocument/declaration",
            None,
        ),
        (
            "lsp_call_hierarchy_prepare",
            "Prepare call hierarchy information for the symbol at the cursor",
            "textDocument/prepareCallHierarchy",
            Some("Use the returned item with incoming/outgoing call tools."),
        ),
        (
            "lsp_type_hierarchy_prepare",
            "Prepare type hierarchy information for the symbol at the cursor",
            "textDocument/prepareTypeHierarchy",
            Some("Use the returned item with type hierarchy subtype/supertype tools."),
        ),
    ];

    for (name, summary, method, extra) in positional_tools {
        let mut desc = format!(
            "{summary}. Forwards to LSP `{method}`. Provide `uri` (file:// or absolute path) and zero-based `position`. {SERVER_NOTE}",
        );
        if let Some(extra_text) = extra {
            desc.push(' ');
            desc.push_str(extra_text);
        }
        tools.push(Tool {
            name: name.to_string(),
            description: Some(desc),
            input_schema: lsp_positional_schema.clone(),
        });
    }

    tools.push(Tool {
        name: "lsp_references".to_string(),
        description: Some(format!(
            "Find references for the symbol at the cursor by calling LSP `textDocument/references`. Provide `uri`, zero-based `position`, and optionally set `includeDeclaration`. {SERVER_NOTE}"
        )),
        input_schema: lsp_references_schema,
    });

    tools.push(Tool {
        name: "lsp_selection_range".to_string(),
        description: Some(format!(
            "Expand or contract selection ranges suggested by the server via `textDocument/selectionRange`. Provide `uri` and at least one position. {SERVER_NOTE}"
        )),
        input_schema: lsp_positions_array_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_folding_range".to_string(),
        description: Some(format!(
            "Request foldable regions from the server via `textDocument/foldingRange`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_document_symbol".to_string(),
        description: Some(format!(
            "List symbols defined in a single document using LSP `textDocument/documentSymbol`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_workspace_symbol".to_string(),
        description: Some(format!(
            "Search the workspace for symbols matching a query via `workspace/symbol`. Supply a human-readable `query`. {SERVER_NOTE}"
        )),
        input_schema: lsp_query_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_workspace_symbol_resolve".to_string(),
        description: Some(format!(
            "Resolve additional data for a workspace symbol item returned by `lsp_workspace_symbol` using `workspaceSymbol/resolve`. Provide the original `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_rename".to_string(),
        description: Some(format!(
            "Rename a symbol across the workspace via `textDocument/rename`. Provide `uri`, zero-based `position`, and the replacement `newName`. {SERVER_NOTE}"
        )),
        input_schema: lsp_rename_schema,
    });

    tools.push(Tool {
        name: "lsp_code_action".to_string(),
        description: Some(format!(
            "Request code actions (fixes, refactors) via `textDocument/codeAction`. Provide `uri`, the affected `range`, and a `context` containing diagnostics. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "range": range_property.clone(),
                "context": {"description": "textDocument/codeAction context object (diagnostics, triggerKind, etc.)."},
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "range", "context"],
            "additionalProperties": false
        }),
    });

    tools.push(Tool {
        name: "lsp_code_action_resolve".to_string(),
        description: Some(format!(
            "Resolve a code action returned by `lsp_code_action` using `codeAction/resolve`. Provide the original `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_completion_item_resolve".to_string(),
        description: Some(format!(
            "Resolve additional details for a completion item returned by `lsp_completion` using `completionItem/resolve`. Provide the original completion `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_code_lens".to_string(),
        description: Some(format!(
            "Request code lenses (inline commands) via `textDocument/codeLens`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_code_lens_resolve".to_string(),
        description: Some(format!(
            "Resolve a code lens returned by `lsp_code_lens` via `codeLens/resolve`. Provide the original lens `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_document_link".to_string(),
        description: Some(format!(
            "Collect document links via `textDocument/documentLink`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_document_link_resolve".to_string(),
        description: Some(format!(
            "Resolve target information for a link returned by `lsp_document_link` using `documentLink/resolve`. Provide the original `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_document_color".to_string(),
        description: Some(format!(
            "List color references within a document via `textDocument/documentColor`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_color_presentation".to_string(),
        description: Some(format!(
            "Request alternative color presentations via `textDocument/colorPresentation`. Provide `uri`, the RGBA `color`, and the `range` covering the literal. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "color": {"description": "RGBA color object as defined by the LSP."},
                "range": range_property.clone(),
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "color", "range"],
            "additionalProperties": false
        }),
    });

    tools.push(Tool {
        name: "lsp_formatting".to_string(),
        description: Some(format!(
            "Format an entire document via `textDocument/formatting`. Provide `uri` and the LSP formatting `options`. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "options": {"type": "object", "description": "Formatting options (tabSize, insertSpaces, etc.)."},
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "options"],
            "additionalProperties": true
        }),
    });

    tools.push(Tool {
        name: "lsp_range_formatting".to_string(),
        description: Some(format!(
            "Format a portion of a document via `textDocument/rangeFormatting`. Provide `uri`, the target `range`, and formatting `options`. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "range": range_property.clone(),
                "options": {"type": "object", "description": "Formatting options."},
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "range", "options"],
            "additionalProperties": true
        }),
    });

    tools.push(Tool {
        name: "lsp_on_type_formatting".to_string(),
        description: Some(format!(
            "Request formatting edits triggered by typing a character via `textDocument/onTypeFormatting`. Provide `uri`, the cursor `position`, typed character `ch`, and formatting `options`. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "position": position_property.clone(),
                "ch": {"type": "string", "description": "Single character that triggered formatting."},
                "options": {"type": "object", "description": "Formatting options."},
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "position", "ch", "options"],
            "additionalProperties": true
        }),
    });

    tools.push(Tool {
        name: "lsp_inline_value".to_string(),
        description: Some(format!(
            "Compute inline values (debug views) for a range via `textDocument/inlineValue`. Provide `uri`, target `range`, and inline value `context`. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "range": range_property.clone(),
                "context": {"description": "Inline value context (see LSP spec)."},
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "range", "context"],
            "additionalProperties": true
        }),
    });

    tools.push(Tool {
        name: "lsp_inlay_hint".to_string(),
        description: Some(format!(
            "Request inlay hints for a range via `textDocument/inlayHint`. Provide `uri` and the target `range`. {SERVER_NOTE}"
        )),
        input_schema: lsp_range_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_inlay_hint_resolve".to_string(),
        description: Some(format!(
            "Resolve additional details for an inlay hint returned by `lsp_inlay_hint` via `inlayHint/resolve`. Provide the original hint `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_call_hierarchy_incoming_calls".to_string(),
        description: Some(format!(
            "Request incoming calls for an item produced by `lsp_call_hierarchy_prepare` using `callHierarchy/incomingCalls`. Provide the original hierarchy `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_call_hierarchy_outgoing_calls".to_string(),
        description: Some(format!(
            "Request outgoing calls for an item produced by `lsp_call_hierarchy_prepare` via `callHierarchy/outgoingCalls`. Provide the original hierarchy `item`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_type_hierarchy_supertypes".to_string(),
        description: Some(format!(
            "Fetch supertype information for a type-hierarchy item using `typeHierarchy/supertypes`. Provide the original item from `lsp_type_hierarchy_prepare`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_type_hierarchy_subtypes".to_string(),
        description: Some(format!(
            "Fetch subtype information for a type-hierarchy item using `typeHierarchy/subtypes`. Provide the original item from `lsp_type_hierarchy_prepare`. {SERVER_NOTE}"
        )),
        input_schema: lsp_item_resolve_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_semantic_tokens_full".to_string(),
        description: Some(format!(
            "Request full-document semantic tokens via `textDocument/semanticTokens/full`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_semantic_tokens_full_delta".to_string(),
        description: Some(format!(
            "Request semantic token deltas with respect to a previous result using `textDocument/semanticTokens/full/delta`. Provide `uri` and `previousResultId`. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "previousResultId": {"type": "string", "description": "Previous semantic tokens result identifier."},
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "previousResultId"],
            "additionalProperties": false
        }),
    });

    tools.push(Tool {
        name: "lsp_semantic_tokens_range".to_string(),
        description: Some(format!(
            "Request semantic tokens for a specific range via `textDocument/semanticTokens/range`. Provide `uri` and the `range`. {SERVER_NOTE}"
        )),
        input_schema: json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": URI_DESC},
                "range": range_property.clone(),
                "serverCommand": {"type": "string", "description": SERVER_CMD_DESC}
            },
            "required": ["uri", "range"],
            "additionalProperties": false
        }),
    });

    tools.push(Tool {
        name: "lsp_execute_command".to_string(),
        description: Some(format!(
            "Execute a workspace command exposed by the server via `workspace/executeCommand`. Provide the command identifier and optional `arguments` array. {SERVER_NOTE}"
        )),
        input_schema: lsp_execute_command_schema,
    });

    tools.push(Tool {
        name: "lsp_will_create_files".to_string(),
        description: Some(format!(
            "Request permission for workspace file creation by calling `workspace/willCreateFiles`. Provide the LSP `files` array describing the changes. {SERVER_NOTE}"
        )),
        input_schema: lsp_files_array_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_will_rename_files".to_string(),
        description: Some(format!(
            "Request permission for workspace file renames via `workspace/willRenameFiles`. Provide the LSP `files` array with rename descriptors. {SERVER_NOTE}"
        )),
        input_schema: lsp_files_array_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_will_delete_files".to_string(),
        description: Some(format!(
            "Request permission for workspace file deletions via `workspace/willDeleteFiles`. Provide the LSP `files` array describing deletions. {SERVER_NOTE}"
        )),
        input_schema: lsp_files_array_schema,
    });

    tools.push(Tool {
        name: "lsp_text_document_content".to_string(),
        description: Some(format!(
            "Resolve virtual content for a document via `workspace/textDocumentContent`. Provide the document `uri`. {SERVER_NOTE}"
        )),
        input_schema: lsp_doc_only_schema.clone(),
    });

    tools.push(Tool {
        name: "lsp_text_document_diagnostic".to_string(),
        description: Some(format!(
            "Pull diagnostics for a single document using `textDocument/diagnostic`. Provide `uri` and optionally carry `identifier`/`previousResultId` tokens. {SERVER_NOTE}"
        )),
        input_schema: lsp_text_document_diagnostic_schema,
    });

    tools.push(Tool {
        name: "lsp_workspace_diagnostic".to_string(),
        description: Some(format!(
            "Pull workspace diagnostics via `workspace/diagnostic`. Optionally include `identifier` and `previousResultIds` to maintain state. {SERVER_NOTE}"
        )),
        input_schema: lsp_workspace_diagnostic_schema,
    });

    tools.push(Tool {
        name: "lsp_call".to_string(),
        description: Some(format!(
            "Send a custom LSP request using an arbitrary `method` and `params`. Useful for experimenting with server features not yet modeled as dedicated tools. {SERVER_NOTE}"
        )),
        input_schema: lsp_call_schema,
    });

    tools.push(Tool {
        name: "lsp_notify".to_string(),
        description: Some(format!(
            "Send a custom LSP notification with an arbitrary `method` and `params`. No response is expected. {SERVER_NOTE}"
        )),
        input_schema: lsp_notify_schema,
    });

    tools
}

fn uri_from_object(map: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(Value::String(uri)) = map.get("uri") {
        return Some(uri.clone());
    }
    if let Some(Value::Object(td)) = map.get("textDocument") {
        if let Some(Value::String(uri)) = td.get("uri") {
            return Some(uri.clone());
        }
    }
    None
}

fn uri_from_params(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(uri) = uri_from_object(map) {
                return Some(uri);
            }
            if let Some(Value::Array(items)) = map.get("items") {
                for item in items {
                    if let Some(uri) = uri_from_params(item) {
                        return Some(uri);
                    }
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(uri) = uri_from_params(item) {
                    return Some(uri);
                }
            }
            None
        }
        _ => None,
    }
}

fn language_from_did_open(params: &Value) -> Option<String> {
    params
        .get("textDocument")
        .and_then(|td| td.get("languageId"))
        .and_then(|lang| lang.as_str())
        .map(|s| s.to_ascii_lowercase())
}

fn parse_params_value(raw: Value) -> Value {
    match raw {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        other => other,
    }
}

fn build_error_data(
    tool: &str,
    method: Option<&str>,
    uri: Option<&str>,
    server_cmd: Option<&str>,
    err: &anyhow::Error,
) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("tool".into(), Value::String(tool.to_string()));
    if let Some(method) = method {
        map.insert("method".into(), Value::String(method.to_string()));
    }
    if let Some(uri) = uri {
        map.insert("uri".into(), Value::String(uri.to_string()));
    }
    if let Some(cmd) = server_cmd {
        map.insert("serverCommand".into(), Value::String(cmd.to_string()));
    }
    map.insert("details".into(), Value::String(format!("{:#}", err)));
    Value::Object(map)
}

fn format_tool_error_message(tool: &str, method: Option<&str>, err: &anyhow::Error) -> String {
    match method {
        Some(method) => format!("LSP tool '{tool}' invoking '{method}' failed: {:#}", err),
        None => format!("Tool '{tool}' failed: {:#}", err),
    }
}

pub(crate) async fn handle_tools_call(params: Option<Value>) -> JsonRpcResponse {
    let err_resp = |code: i64, msg: &str| JsonRpcResponse::error(ErrorObject::new(code, msg, None));
    let params = match params {
        Some(Value::Object(map)) => map,
        _ => return err_resp(-32602, "Invalid params: expected object"),
    };
    let tool_name = match params.get("name") {
        Some(Value::String(s)) => s.clone(),
        _ => return err_resp(-32602, "Missing 'name' in params"),
    };
    let tool_name = match tool_name.as_str() {
        "hover" => "lsp_hover".to_string(),
        "definition" => "lsp_definition".to_string(),
        "type_definition" => "lsp_type_definition".to_string(),
        "implementation" => "lsp_implementation".to_string(),
        "references" => "lsp_references".to_string(),
        "completion" => "lsp_completion".to_string(),
        "call" => "lsp_call".to_string(),
        other => other.to_string(),
    };

    let arguments_value = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match tool_name.as_str() {
        "lsp_call" => {
            let mut args_map = match arguments_value.as_object() {
                Some(m) => m.clone(),
                None => return err_resp(-32602, "Invalid arguments: expected object"),
            };
            let server_cmd = args_map
                .remove("serverCommand")
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            return handle_lsp_call(args_map, server_cmd).await;
        }
        "lsp_notify" => {
            let mut args_map = match arguments_value.as_object() {
                Some(m) => m.clone(),
                None => Map::new(),
            };
            let server_cmd = args_map
                .remove("serverCommand")
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            return handle_lsp_notify(args_map, server_cmd).await;
        }
        _ => {}
    }

    let mut args_map = match arguments_value.as_object() {
        Some(m) => m.clone(),
        None => return err_resp(-32602, "Invalid arguments: expected object"),
    };

    let server_cmd = args_map
        .remove("serverCommand")
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    if !tool_name.starts_with("lsp_") {
        return JsonRpcResponse::error(unsupported_tool_error(&tool_name));
    }

    let invocation = match build_lsp_invocation(&tool_name, &args_map, server_cmd.clone()) {
        Ok(inv) => inv,
        Err(err) => return JsonRpcResponse::error(err),
    };

    let method = invocation.method;
    let params_for_request = invocation.params.clone();
    let server_cmd_for_request = invocation.server_cmd.clone();
    let uri_hint_for_request = invocation.uri_hint.clone();

    let params_for_closure = params_for_request.clone();
    let server_cmd_for_closure = server_cmd_for_request.clone();
    let uri_hint_for_closure = uri_hint_for_request.clone();

    let result = task::spawn_blocking(move || {
        with_language_pool(|pool| {
            let cmd = pool.resolve_command(
                server_cmd_for_closure.as_deref(),
                uri_hint_for_closure.as_deref(),
                None,
            )?;
            let need_open = uri_hint_for_closure
                .as_deref()
                .map(|uri| !pool.has_document(uri))
                .unwrap_or(false);
            let open_params = if need_open {
                if let Some(uri) = uri_hint_for_closure.as_ref() {
                    Some(pool.build_did_open_params(uri, None)?)
                } else {
                    None
                }
            } else {
                None
            };
            let outcome = pool.with_manager(&cmd, |lsm| {
                if let Some(payload) = open_params.as_ref() {
                    lsm.notify("textDocument/didOpen", payload.clone(), Some(cmd.as_str()))?;
                }
                lsm.request(method, params_for_closure.clone(), Some(cmd.as_str()))
            })?;
            if need_open {
                if let Some(uri) = uri_hint_for_closure.as_ref() {
                    pool.associate_document(uri, &cmd);
                }
            }
            Ok(outcome)
        })
    })
    .await;

    match result {
        Ok(Ok(value)) => JsonRpcResponse::result(json!({
            "tool": tool_name,
            "status": "ok",
            "result": value
        })),
        Ok(Err(e)) => {
            let data = build_error_data(
                &tool_name,
                Some(method),
                uri_hint_for_request.as_deref(),
                server_cmd_for_request.as_deref(),
                &e,
            );
            if let Ok(json_data) = serde_json::to_string(&data) {
                eprintln!("mcp-lsp: tool '{}' failed -> {}", tool_name, json_data);
            }
            let message = format_tool_error_message(&tool_name, Some(method), &e);
            JsonRpcResponse::error(ErrorObject::new(-32050, &message, Some(data)))
        }
        Err(join_err) => {
            let err = anyhow::Error::new(join_err);
            let data = build_error_data(
                &tool_name,
                Some(method),
                uri_hint_for_request.as_deref(),
                server_cmd_for_request.as_deref(),
                &err,
            );
            if let Ok(json_data) = serde_json::to_string(&data) {
                eprintln!("mcp-lsp: tool '{}' failed -> {}", tool_name, json_data);
            }
            let message = format_tool_error_message(&tool_name, Some(method), &err);
            JsonRpcResponse::error(ErrorObject::new(-32050, &message, Some(data)))
        }
    }
}

impl Drop for LanguageServerPool {
    fn drop(&mut self) {
        if let Err(err) = self.shutdown_all() {
            eprintln!("mcp-lsp: failed to shut down language servers: {err:#}");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    mcp::run().await
}
