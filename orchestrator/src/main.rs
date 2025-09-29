use anyhow::Result;
use parking_lot::Mutex;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ErrorData, InitializeRequestParam, JsonObject,
        ListToolsResult, LoggingLevel, LoggingMessageNotification, LoggingMessageNotificationParam,
        PaginatedRequestParam, ServerCapabilities, ServerInfo, Tool as McpTool,
    },
    service::{Peer, RequestContext, RoleServer, ServiceExt},
    ServerHandler,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;
use tokio::sync::RwLock;
use which::which;

struct LineWriter<W: Write> {
    inner: BufWriter<W>,
}

impl<W: Write> LineWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner: BufWriter::new(inner),
        }
    }

    fn write_message(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(bytes)?;
        self.inner.write_all(b"\n")?;
        self.inner.flush()
    }
}

struct Agent {
    child: Child,
    writer: LineWriter<ChildStdin>,
    rx: Arc<Mutex<mpsc::Receiver<String>>>,
}

struct State {
    agents: HashMap<String, Agent>,
}

impl State {
    fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }
}

#[derive(Clone)]
struct OrchestratorServer {
    state: Arc<Mutex<State>>,
    peer: Arc<RwLock<Option<Peer<RoleServer>>>>,
    event_tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}

struct AgentEvent {
    agent_id: String,
    event: Value,
}

impl OrchestratorServer {
    fn new() -> Self {
        let state = Arc::new(Mutex::new(State::new()));
        let peer = Arc::new(RwLock::new(None::<Peer<RoleServer>>));
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
        let peer_clone = peer.clone();
        tokio::spawn(async move {
            while let Some(ev) = event_rx.recv().await {
                let maybe_peer = {
                    let guard = peer_clone.read().await;
                    guard.clone()
                };
                if let Some(peer) = maybe_peer {
                    let notification =
                        LoggingMessageNotification::new(LoggingMessageNotificationParam {
                            level: LoggingLevel::Info,
                            logger: Some("codex/event".to_string()),
                            data: json!({"agentId": ev.agent_id, "event": ev.event}),
                        });
                    if let Err(err) = peer.send_notification(notification.into()).await {
                        eprintln!("codex-orchestrator: failed to send event notification: {err}");
                    }
                }
            }
        });

        Self {
            state,
            peer,
            event_tx,
        }
    }

    async fn record_peer(&self, peer: Peer<RoleServer>) {
        let mut guard = self.peer.write().await;
        *guard = Some(peer);
    }

    fn call_tool_sync(&self, request: CallToolRequestParam) -> Result<CallToolResult, ErrorData> {
        let CallToolRequestParam { name, arguments } = request;
        let args = arguments.unwrap_or_default();
        match name.as_ref() {
            "spawn_agent" => self.handle_spawn_agent(&args),
            "list_agents" => self.handle_list_agents(),
            "kill_agent" => self.handle_kill_agent(&args),
            "new_conversation" | "send_user_message" | "interrupt" => {
                self.handle_forwarded_tool(name.as_ref(), &args)
            }
            other => Err(ErrorData::invalid_params(
                format!("Unsupported tool: {other}"),
                Some(json!({"tool": other})),
            )),
        }
    }

    fn handle_spawn_agent(&self, args: &JsonObject) -> Result<CallToolResult, ErrorData> {
        let id_opt = args
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let agent_id = id_opt.unwrap_or_else(|| format!("agent-{}", uuid_like()));
        let cwd = args
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut cmd = std::env::var("CODEX_BIN")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "codex".to_string());
        if cmd == "codex" && which("codex-test").is_ok() {
            cmd = "codex-test".to_string();
        }

        let mut command = Command::new(cmd);
        command
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        let mut child = command
            .spawn()
            .map_err(|e| ErrorData::internal_error(format!("failed to spawn agent: {e}"), None))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ErrorData::internal_error("agent stdin unavailable", None))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ErrorData::internal_error("agent stdout unavailable", None))?;

        let writer = LineWriter::new(stdin);
        let mut reader = BufReader::new(stdout);
        let (line_tx, line_rx) = mpsc::channel::<String>();
        let event_tx = self.event_tx.clone();
        let agent_id_clone = agent_id.clone();
        thread::spawn(move || {
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match reader.read_line(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buffer.trim_end_matches(['\r', '\n']).to_string();
                        if line.is_empty() {
                            continue;
                        }
                        let _ = line_tx.send(line.clone());
                        if let Ok(value) = serde_json::from_str::<Value>(&line) {
                            let _ = event_tx.send(AgentEvent {
                                agent_id: agent_id_clone.clone(),
                                event: value,
                            });
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let agent = Agent {
            child,
            writer,
            rx: Arc::new(Mutex::new(line_rx)),
        };
        self.state.lock().agents.insert(agent_id.clone(), agent);

        Ok(CallToolResult::structured(json!({"agentId": agent_id})))
    }

    fn handle_list_agents(&self) -> Result<CallToolResult, ErrorData> {
        let list: Vec<_> = self.state.lock().agents.keys().cloned().collect();
        Ok(CallToolResult::structured(json!({"agents": list})))
    }

    fn handle_kill_agent(&self, args: &JsonObject) -> Result<CallToolResult, ErrorData> {
        let Some(agent_id) = args.get("agentId").and_then(|v| v.as_str()) else {
            return Err(ErrorData::invalid_params(
                "kill_agent requires 'agentId'",
                None,
            ));
        };
        let agent_opt = { self.state.lock().agents.remove(agent_id) };
        if let Some(mut agent) = agent_opt {
            let _ = agent.child.kill();
            let _ = agent.child.wait();
            Ok(CallToolResult::structured(json!({"ok": true})))
        } else {
            Err(ErrorData::invalid_params(
                "agent not found",
                Some(json!({"agentId": agent_id})),
            ))
        }
    }

    fn handle_forwarded_tool(
        &self,
        tool: &str,
        args: &JsonObject,
    ) -> Result<CallToolResult, ErrorData> {
        let agent_id = args
            .get("agentId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ErrorData::invalid_params("Missing 'agentId'", None))?;
        let method = match tool {
            "new_conversation" => "newConversation",
            "send_user_message" => "sendUserMessage",
            _ => "interruptConversation",
        };
        let inner_params = args.get("params").cloned().unwrap_or_else(|| json!({}));

        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": inner_params
        });
        let line = serde_json::to_string(&req)
            .map_err(|e| ErrorData::internal_error(format!("serialize request: {e}"), None))?;

        let rx = {
            let mut state = self.state.lock();
            let Some(agent) = state.agents.get_mut(agent_id) else {
                return Err(ErrorData::invalid_params(
                    "agent not found",
                    Some(json!({"agentId": agent_id})),
                ));
            };

            agent.writer.write_message(line.as_bytes()).map_err(|e| {
                ErrorData::internal_error(format!("failed to write to agent: {e}"), None)
            })?;

            agent.rx.clone()
        };

        let response = {
            let guard = rx.lock();
            guard
                .recv_timeout(Duration::from_secs(60))
                .map_err(|e| match e {
                    mpsc::RecvTimeoutError::Timeout => {
                        ErrorData::internal_error("agent response timeout", None)
                    }
                    mpsc::RecvTimeoutError::Disconnected => {
                        ErrorData::internal_error("agent channel closed", None)
                    }
                })?
        };

        let parsed: Value = serde_json::from_str(&response)
            .map_err(|e| ErrorData::internal_error(format!("invalid agent response: {e}"), None))?;
        Ok(CallToolResult::structured(json!({
            "agentResponse": parsed
        })))
    }
}

impl ServerHandler for OrchestratorServer {
    fn get_info(&self) -> ServerInfo {
        server_info()
    }

    fn initialize(
        &self,
        request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ServerInfo, ErrorData>> + Send + '_ {
        let server = self.clone();
        async move {
            if context.peer.peer_info().is_none() {
                context.peer.set_peer_info(request);
            }
            server.record_peer(context.peer.clone()).await;
            Ok(server_info())
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        let server = self.clone();
        async move {
            server.record_peer(context.peer.clone()).await;
            tokio::task::spawn_blocking(move || server.call_tool_sync(request))
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("call tool task panicked: {e}"), None)
                })?
        }
    }
}

fn server_info() -> ServerInfo {
    let mut info = ServerInfo::default();
    info.server_info.name = "codex-orchestrator".to_string();
    info.server_info.version = env!("CARGO_PKG_VERSION").to_string();
    info.instructions = Some("Manage Codex MCP agents and proxy conversation tooling.".to_string());
    info.capabilities = ServerCapabilities::builder().enable_tools().build();
    info
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
    vec![
        McpTool::new(
            "spawn_agent",
            "Spawn a Codex MCP agent process",
            schema(json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "cwd": {"type": "string"}
                }
            })),
        ),
        McpTool::new(
            "list_agents",
            "List active agents",
            schema(json!({"type": "object"})),
        ),
        McpTool::new(
            "kill_agent",
            "Terminate an agent by id",
            schema(json!({
                "type": "object",
                "properties": {"agentId": {"type": "string"}},
                "required": ["agentId"]
            })),
        ),
        McpTool::new(
            "new_conversation",
            "Create a conversation (forwarded to agent)",
            schema(json!({
                "type": "object",
                "properties": {
                    "agentId": {"type": "string"},
                    "params": {"type": "object"}
                },
                "required": ["agentId"]
            })),
        ),
        McpTool::new(
            "send_user_message",
            "Send a user message (forwarded to agent)",
            schema(json!({
                "type": "object",
                "properties": {
                    "agentId": {"type": "string"},
                    "params": {"type": "object"}
                },
                "required": ["agentId"]
            })),
        ),
        McpTool::new(
            "interrupt",
            "Interrupt a conversation (forwarded to agent)",
            schema(json!({
                "type": "object",
                "properties": {
                    "agentId": {"type": "string"},
                    "params": {"type": "object"}
                },
                "required": ["agentId"]
            })),
        ),
    ]
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{:x}", nanos)
}

#[tokio::main]
async fn main() -> Result<()> {
    let server = OrchestratorServer::new();
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tools_list_has_schemas() {
        let items = tools();
        assert!(!items.is_empty());
        assert!(items.iter().all(|tool| !tool.input_schema.is_empty()));
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        let server = OrchestratorServer::new();
        let req = CallToolRequestParam {
            name: "does/not/exist".into(),
            arguments: Some(JsonObject::default()),
        };
        let err = server.call_tool_sync(req).expect_err("expected error");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kill_agent_completes_with_pending_forward() {
        use rmcp::model::ErrorCode;
        use std::sync::Barrier;
        use std::time::{Duration, Instant};

        let server = OrchestratorServer::new();

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("while read line; do sleep 3600; done")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command.spawn().expect("spawn stub agent");
        let stdin = child.stdin.take().expect("stub agent stdin");
        let stdout = child.stdout.take().expect("stub agent stdout");

        let writer = LineWriter::new(stdin);
        drop(stdout);
        let (line_tx, line_rx) = mpsc::channel::<String>();
        let keep_sender = Some(line_tx);
        let agent_id = "test-agent".to_string();

        let agent = Agent {
            child,
            writer,
            rx: Arc::new(Mutex::new(line_rx)),
        };
        server.state.lock().agents.insert(agent_id.clone(), agent);

        let barrier = Arc::new(Barrier::new(2));
        let forward_barrier = barrier.clone();
        let server_clone = server.clone();
        let forward_agent_id = agent_id.clone();
        let forward_handle = thread::spawn(move || {
            forward_barrier.wait();
            let mut args = JsonObject::new();
            args.insert("agentId".into(), Value::String(forward_agent_id));
            args.insert(
                "params".into(),
                json!({"conversationId": "c1", "message": "hi"}),
            );

            let req = CallToolRequestParam {
                name: "send_user_message".into(),
                arguments: Some(args),
            };

            server_clone.call_tool_sync(req)
        });

        barrier.wait();
        thread::sleep(Duration::from_millis(100));

        let mut kill_args = JsonObject::new();
        kill_args.insert("agentId".into(), Value::String(agent_id.clone()));
        let start = Instant::now();
        let _kill_result = server
            .call_tool_sync(CallToolRequestParam {
                name: "kill_agent".into(),
                arguments: Some(kill_args),
            })
            .expect("kill agent succeeds");
        assert!(start.elapsed() < Duration::from_secs(1));

        drop(keep_sender);

        let forward_result = forward_handle.join().expect("forward thread join");
        let err = forward_result.expect_err("forwarded call should error after kill");
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);

        let guard = server.state.lock();
        assert!(!guard.agents.contains_key(&agent_id));
    }
}
