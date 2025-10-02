mod da;
mod mcp;

use anyhow::Result;
use da::DapAdapterManager;
use rmcp::model::{CallToolResult, ErrorData, JsonObject, Tool as McpTool};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

fn schema(value: Value) -> Arc<JsonObject> {
    Arc::new(
        value
            .as_object()
            .cloned()
            .expect("tool schema must be an object"),
    )
}

fn tools() -> Vec<McpTool> {
    let dap_call_schema = json!({
        "type": "object",
        "properties": {
            "command": {"type": "string"},
            "arguments": {"description": "Arbitrary DAP arguments"},
            "adapterCommand": {"type": "string"}
        },
        "required": ["command"]
    });
    let adapter_only_schema = json!({
        "type": "object",
        "properties": {"adapterCommand": {"type": "string"}},
        "additionalProperties": true
    });
    let launch_attach_schema = json!({
        "type": "object",
        "properties": {"arguments": {}, "adapterCommand": {"type": "string"}},
        "required": ["arguments"]
    });
    let set_breakpoints_schema = json!({
        "type": "object",
        "properties": {
            "source": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
            "breakpoints": {"type": "array"},
            "lines": {"type": "array", "items": {"type": "integer", "minimum": 1}},
            "sourceModified": {"type": "boolean"},
            "adapterCommand": {"type": "string"}
        },
        "required": ["source"]
    });
    let thread_id_schema = json!({
        "type": "object",
        "properties": {"threadId": {"type": "integer", "minimum": 1}, "adapterCommand": {"type": "string"}},
        "required": ["threadId"]
    });
    let stack_trace_schema = json!({
        "type": "object",
        "properties": {"threadId": {"type": "integer", "minimum": 1}, "startFrame": {"type": "integer"}, "levels": {"type": "integer"}, "adapterCommand": {"type": "string"}},
        "required": ["threadId"]
    });
    let scopes_schema = json!({
        "type": "object",
        "properties": {"frameId": {"type": "integer", "minimum": 1}, "adapterCommand": {"type": "string"}},
        "required": ["frameId"]
    });
    let variables_schema = json!({
        "type": "object",
        "properties": {"variablesReference": {"type": "integer", "minimum": 1}, "adapterCommand": {"type": "string"}},
        "required": ["variablesReference"]
    });
    let evaluate_schema = json!({
        "type": "object",
        "properties": {"expression": {"type": "string"}, "frameId": {"type": "integer"}, "context": {"type": "string"}, "adapterCommand": {"type": "string"}},
        "required": ["expression"]
    });
    let disconnect_schema = json!({
        "type": "object",
        "properties": {"terminateDebuggee": {"type": "boolean"}, "restart": {"type": "boolean"}, "adapterCommand": {"type": "string"}}
    });

    vec![
        McpTool::new(
            "dap_initialize",
            "Start adapter and report capabilities",
            schema(adapter_only_schema.clone()),
        ),
        McpTool::new("dap_call", "DAP custom call", schema(dap_call_schema)),
        McpTool::new(
            "dap_launch",
            "DAP launch",
            schema(launch_attach_schema.clone()),
        ),
        McpTool::new(
            "dap_attach",
            "DAP attach",
            schema(launch_attach_schema.clone()),
        ),
        McpTool::new(
            "dap_set_breakpoints",
            "Set breakpoints for a source",
            schema(set_breakpoints_schema),
        ),
        McpTool::new(
            "dap_configuration_done",
            "Configuration done",
            schema(adapter_only_schema.clone()),
        ),
        McpTool::new(
            "dap_continue",
            "Continue execution",
            schema(thread_id_schema.clone()),
        ),
        McpTool::new("dap_next", "Step over", schema(thread_id_schema.clone())),
        McpTool::new("dap_step_in", "Step in", schema(thread_id_schema.clone())),
        McpTool::new("dap_step_out", "Step out", schema(thread_id_schema.clone())),
        McpTool::new(
            "dap_threads",
            "List threads",
            schema(adapter_only_schema.clone()),
        ),
        McpTool::new(
            "dap_stack_trace",
            "Get stack trace",
            schema(stack_trace_schema),
        ),
        McpTool::new("dap_scopes", "Get scopes for frame", schema(scopes_schema)),
        McpTool::new(
            "dap_variables",
            "Get variables for reference",
            schema(variables_schema),
        ),
        McpTool::new(
            "dap_evaluate",
            "Evaluate expression",
            schema(evaluate_schema),
        ),
        McpTool::new(
            "dap_disconnect",
            "Disconnect debugger",
            schema(disconnect_schema),
        ),
    ]
}

fn filter_tools_by_capabilities(mut all: Vec<McpTool>, caps: Option<Value>) -> Vec<McpTool> {
    let Some(caps) = caps else {
        return all;
    };
    let obj = caps.as_object().cloned().unwrap_or_default();
    let mut allowed = HashSet::<String>::new();
    for name in [
        "dap_initialize",
        "dap_call",
        "dap_launch",
        "dap_attach",
        "dap_set_breakpoints",
        "dap_continue",
        "dap_next",
        "dap_step_in",
        "dap_step_out",
        "dap_threads",
        "dap_stack_trace",
        "dap_scopes",
        "dap_variables",
        "dap_evaluate",
        "dap_disconnect",
    ] {
        allowed.insert(name.to_string());
    }
    if obj
        .get("supportsConfigurationDoneRequest")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        allowed.insert("dap_configuration_done".to_string());
    }

    all.retain(|tool| allowed.contains(tool.name.as_ref()));
    all
}

fn list_tools_impl(manager: &mut DapAdapterManager) -> Result<Vec<McpTool>, ErrorData> {
    let all = tools();
    let caps = manager
        .capabilities(None)
        .map_err(|e| ErrorData::internal_error(format!("dap init error: {e}"), None))?;
    Ok(filter_tools_by_capabilities(all, caps))
}

fn handle_structured_call(
    tool: &str,
    args: &JsonObject,
    adapter_cmd: Option<&str>,
    manager: &mut DapAdapterManager,
) -> Result<CallToolResult, ErrorData> {
    let (command, payload) = match tool {
        "dap_launch" | "dap_attach" => {
            let arguments = args.get("arguments").cloned().ok_or_else(|| {
                ErrorData::invalid_params("Missing required field: arguments", None)
            })?;
            let cmd = if tool == "dap_launch" {
                "launch"
            } else {
                "attach"
            };
            (cmd, arguments)
        }
        "dap_set_breakpoints" => {
            let source = args
                .get("source")
                .cloned()
                .ok_or_else(|| ErrorData::invalid_params("Missing required field: source", None))?;
            let mut breakpoints = args.get("breakpoints").cloned();
            if breakpoints.is_none() {
                if let Some(lines) = args.get("lines").and_then(|v| v.as_array()) {
                    let values: Vec<Value> = lines
                        .iter()
                        .filter_map(|v| v.as_i64())
                        .map(|line| json!({"line": line}))
                        .collect();
                    breakpoints = Some(json!(values));
                }
            }
            let mut obj =
                json!({"source": source, "breakpoints": breakpoints.unwrap_or_else(|| json!([]))});
            if let Some(sm) = args.get("sourceModified").cloned() {
                obj.as_object_mut()
                    .unwrap()
                    .insert("sourceModified".into(), sm);
            }
            ("setBreakpoints", obj)
        }
        "dap_configuration_done" => ("configurationDone", json!({})),
        "dap_continue" => {
            let thread_id = require_i64(args, "threadId")?;
            ("continue", json!({"threadId": thread_id}))
        }
        "dap_next" => {
            let thread_id = require_i64(args, "threadId")?;
            ("next", json!({"threadId": thread_id}))
        }
        "dap_step_in" => {
            let thread_id = require_i64(args, "threadId")?;
            ("stepIn", json!({"threadId": thread_id}))
        }
        "dap_step_out" => {
            let thread_id = require_i64(args, "threadId")?;
            ("stepOut", json!({"threadId": thread_id}))
        }
        "dap_threads" => ("threads", json!({})),
        "dap_stack_trace" => {
            let thread_id = require_i64(args, "threadId")?;
            let mut payload = json!({"threadId": thread_id});
            if let Some(sf) = args.get("startFrame").cloned() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("startFrame".into(), sf);
            }
            if let Some(levels) = args.get("levels").cloned() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("levels".into(), levels);
            }
            ("stackTrace", payload)
        }
        "dap_scopes" => {
            let frame_id = require_i64(args, "frameId")?;
            ("scopes", json!({"frameId": frame_id}))
        }
        "dap_variables" => {
            let vr = require_i64(args, "variablesReference")?;
            ("variables", json!({"variablesReference": vr}))
        }
        "dap_evaluate" => {
            let expression = args
                .get("expression")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ErrorData::invalid_params("Missing required field: expression", None)
                })?;
            let mut payload = json!({"expression": expression});
            if let Some(fid) = args.get("frameId").cloned() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("frameId".into(), fid);
            }
            if let Some(ctx) = args.get("context").cloned() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("context".into(), ctx);
            }
            ("evaluate", payload)
        }
        "dap_disconnect" => {
            let mut payload = json!({});
            if let Some(td) = args.get("terminateDebuggee").cloned() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("terminateDebuggee".into(), td);
            }
            if let Some(restart) = args.get("restart").cloned() {
                payload
                    .as_object_mut()
                    .unwrap()
                    .insert("restart".into(), restart);
            }
            ("disconnect", payload)
        }
        _ => {
            return Err(ErrorData::invalid_params(
                format!("Unsupported dap tool: {tool}"),
                Some(json!({"tool": tool})),
            ));
        }
    };

    let result = manager
        .request(command, payload, adapter_cmd)
        .map_err(|e| ErrorData::internal_error(format!("dap error: {e}"), None))?;
    Ok(CallToolResult::structured(json!({
        "tool": tool,
        "status": "ok",
        "result": result
    })))
}

fn require_i64(args: &JsonObject, key: &str) -> Result<i64, ErrorData> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| ErrorData::invalid_params(format!("Missing required field: {key}"), None))
}

#[tokio::main]
async fn main() -> Result<()> {
    mcp::run().await
}
