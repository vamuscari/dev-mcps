# Codex Orchestrator Notifications & Event Handling

## Problem

When using the Codex orchestrator MCP server, agent responses and events are sent as MCP notifications rather than being returned in tool call responses. This can make them invisible in MCP clients that don't display notifications by default.

## How Notifications Work

The orchestrator forwards all Codex agent events as MCP logging notifications:
- **Notification type**: `notifications/message`
- **Logger**: `codex/event`
- **Payload**: JSON containing agent events (responses, tool calls, approvals, etc.)

This is implemented in `src/codex.rs`:
```rust
// Line 419-424
Ok(JsonRpcMessage::Notification(JsonRpcNotification { notification, .. })) => {
    tracing::debug!("read_loop: got notification method={}", notification.method);
    let payload = json!({
        "method": notification.method,
        "params": notification.params,
    });
    let _ = mcp::notify_codex_event(&agent.id, payload).await;
}
```

And forwarded in `src/mcp.rs`:
```rust
// Line 353-369
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
```

## Solutions for Retrieving Agent Responses

### Solution 1: Configure MCP Client to Display Notifications

Configure your MCP client to listen for and display notifications with logger `codex/event`. The exact method depends on your client implementation.

### Solution 2: Poll Rollout Files (Recommended)

Use the new `get_conversation_events` tool to read events directly from the conversation rollout file:

```javascript
// 1. Start a conversation
const conv = await call_tool("new_conversation", {
  agentId: "my-agent",
  params: { prompt: "Hello" }
});

// 2. Send a message to trigger processing
await call_tool("send_user_turn", {
  agentId: "my-agent",
  params: { text: "List files in current directory" }
});

// 3. Wait a moment for processing
await sleep(2000);

// 4. Read events from the rollout
const events = await call_tool("get_conversation_events", {
  rolloutPath: conv.rolloutPath,
  limit: 20
});

console.log(events.events);  // Contains all conversation events
```

### Solution 3: Implement Custom Notification Handler

In your MCP client, implement a handler for incoming notifications:

```javascript
client.on('notification', (notification) => {
  if (notification.params?.logger === 'codex/event') {
    console.log('Codex event:', notification.params.data);
  }
});
```

## Important Notes

1. **`new_conversation` does NOT trigger processing** - It only creates conversation metadata. You MUST call `send_user_message` or `send_user_turn` after `new_conversation` to trigger the Codex agent to process the initial prompt.

2. **Rollout files are append-only JSONL** - Each line is a complete JSON event. They grow as the conversation progresses.

3. **Events include**:
   - Session metadata
   - User messages
   - Agent responses
   - Tool calls and results
   - Approval requests
   - Turn context updates

## Debugging

Enable debug logging to see notification flow:

```bash
RUST_LOG=debug cargo run -p codex-orchestrator
```

Look for log lines like:
- `read_loop: got notification method=...` - Notifications received from Codex agent
- `rpc_call: method=..., id=...` - RPC calls to Codex agent
- `rpc_call: id=... got response: ...` - RPC responses from Codex agent
