use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use rmcp::model::{
    InitializeRequestParam, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, JsonRpcVersion2_0, Notification, Request, RequestId,
};
use rmcp::transport::async_rw::JsonRpcMessageCodec;
use tokio_util::codec::{FramedRead, FramedWrite};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde_json::{json, Value};
use tokio::{
    process::Command,
    sync::{Mutex, RwLock, oneshot},
};

use crate::mcp;

/// Manages Codex agent processes and RPC clients.
#[derive(Default, Clone)]
pub struct Manager {
    agents: Arc<RwLock<HashMap<String, Arc<Agent>>>>,
    approvals: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}

#[derive(Debug)]
struct Agent {
    id: String,
    #[allow(dead_code)]
    cwd: Option<PathBuf>,
    child: Mutex<tokio::process::Child>,
    reader: Arc<Mutex<FramedRead<tokio::process::ChildStdout, JsonRpcMessageCodec<RawMsg>>>>,
    writer: Arc<Mutex<FramedWrite<tokio::process::ChildStdin, JsonRpcMessageCodec<RawMsg>>>>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>>,
    last_conversation_id: Mutex<Option<String>>, 
}

type RawReq = Request<String, Value>;
type RawNot = Notification<String, Value>;
type RawMsg = JsonRpcMessage<RawReq, Value, RawNot>;

impl Manager {
    pub async fn spawn_agent(&self, id: Option<String>, cwd: Option<PathBuf>) -> Result<String> {
        let agent_id = match id {
            Some(s) if !s.is_empty() => s,
            _ => format!(
                "agent-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros()
            ),
        };

        // Resolve binary: env CODEX_BIN, else which("codex")
        let bin = if let Some(v) = std::env::var("CODEX_BIN").ok().filter(|s| !s.is_empty()) {
            v
        } else if let Ok(path) = which::which("codex") {
            path.to_string_lossy().into_owned()
        } else {
            return Err(anyhow!("Unable to locate Codex binary. Set CODEX_BIN or add 'codex' to PATH."));
        };

        let mut cmd = Command::new(bin);
        cmd.arg("mcp");
        if let Some(ref c) = cwd {
            cmd.current_dir(c);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());

        let mut child = cmd.spawn().map_err(|e| anyhow!("spawn codex failed: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("child stdout missing"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("child stdin missing"))?;

        let reader: FramedRead<_, JsonRpcMessageCodec<RawMsg>> =
            FramedRead::new(stdout, JsonRpcMessageCodec::new());
        let writer: FramedWrite<_, JsonRpcMessageCodec<RawMsg>> =
            FramedWrite::new(stdin, JsonRpcMessageCodec::new());

        let agent = Arc::new(Agent {
            id: agent_id.clone(),
            cwd,
            child: Mutex::new(child),
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            last_conversation_id: Mutex::new(None),
        });

        // Initialize MCP handshake
        self.initialize(&agent).await?;
        // Start read loop
        self.spawn_read_loop(agent.clone());

        self.agents.write().await.insert(agent_id.clone(), agent);
        Ok(agent_id)
    }

    pub async fn list_agents(&self) -> Vec<String> {
        self.agents.read().await.keys().cloned().collect()
    }

    pub async fn kill_agent(&self, agent_id: &str) -> Result<()> {
        let removed = self.agents.write().await.remove(agent_id);
        match removed {
            Some(agent) => {
                if let Ok(mut child) = agent.child.try_lock() {
                    let _ = child.kill().await;
                }
                Ok(())
            }
            None => Err(anyhow!("agent not found: {agent_id}")),
        }
    }

    pub async fn new_conversation(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let value = self
            .rpc_call(&agent, "newConversation", params)
            .await?;
        if let Some(cid) = value
            .get("conversationId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| value.get("conversation_id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        {
            *agent.last_conversation_id.lock().await = Some(cid);
        }
        Ok(value)
    }

    pub async fn send_user_message(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let params = self.prepare_message_params(&agent, params).await?;
        let value = self
            .rpc_call(&agent, "sendUserMessage", params)
            .await?;
        Ok(value)
    }

    pub async fn send_user_turn(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let mut params = self.prepare_message_params(&agent, params).await?;

        // sendUserTurn requires additional fields - provide sensible defaults if missing
        if let Value::Object(ref mut map) = params {
            if !map.contains_key("cwd") {
                map.insert("cwd".to_string(), json!(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"))));
            }
            if !map.contains_key("approvalPolicy") {
                map.insert("approvalPolicy".to_string(), json!("never"));
            }
            if !map.contains_key("sandboxPolicy") {
                map.insert("sandboxPolicy".to_string(), json!({"mode": "read-only"}));
            }
            if !map.contains_key("model") {
                map.insert("model".to_string(), json!("gpt-4"));
            }
            if !map.contains_key("summary") {
                map.insert("summary".to_string(), json!("auto"));
            }
        }

        let value = self
            .rpc_call(&agent, "sendUserTurn", params)
            .await?;
        Ok(value)
    }

    pub async fn interrupt(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let mut params = params;
        if !params.get("conversationId").is_some() && !params.get("conversation_id").is_some() {
            if let Some(cid) = agent.last_conversation_id.lock().await.clone() {
                match &mut params {
                    Value::Object(map) => {
                        map.insert("conversationId".to_string(), Value::String(cid));
                    }
                    _ => {
                        params = json!({"conversationId": cid});
                    }
                }
            }
        }
        let value = self
            .rpc_call(&agent, "interruptConversation", params)
            .await?;
        Ok(value)
    }

    pub async fn list_conversations(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let value = self
            .rpc_call(&agent, "listConversations", params)
            .await?;
        Ok(value)
    }

    pub async fn resume_conversation(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let value = self
            .rpc_call(&agent, "resumeConversation", params)
            .await?;
        // Update last_conversation_id if present in response
        if let Some(cid) = value
            .get("conversationId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| value.get("conversation_id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        {
            *agent.last_conversation_id.lock().await = Some(cid);
        }
        Ok(value)
    }

    pub async fn archive_conversation(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
        let value = self
            .rpc_call(&agent, "archiveConversation", params)
            .await?;
        Ok(value)
    }

    async fn prepare_message_params(&self, agent: &Agent, params: Value) -> Result<Value> {
        // Normalize params into an object with at least items or text, and ensure conversationId if possible.
        let mut obj = match params {
            Value::String(s) => {
                json!({
                    "items": [{"type": "text", "data": {"text": s}}]
                })
                .as_object()
                .cloned()
                .unwrap()
            }
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            other => {
                // Wrap other scalar/array as a single text item
                json!({
                    "items": [{"type": "text", "data": {"text": other.to_string()}}]
                })
                .as_object()
                .cloned()
                .unwrap()
            }
        };

        // If no items but has text/message/prompt, convert.
        let has_items = obj.get("items").and_then(|v| v.as_array()).is_some();
        if !has_items {
            if let Some(text) = obj
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| obj.get("message").and_then(|v| v.as_str()).map(|s| s.to_string()))
                .or_else(|| obj.get("prompt").and_then(|v| v.as_str()).map(|s| s.to_string()))
            {
                obj.remove("text");
                obj.remove("message");
                obj.remove("prompt");
                obj.insert(
                    "items".to_string(),
                    json!([{ "type": "text", "data": { "text": text } }]),
                );
            }
        }

        // Ensure conversationId if we have a remembered one and it's missing.
        let has_cid = obj.contains_key("conversationId") || obj.contains_key("conversation_id");
        if !has_cid {
            if let Some(cid) = agent.last_conversation_id.lock().await.clone() {
                obj.insert("conversationId".to_string(), Value::String(cid));
            }
        }

        Ok(Value::Object(obj))
    }

    async fn require_agent(&self, agent_id: &str) -> Result<Arc<Agent>> {
        self.agents
            .read()
            .await
            .get(agent_id)
            .cloned()
            .ok_or_else(|| anyhow!("agent not found: {agent_id}"))
    }

    async fn initialize(&self, agent: &Arc<Agent>) -> Result<()> {
        let params = InitializeRequestParam::default();
        let req = Request::<String, Value> {
            method: "initialize".to_string(),
            params: serde_json::to_value(&params)?,
            extensions: Default::default(),
        };
        let id = Self::next_id();
        let msg = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JsonRpcVersion2_0,
            id: RequestId::Number(id),
            request: req,
        });
        {
            let mut w = agent.writer.lock().await;
            w.send(msg).await.map_err(|e| anyhow!("send init failed: {e}"))?;
        }
        // await response for initialize
        loop {
            let opt = { let mut r = agent.reader.lock().await; r.next().await };
            let Some(pkt) = opt else { return Err(anyhow!("codex closed during init")); };
            match pkt {
                Ok(JsonRpcMessage::Response(JsonRpcResponse { id: rid, .. })) if rid == RequestId::Number(id) => {
                    break;
                }
                Ok(JsonRpcMessage::Error(e)) if e.id == RequestId::Number(id) => {
                    return Err(anyhow!("initialize error: {}", e.error.message));
                }
                Ok(JsonRpcMessage::Notification(n)) => {
                    let payload = json!({
                        "method": n.notification.method,
                        "params": n.notification.params,
                    });
                    let _ = mcp::notify_codex_event(&agent.id, payload).await;
                }
                Ok(_) => {}
                Err(e) => return Err(anyhow!("transport error during init: {}", e)),
            }
        }
        // Send initialized notification
        let not = JsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: JsonRpcVersion2_0,
            notification: Notification::<String, Value> {
                method: "notifications/initialized".to_string(),
                params: json!({}),
                extensions: Default::default(),
            },
        });
        { let mut w = agent.writer.lock().await; w.send(not).await.map_err(|e| anyhow!("send initialized failed: {e}"))?; }
        Ok(())
    }

    fn spawn_read_loop(&self, agent: Arc<Agent>) {
        let approvals = self.approvals.clone();
        tokio::spawn(async move {
            tracing::debug!("read_loop: started for agent {}", agent.id);
            loop {
                let msg_opt = { let mut r = agent.reader.lock().await; r.next().await };
                let Some(pkt) = msg_opt else {
                    tracing::warn!("read_loop: agent {} stream ended", agent.id);
                    // Drain and fail any pending RPC waiters so callers don't hang
                    let drained: Vec<oneshot::Sender<Result<Value, Value>>> = {
                        let mut guard = agent.pending.lock().await;
                        let mut map = std::mem::take(&mut *guard);
                        map.drain().map(|(_, tx)| tx).collect()
                    };
                    for tx in drained {
                        let _ = tx.send(Err(json!({
                            "error": "agent terminated",
                            "agentId": agent.id,
                        })));
                    }
                    break
                };
                match pkt {
                    Ok(JsonRpcMessage::Response(JsonRpcResponse { id, result, .. })) => {
                        let key = match id {
                            RequestId::Number(n) => n,
                            RequestId::String(s) => {
                                tracing::warn!("string id not supported: {}", s);
                                continue;
                            }
                        };
                        tracing::debug!("read_loop: got response for id={}", key);
                        if let Some(tx) = agent.pending.lock().await.remove(&key) {
                            let _ = tx.send(Ok(result));
                        } else {
                            tracing::warn!("read_loop: no pending waiter for response id={}", key);
                        }
                    }
                    Ok(JsonRpcMessage::Error(err)) => {
                        let key = match err.id {
                            RequestId::Number(n) => n,
                            _ => -1,
                        };
                        if key >= 0 {
                            if let Some(tx) = agent.pending.lock().await.remove(&key) {
                                let _ = tx.send(Err(serde_json::to_value(err.error).unwrap_or(json!({"error": "unknown"}))));
                            }
                        }
                    }
                    Ok(JsonRpcMessage::Notification(JsonRpcNotification { notification, .. })) => {
                        tracing::debug!("read_loop: got notification method={}", notification.method);
                        let payload = json!({
                            "method": notification.method,
                            "params": notification.params,
                        });
                        let _ = mcp::notify_codex_event(&agent.id, payload).await;
                    }
                    Ok(JsonRpcMessage::Request(JsonRpcRequest { id, request, .. })) => {
                        // Only treat known approval methods as approvals; otherwise reply with empty result
                        let method = request.method.clone();
                        if method == "applyPatchApproval" || method == "execCommandApproval" {
                            // Register pending approval
                            let req_id_str = match &id {
                                RequestId::Number(n) => n.to_string(),
                                RequestId::String(s) => s.to_string(),
                            };
                            let key = format!("{}:{}", agent.id, req_id_str);
                            let (tx, rx) = oneshot::channel::<String>();
                            approvals.lock().await.insert(key.clone(), tx);
                            // Notify upstream client
                            let payload = json!({
                                "kind": "approval_request",
                                "agentId": agent.id,
                                "requestId": req_id_str,
                                "method": request.method,
                                "params": request.params,
                            });
                            let _ = mcp::notify_codex_event(&agent.id, payload).await;
                            // Wait for decision with timeout
                            let decision = match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
                                Ok(Ok(s)) => s,
                                _ => "deny".to_string(),
                            };
                            let result = json!({ "decision": decision });
                            let resp = JsonRpcMessage::Response(JsonRpcResponse { jsonrpc: JsonRpcVersion2_0, id, result });
                            let mut w = agent.writer.lock().await;
                            if let Err(e) = w.send(resp).await { tracing::warn!("failed send approval resp: {}", e); }
                        } else {
                            // Unknown request from Codex – log and reply with a benign empty result
                            let payload = json!({
                                "kind": "codex_request",
                                "agentId": agent.id,
                                "method": method,
                                "params": request.params,
                            });
                            let _ = mcp::notify_codex_event(&agent.id, payload).await;
                            let result = json!({});
                            let resp = JsonRpcMessage::Response(JsonRpcResponse { jsonrpc: JsonRpcVersion2_0, id, result });
                            let mut w = agent.writer.lock().await;
                            if let Err(e) = w.send(resp).await { tracing::warn!("failed send generic resp: {}", e); }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("transport read error: {}", e);
                        // Drain and fail any pending RPC waiters so callers don't hang
                        let drained: Vec<oneshot::Sender<Result<Value, Value>>> = {
                            let mut guard = agent.pending.lock().await;
                            let mut map = std::mem::take(&mut *guard);
                            map.drain().map(|(_, tx)| tx).collect()
                        };
                        for tx in drained {
                            let _ = tx.send(Err(json!({
                                "error": "agent read error",
                                "message": e.to_string(),
                                "agentId": agent.id,
                            })));
                        }
                        break;
                    }
                }
            }
        });
    }

    fn next_id() -> i64 {
        use std::sync::atomic::{AtomicI64, Ordering};
        static NEXT: AtomicI64 = AtomicI64::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    async fn rpc_call(&self, agent: &Arc<Agent>, method: &str, params: Value) -> Result<Value> {
        // rmcp Request may flatten params; ensure it's an object to avoid serde flattening errors
        let params = match params {
            Value::Object(_) => params,
            Value::Null => json!({}),
            other => json!({ "value": other }),
        };
        let id = Self::next_id();
        tracing::debug!("rpc_call: method={}, id={}, params={}", method, id, serde_json::to_string(&params).unwrap_or_default());
        let req = Request::<String, Value> {
            method: method.to_string(),
            params,
            extensions: Default::default(),
        };
        let msg = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: JsonRpcVersion2_0,
            id: RequestId::Number(id),
            request: req,
        });
        // Register waiter
        let (tx, rx) = oneshot::channel();
        agent.pending.lock().await.insert(id, tx);
        // Send request
        { let mut w = agent.writer.lock().await; w.send(msg).await.map_err(|e| anyhow!("send {} failed: {}", method, e))?; }
        tracing::debug!("rpc_call: sent request id={}, waiting for response...", id);
        match rx.await {
            Ok(Ok(val)) => {
                tracing::debug!("rpc_call: id={} got response: {}", id, serde_json::to_string(&val).unwrap_or_default());
                Ok(val)
            },
            Ok(Err(err)) => {
                tracing::warn!("rpc_call: id={} got error: {}", id, err);
                Err(anyhow!("rpc error: {}", err))
            },
            Err(_) => {
                tracing::warn!("rpc_call: id={} cancelled", id);
                Err(anyhow!("rpc cancelled"))
            },
        }
    }

    pub async fn list_pending_approvals(&self) -> Vec<String> {
        self.approvals
            .lock()
            .await
            .keys()
            .cloned()
            .collect()
    }

    pub async fn decide_approval(&self, key: &str, decision: String) -> Result<bool> {
        if let Some(tx) = self.approvals.lock().await.remove(key) {
            let _ = tx.send(decision);
            Ok(true)
        } else {
            Err(anyhow!("approval key not found: {}", key))
        }
    }
}
