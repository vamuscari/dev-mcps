use std::sync::Arc;

use anyhow::{anyhow, Result};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, LoggingLevel, LoggingMessageNotification, LoggingMessageNotificationParam, ServerCapabilities, ServerInfo},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::codex;
use once_cell::sync::OnceCell;

// Upstream peer handle so background tasks (codex clients) can send notifications.
static UPSTREAM_PEER: OnceCell<rmcp::service::ClientSink> = OnceCell::new();

pub fn set_upstream_peer(peer: rmcp::service::ClientSink) {
    let _ = UPSTREAM_PEER.set(peer);
}

/// Orchestrator MCP server state and handlers.
#[derive(Clone)]
pub struct Orchestrator {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

#[derive(Default)]
struct Inner {
    manager: codex::Manager,
}

impl Orchestrator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::default()),
            tool_router: Self::tool_router(),
        }
    }

    fn normalize_params(params: serde_json::Value) -> serde_json::Value {
        match params {
            serde_json::Value::String(ref s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    serde_json::json!({})
                } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    v
                } else {
                    // Fallback: wrap as a simple text item payload for agents that accept it.
                    serde_json::json!({ "text": s })
                }
            }
            _ => params,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SpawnAgentArgs {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SpawnAgentResult {
    #[serde(rename = "agentId")]
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct ListAgentsArgs {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListAgentsResult {
    #[serde(rename = "agentIds")]
    pub agent_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct KillAgentArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct KillAgentResult {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct NewConversationArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendUserMessageArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendUserTurnArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct InterruptArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecisionArgs {
    /// Composite key identifying a pending approval: "<agentId>:<requestId>"
    pub key: String,
    /// "allow" or "deny"
    pub decision: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct ListApprovalsArgs {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListApprovalsResult {
    pub keys: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListConversationsArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResumeConversationArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveConversationArgs {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetConversationEventsArgs {
    #[serde(rename = "rolloutPath")]
    pub rollout_path: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[tool_router]
impl Orchestrator {
    #[tool(description = "Start a new Codex agent process (subprocess) that can manage multiple conversations. Each agent is an independent Codex MCP server.\n\nArguments:\n- id (optional): Custom identifier for the agent. Auto-generated if not provided.\n- cwd (optional): Working directory for the agent. Defaults to current directory.\n\nReturns: { agentId: string }\n\nExample: spawn_agent({ id: \"my-agent\", cwd: \"/path/to/project\" })")]
    pub async fn spawn_agent(
        &self,
        Parameters(SpawnAgentArgs { id, cwd }): Parameters<SpawnAgentArgs>,
    ) -> Result<CallToolResult, McpError> {
        let agent_id = self
            .inner
            .manager
            .spawn_agent(id, cwd.map(Into::into))
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let value = serde_json::to_value(SpawnAgentResult { agent_id })
            .unwrap_or_else(|_| serde_json::json!({"ok": true}));
        Ok(CallToolResult::success(vec![Content::text(value.to_string())]))
    }

    #[tool(description = "List all currently running Codex agents managed by this orchestrator.\n\nArguments: None\n\nReturns: { agentIds: string[] } - Array of agent identifiers\n\nExample: list_agents() → { \"agentIds\": [\"agent-1\", \"agent-2\"] }")]
    pub async fn list_agents(
        &self,
        _params: Parameters<ListAgentsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let agent_ids = self.inner.manager.list_agents().await;
        let value = serde_json::to_value(ListAgentsResult { agent_ids })
            .unwrap_or_else(|_| serde_json::json!({"agentIds": []}));
        Ok(CallToolResult::success(vec![Content::text(value.to_string())]))
    }

    #[tool(description = "Terminate a Codex agent process and clean up its resources. All active conversations on this agent will be stopped.\n\nArguments:\n- agentId (required): Identifier of the agent to terminate\n\nReturns: { ok: true }\n\nExample: kill_agent({ agentId: \"my-agent\" })")]
    pub async fn kill_agent(
        &self,
        Parameters(KillAgentArgs { agent_id }): Parameters<KillAgentArgs>,
    ) -> Result<CallToolResult, McpError> {
        self
            .inner
            .manager
            .kill_agent(&agent_id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let value = serde_json::to_value(KillAgentResult {})
            .unwrap_or_else(|_| serde_json::json!({"ok": true}));
        Ok(CallToolResult::success(vec![Content::text(value.to_string())]))
    }

    #[tool(description = "Start a new conversation with a Codex agent. Creates a new conversation context that can track multiple messages.\n\nArguments:\n- agentId (required): Identifier of the agent to use\n- params (optional): Configuration object\n  - prompt/topic/message (any works): Initial conversation prompt\n  - Other Codex-specific parameters as needed\n\nReturns: { conversationId: string, ... } - Conversation metadata including unique ID\n\nExample: new_conversation({ agentId: \"my-agent\", params: { prompt: \"Review the codebase\" } })")]
    pub async fn new_conversation(
        &self,
        Parameters(NewConversationArgs { agent_id, params }): Parameters<NewConversationArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Accept flexible shapes; forward raw params to Codex.
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .new_conversation(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "Send a message to an existing Codex conversation. Simpler than send_user_turn for basic message exchange.\n\nArguments:\n- agentId (required): Identifier of the agent\n- params (required): Message parameters\n  - conversationId (required): ID of the conversation\n  - message/prompt (either works): The message text to send\n\nReturns: Response from Codex agent\n\nExample: send_user_message({ agentId: \"my-agent\", params: { conversationId: \"c1\", message: \"What's next?\" } })")]
    pub async fn send_user_message(
        &self,
        Parameters(SendUserMessageArgs { agent_id, params }): Parameters<SendUserMessageArgs>,
    ) -> Result<CallToolResult, McpError> {
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .send_user_message(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "Send a user turn to a Codex conversation with automatic defaults for required fields. This is the recommended way to send messages.\n\nArguments:\n- agentId (required): Identifier of the agent\n- params (flexible): Can be a string, or an object with:\n  - conversationId (optional if last conversation exists): ID of the conversation\n  - text (optional if items provided): Message text - automatically converted to items format\n  - items (optional if text provided): Pre-formatted message items\n  - cwd (auto-filled): Working directory (defaults to current dir)\n  - approvalPolicy (auto-filled): Approval mode (defaults to \"never\")\n  - sandboxPolicy (auto-filled): Sandbox settings (defaults to read-only)\n  - model (auto-filled): AI model (defaults to \"gpt-4\")\n  - summary (auto-filled): Summary mode (defaults to \"auto\")\n\nReturns: Response from Codex agent\n\nExample: send_user_turn({ agentId: \"my-agent\", params: \"Hello!\" })\nExample: send_user_turn({ agentId: \"my-agent\", params: { conversationId: \"c1\", text: \"Continue\" } })")]
    pub async fn send_user_turn(
        &self,
        Parameters(SendUserTurnArgs { agent_id, params }): Parameters<SendUserTurnArgs>,
    ) -> Result<CallToolResult, McpError> {
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .send_user_turn(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "Interrupt an in-progress Codex conversation, stopping any ongoing agent processing.\n\nArguments:\n- agentId (required): Identifier of the agent\n- params (optional): Interrupt parameters\n  - conversationId (required): ID of the conversation to interrupt\n\nReturns: Confirmation from Codex agent\n\nNote: Not all Codex versions support interruption. Check agent capabilities.\n\nExample: interrupt({ agentId: \"my-agent\", params: { conversationId: \"c1\" } })")]
    pub async fn interrupt(
        &self,
        Parameters(InterruptArgs { agent_id, params }): Parameters<InterruptArgs>,
    ) -> Result<CallToolResult, McpError> {
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .interrupt(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "List all pending approval requests from Codex agents waiting for user decisions.\n\nArguments: None\n\nReturns: { keys: string[] } - Array of approval keys in format \"agentId:requestId\"\n\nNote: Approvals auto-deny after 60 seconds if not decided.\n\nExample: list_pending_approvals() → { \"keys\": [\"agent-1:42\", \"agent-2:7\"] }")]
    pub async fn list_pending_approvals(
        &self,
        _params: Parameters<ListApprovalsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let keys = self.inner.manager.list_pending_approvals().await;
        let value = serde_json::to_value(ListApprovalsResult { keys })
            .unwrap_or_else(|_| serde_json::json!({"keys": []}));
        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Resolve a pending Codex approval request by allowing or denying it.\n\nArguments:\n- key (required): Approval key in format \"agentId:requestId\" (from list_pending_approvals)\n- decision (required): \"allow\" to approve, \"deny\" to reject\n\nReturns: { ok: true } if decision was applied\n\nNote: Invalid keys or expired approvals will return an error.\n\nExample: decide_approval({ key: \"agent-1:42\", decision: \"allow\" })")]
    pub async fn decide_approval(
        &self,
        Parameters(ApprovalDecisionArgs { key, decision }): Parameters<ApprovalDecisionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let ok = self
            .inner
            .manager
            .decide_approval(&key, decision)
            .await
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let value = serde_json::json!({"ok": ok});
        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "List all recorded conversations (rollouts) for a Codex agent with optional pagination.\n\nArguments:\n- agentId (required): Identifier of the agent\n- params (optional): Pagination parameters\n  - pageSize (optional): Number of items per page (default: 10)\n  - cursor (optional): Pagination cursor from previous response\n\nReturns: { items: [...], nextCursor?: string }\n  Each item contains: { conversationId, path, preview, timestamp }\n\nExample: list_conversations({ agentId: \"my-agent\", params: { pageSize: 20 } })")]
    pub async fn list_conversations(
        &self,
        Parameters(ListConversationsArgs { agent_id, params }): Parameters<ListConversationsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .list_conversations(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "Resume a previously recorded Codex conversation from its rollout file, optionally overriding parameters.\n\nArguments:\n- agentId (required): Identifier of the agent\n- params (required): Resume parameters\n  - path (required): Full path to the rollout file (.jsonl)\n  - overrides (optional): Override conversation settings (model, cwd, etc.)\n\nReturns: { conversationId, model, initialMessages?: [...] } - Restored conversation metadata\n\nExample: resume_conversation({ agentId: \"my-agent\", params: { path: \"/path/to/rollout.jsonl\" } })")]
    pub async fn resume_conversation(
        &self,
        Parameters(ResumeConversationArgs { agent_id, params }): Parameters<ResumeConversationArgs>,
    ) -> Result<CallToolResult, McpError> {
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .resume_conversation(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "Archive a Codex conversation, marking it as finished and freeing up agent resources.\n\nArguments:\n- agentId (required): Identifier of the agent\n- params (required): Archive parameters\n  - conversationId (required): ID of the conversation to archive\n\nReturns: { ok: true }\n\nNote: Archived conversations remain in rollout files and can be resumed later.\n\nExample: archive_conversation({ agentId: \"my-agent\", params: { conversationId: \"c1\" } })")]
    pub async fn archive_conversation(
        &self,
        Parameters(ArchiveConversationArgs { agent_id, params }): Parameters<ArchiveConversationArgs>,
    ) -> Result<CallToolResult, McpError> {
        let params = Self::normalize_params(params);
        let res = self
            .inner
            .manager
            .archive_conversation(&agent_id, params)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::structured(res))
    }

    #[tool(description = "Read events from a Codex conversation rollout file. Returns the last N events from the rollout.\n\nArguments:\n- rolloutPath (required): Full path to the rollout file (.jsonl)\n- limit (optional): Maximum number of events to return (default: 50)\n\nReturns: { events: [...] } - Array of events from the rollout file, most recent last\n\nNote: This is useful for retrieving agent responses when MCP notifications are not visible.\nUse list_conversations to get rollout paths for active conversations.\n\nExample: get_conversation_events({ rolloutPath: \"/path/to/rollout.jsonl\", limit: 20 })")]
    pub async fn get_conversation_events(
        &self,
        Parameters(GetConversationEventsArgs { rollout_path, limit }): Parameters<GetConversationEventsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let limit = limit.unwrap_or(50);

        // Read the rollout file (blocking I/O in tokio context)
        let file_content = tokio::task::spawn_blocking({
            let path = rollout_path.clone();
            move || std::fs::read_to_string(path)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("Task failed: {}", e), None))?
        .map_err(|e| McpError::invalid_params(format!("Failed to read rollout file: {}", e), None))?;

        // Parse JSONL - each line is an event
        let events: Vec<serde_json::Value> = file_content
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        // Take last N events
        let start_idx = events.len().saturating_sub(limit);
        let recent_events: Vec<serde_json::Value> = events.into_iter().skip(start_idx).collect();

        let result = serde_json::json!({
            "events": recent_events,
            "count": recent_events.len()
        });

        Ok(CallToolResult::structured(result))
    }
}

#[tool_handler]
impl ServerHandler for Orchestrator {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "MCP server that manages Codex agent processes and proxies conversation methods.".into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

// == Upstream forwarding helpers (called by codex.rs read loop in future) ==

/// Forward a Codex event notification upstream to the MCP client as `codex/event`.
pub async fn notify_codex_event(_agent_id: &str, _event: serde_json::Value) -> Result<()> {
    if let Some(peer) = UPSTREAM_PEER.get() {
        let _ = peer
            .send_notification(LoggingMessageNotification {
                method: Default::default(),
                params: LoggingMessageNotificationParam {
                    level: LoggingLevel::Info,
                    logger: Some("codex/event".to_string()),
                    data: _event,
                },
                extensions: Default::default(),
            }
            .into())
            .await;
    }
    Ok(())
}

/// Request applyPatchApproval from the upstream MCP client and return decision.
#[allow(dead_code)]
pub async fn request_apply_patch_approval(
    _params: serde_json::Value,
) -> Result<serde_json::Value> {
    Err(anyhow!("approval request forwarding is not implemented yet"))
}

/// Request execCommandApproval from the upstream MCP client and return decision.
#[allow(dead_code)]
pub async fn request_exec_command_approval(
    _params: serde_json::Value,
) -> Result<serde_json::Value> {
    Err(anyhow!("approval request forwarding is not implemented yet"))
}
