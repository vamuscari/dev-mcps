mod lsif;

use anyhow::Result;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ErrorData, JsonObject, ListToolsResult,
        PaginatedRequestParam, ServerCapabilities, ServerInfo, Tool as McpTool,
    },
    service::{RequestContext, RoleServer, ServiceExt},
    ServerHandler,
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Default)]
struct CodexLsifServer;

impl ServerHandler for CodexLsifServer {
    fn get_info(&self) -> ServerInfo {
        server_info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        call_tool_impl(request)
    }
}

fn server_info() -> ServerInfo {
    ServerInfo {
        instructions: Some(
            "Serve LSIF-backed code intelligence queries (definitions, references, hover)."
                .to_string(),
        ),
        capabilities: ServerCapabilities::builder().enable_tools().build(),
        ..ServerInfo::default()
    }
}

fn schema(value: Value) -> Arc<JsonObject> {
    Arc::new(
        value
            .as_object()
            .cloned()
            .expect("tool schema must be an object"),
    )
}

fn tools() -> Vec<McpTool> {
    let positional = json!({
        "type": "object",
        "properties": {
            "uri": {
                "type": "string",
                "description": "Document URI (file:// or path)"
            },
            "position": {
                "type": "object",
                "properties": {
                    "line": {"type": "integer", "minimum": 0},
                    "character": {"type": "integer", "minimum": 0}
                },
                "required": ["line", "character"]
            }
        },
        "required": ["uri", "position"]
    });

    let position_schema = positional
        .get("properties")
        .and_then(|p| p.get("position"))
        .cloned()
        .expect("position schema");

    let references_schema = json!({
        "type": "object",
        "properties": {
            "uri": {"type": "string"},
            "position": position_schema,
            "includeDeclarations": {"type": "boolean", "default": false}
        },
        "required": ["uri", "position"]
    });

    vec![
        McpTool::new(
            "lsif_load",
            "Load LSIF JSONL from path",
            schema(json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            })),
        ),
        McpTool::new(
            "lsif_definition",
            "Definition via LSIF index",
            schema(positional.clone()),
        ),
        McpTool::new(
            "lsif_references",
            "References via LSIF index",
            schema(references_schema),
        ),
        McpTool::new(
            "lsif_hover",
            "Hover via LSIF index (if available)",
            schema(positional),
        ),
    ]
}

fn require_string(args: &JsonObject, key: &str) -> Result<String, ErrorData> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ErrorData::invalid_params(format!("Missing required field: {key}"), None))
}

fn require_position(args: &JsonObject) -> Result<(u32, u32), ErrorData> {
    let position = args
        .get("position")
        .and_then(|v| v.as_object())
        .ok_or_else(|| ErrorData::invalid_params("Missing required field: position", None))?;
    let line = position
        .get("line")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ErrorData::invalid_params("Field 'line' must be an integer", None))?;
    let character = position
        .get("character")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ErrorData::invalid_params("Field 'character' must be an integer", None))?;
    Ok((line as u32, character as u32))
}

fn call_tool_impl(request: CallToolRequestParam) -> Result<CallToolResult, ErrorData> {
    let CallToolRequestParam { name, arguments } = request;
    let args = arguments.unwrap_or_default();
    match name.as_ref() {
        "lsif_load" => {
            let path = require_string(&args, "path")?;
            lsif::load_from_path(&path).map_err(|err| to_internal_error("lsif load error", err))?;
            Ok(CallToolResult::structured(json!({
                "tool": "lsif_load",
                "status": "ok"
            })))
        }
        "lsif_definition" => {
            let uri = require_string(&args, "uri")?;
            let (line, character) = require_position(&args)?;
            let result = lsif::query_definition(&uri, line, character)
                .map_err(|err| to_internal_error("lsif definition error", err))?;
            Ok(CallToolResult::structured(result))
        }
        "lsif_references" => {
            let uri = require_string(&args, "uri")?;
            let (line, character) = require_position(&args)?;
            let include = args
                .get("includeDeclarations")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let result = lsif::query_references(&uri, line, character, include)
                .map_err(|err| to_internal_error("lsif references error", err))?;
            Ok(CallToolResult::structured(result))
        }
        "lsif_hover" => {
            let uri = require_string(&args, "uri")?;
            let (line, character) = require_position(&args)?;
            let result = lsif::query_hover(&uri, line, character)
                .map_err(|err| to_internal_error("lsif hover error", err))?;
            Ok(CallToolResult::structured(result))
        }
        _ => Err(ErrorData::invalid_params(
            format!("Unsupported lsif tool: {}", name),
            Some(json!({"tool": name})),
        )),
    }
}

fn to_internal_error(context: &str, err: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(
        format!("{context}: {err}"),
        Some(json!({"details": format!("{:#}", err)})),
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let server = CodexLsifServer;
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;

    #[test]
    fn tools_list_has_schemas() {
        let items = tools();
        assert!(!items.is_empty());
        assert!(items.iter().all(|tool| !tool.input_schema.is_empty()));
    }

    #[test]
    fn lsif_load_requires_path() {
        let req = CallToolRequestParam {
            name: "lsif_load".into(),
            arguments: Some(JsonObject::default()),
        };
        let err = call_tool_impl(req).expect_err("expected invalid params");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
    }
}
