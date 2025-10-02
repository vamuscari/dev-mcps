use anyhow::Result;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ErrorData, ListToolsResult, PaginatedRequestParam,
        ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer, ServiceExt},
    ServerHandler,
};
use serde_json::json;
use tokio::task;
use std::sync::{Arc, Mutex};

use crate::{handle_structured_call, DapAdapterManager};
use crate::list_tools_impl;

fn call_tool_impl(request: CallToolRequestParam, manager: &mut DapAdapterManager) -> Result<CallToolResult, ErrorData> {
    let CallToolRequestParam { name, arguments } = request;
    if !name.starts_with("dap_") {
        return Err(ErrorData::method_not_found::<
            rmcp::model::CallToolRequestMethod,
        >());
    }
    let args = arguments.unwrap_or_default();
    let adapter_cmd = args.get("adapterCommand").and_then(|v| v.as_str());

    match name.as_ref() {
        "dap_initialize" => {
            let res = manager
                .capabilities(adapter_cmd)
                .map_err(|e| ErrorData::internal_error(format!("dap init error: {e}"), None))?;
            Ok(CallToolResult::structured(json!({
                "tool": "dap_initialize",
                "status": "ok",
                "capabilities": res
            })))
        }
        "dap_call" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ErrorData::invalid_params("Missing required field: command", None)
                })?;
            let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let result = manager
                .request(command, arguments, adapter_cmd)
                .map_err(|e| ErrorData::internal_error(format!("dap error: {e}"), None))?;
            Ok(CallToolResult::structured(json!({
                "tool": "dap_call",
                "status": "ok",
                "result": result
            })))
        }
        other => handle_structured_call(other, &args, adapter_cmd, manager),
    }
}

fn server_info() -> ServerInfo {
    ServerInfo {
        instructions: Some(
            "Bridge Debug Adapter Protocol tooling for Codex MCP clients.".to_string(),
        ),
        capabilities: ServerCapabilities::builder().enable_tools().build(),
        ..ServerInfo::default()
    }
}

#[derive(Clone)]
struct CodexDapServer {
    manager: Arc<Mutex<DapAdapterManager>>,
}

impl ServerHandler for CodexDapServer {
    fn get_info(&self) -> ServerInfo {
        server_info()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let manager = self.manager.clone();
        let tools = task::spawn_blocking(move || {
            let mut guard = manager.lock().unwrap();
            list_tools_impl(&mut guard)
        })
            .await
            .map_err(|e| ErrorData::internal_error(format!("list tools task panicked: {e}"), None))??;
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let manager = self.manager.clone();
        task::spawn_blocking(move || {
            let mut guard = manager.lock().unwrap();
            call_tool_impl(request, &mut guard)
        })
            .await
            .map_err(|e| ErrorData::internal_error(format!("call tool task panicked: {e}"), None))?
    }
}

pub async fn run() -> Result<()> {
    let server = CodexDapServer { manager: Arc::new(Mutex::new(DapAdapterManager::new())) };
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

// tests removed by request
