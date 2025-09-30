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

    fn normalize_params(mut params: serde_json::Value) -> serde_json::Value {
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

#[tool_router]
impl Orchestrator {
    #[tool(description = "Start an MCP-capable Codex agent process. Returns { agentId }.")]
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

    #[tool(description = "List identifiers of running agents started by the orchestrator.")]
    pub async fn list_agents(
        &self,
        _params: Parameters<ListAgentsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let agent_ids = self.inner.manager.list_agents().await;
        let value = serde_json::to_value(ListAgentsResult { agent_ids })
            .unwrap_or_else(|_| serde_json::json!({"agentIds": []}));
        Ok(CallToolResult::success(vec![Content::text(value.to_string())]))
    }

    #[tool(description = "Terminate a managed agent.")]
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

    #[tool(description = "Forwarded to the agent as newConversation.")]
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

    #[tool(description = "Forwarded to the agent as sendUserMessage.")]
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

    #[tool(description = "Forwarded to the agent as sendUserTurn.")]
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

    #[tool(description = "Forwarded as interruptConversation (if supported by the agent).")]
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

    #[tool(description = "List pending Codex approvals (keys).")]
    pub async fn list_pending_approvals(
        &self,
        _params: Parameters<ListApprovalsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let keys = self.inner.manager.list_pending_approvals().await;
        let value = serde_json::to_value(ListApprovalsResult { keys })
            .unwrap_or_else(|_| serde_json::json!({"keys": []}));
        Ok(CallToolResult::structured(value))
    }

    #[tool(description = "Decide a pending Codex approval. Arguments: { key, decision } where decision is 'allow' or 'deny'.")]
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
#[allow(dead_code)]
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
