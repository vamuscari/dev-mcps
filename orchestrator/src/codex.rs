use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use crate::protocol_types as codex_mcp;
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

        // Resolve binary: env CODEX_BIN, else which("codex") then which("codex-test")
        let bin = if let Some(v) = std::env::var("CODEX_BIN").ok().filter(|s| !s.is_empty()) {
            v
        } else if let Ok(path) = which::which("codex") {
            path.to_string_lossy().into_owned()
        } else if let Ok(path) = which::which("codex-test") {
            path.to_string_lossy().into_owned()
        } else {
            return Err(anyhow!("Unable to locate Codex binary. Set CODEX_BIN or add 'codex'/'codex-test' to PATH."));
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
        Ok(value)
    }

    pub async fn send_user_message(
        &self,
        agent_id: &str,
        params: Value,
    ) -> Result<Value> {
        let agent = self.require_agent(agent_id).await?;
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
        let value = self
            .rpc_call(&agent, "interruptConversation", params)
            .await?;
        Ok(value)
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
            loop {
                let msg_opt = { let mut r = agent.reader.lock().await; r.next().await };
                let Some(pkt) = msg_opt else { break };
                match pkt {
                    Ok(JsonRpcMessage::Response(JsonRpcResponse { id, result, .. })) => {
                        let key = match id {
                            RequestId::Number(n) => n,
                            RequestId::String(s) => {
                                tracing::warn!("string id not supported: {}", s);
                                continue;
                            }
                        };
                        if let Some(tx) = agent.pending.lock().await.remove(&key) {
                            let _ = tx.send(Ok(result));
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
                        let payload = json!({
                            "method": notification.method,
                            "params": notification.params,
                        });
                        let _ = mcp::notify_codex_event(&agent.id, payload).await;
                    }
                    Ok(JsonRpcMessage::Request(JsonRpcRequest { id, request, .. })) => {
                        // Handle approvals: auto-deny for now
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
                    }
                    Err(e) => {
                        tracing::warn!("transport read error: {}", e);
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
        let id = Self::next_id();
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
        match rx.await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(err)) => Err(anyhow!("rpc error: {}", err)),
            Err(_) => Err(anyhow!("rpc cancelled")),
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
