# Orchestrator Architecture

## Overview

The Codex MCP Orchestrator is a meta-server that manages multiple Codex agent processes, each of which can handle multiple concurrent conversations.

## Architecture Layers

```
┌─────────────────────────────────────────────────────────────┐
│                    MCP Client (User)                        │
│              (Claude Desktop, VS Code, etc)                 │
└─────────────────────────────────────────────────────────────┘
                              │
                    MCP over stdio (JSON-RPC)
                              │
┌─────────────────────────────────────────────────────────────┐
│              Orchestrator MCP Server                        │
│  ┌───────────────────────────────────────────────────────┐ │
│  │  MCP Tools (spawn_agent, list_conversations, etc)    │ │
│  └───────────────────────────────────────────────────────┘ │
│  ┌───────────────────────────────────────────────────────┐ │
│  │  Manager                                              │ │
│  │  • agents: HashMap<AgentId, Agent>                   │ │
│  │  • approvals: HashMap<Key, oneshot::Sender>          │ │
│  └───────────────────────────────────────────────────────┘ │
│  ┌───────────────────────────────────────────────────────┐ │
│  │  Upstream Peer (for event forwarding)                │ │
│  └───────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                              │
              Multiple concurrent connections
                              │
        ┌─────────────┬───────┴───────┬─────────────┐
        │             │               │             │
┌───────▼──────┐ ┌───▼────────┐ ┌────▼──────┐ ┌───▼────────┐
│  Agent 1     │ │  Agent 2   │ │  Agent 3  │ │  Agent N   │
│  (Codex MCP) │ │ (Codex MCP)│ │(Codex MCP)│ │(Codex MCP) │
├──────────────┤ ├────────────┤ ├───────────┤ ├────────────┤
│ • Conv A     │ │ • Conv X   │ │ • Conv P  │ │ • Conv 1   │
│ • Conv B     │ │ • Conv Y   │ │ • Conv Q  │ │ • Conv 2   │
│ • Conv C     │ │            │ │ • Conv R  │ │ • Conv 3   │
└──────────────┘ └────────────┘ └───────────┘ └────────────┘
```

## Key Components

### Manager
- **Purpose**: Manages lifecycle of Codex agent processes
- **State**:
  - `agents: HashMap<String, Arc<Agent>>` - Active agents by ID
  - `approvals: HashMap<String, Sender>` - Pending approvals by key

### Agent
- **Purpose**: Represents a single Codex MCP process
- **State**:
  - `id: String` - Unique agent identifier
  - `child: Process` - Subprocess handle
  - `reader/writer: JsonRpcMessageCodec` - Stdio communication
  - `pending: HashMap<i64, Sender>` - Pending RPC responses
  - `last_conversation_id: Option<String>` - Convenience tracker (optional)

### Read Loop (per Agent)
- **Purpose**: Continuously reads messages from agent's stdout
- **Handles**:
  1. **Responses** → Resolve pending RPC calls
  2. **Notifications** → Forward to upstream via `notify_codex_event`
  3. **Requests** (Approvals) → Register, notify upstream, wait for decision
  4. **Requests** (Other) → Log and reply with empty result

### Upstream Peer
- **Purpose**: Send notifications back to MCP client
- **Set Once**: During server initialization in `main.rs`
- **Thread-safe**: Global `OnceCell<ClientSink>`

## Conversation Model

### Multiple Conversations Per Agent: YES ✅

Each Codex agent process can handle **multiple concurrent conversations**:

```rust
// Codex's internal structure (simplified)
struct ConversationManager {
    conversations: HashMap<ConversationId, Arc<CodexConversation>>,
    auth_manager: Arc<AuthManager>,
}
```

**Key Points:**
1. Each agent maintains a `ConversationManager` with a map of conversations
2. Conversations are identified by unique `ConversationId` (UUID)
3. Multiple conversations can be active simultaneously on one agent
4. Messages are routed by `conversationId` parameter
5. The orchestrator's `last_conversation_id` is just for convenience (auto-fill)

**Example Workflow:**
```javascript
// Spawn one agent
const agent = await spawn_agent({ id: "my-agent" });

// Create multiple conversations on same agent
const conv1 = await new_conversation({ agentId: "my-agent" }); // → { conversationId: "abc" }
const conv2 = await new_conversation({ agentId: "my-agent" }); // → { conversationId: "def" }
const conv3 = await new_conversation({ agentId: "my-agent" }); // → { conversationId: "ghi" }

// Interleave messages to different conversations
await send_user_message({ agentId: "my-agent", params: { conversationId: "abc", ... } });
await send_user_message({ agentId: "my-agent", params: { conversationId: "def", ... } });
await send_user_message({ agentId: "my-agent", params: { conversationId: "abc", ... } });
await send_user_message({ agentId: "my-agent", params: { conversationId: "ghi", ... } });

// All three are active and independent
```

## Event Flow

### Notifications (Codex → Client)

```
Codex Agent
    ↓ (stdout)
JsonRpcNotification
    ↓
Read Loop (codex.rs:391-397)
    ↓
notify_codex_event(agent_id, payload)
    ↓
UPSTREAM_PEER.send_notification()
    ↓
MCP Client receives LoggingMessageNotification
    logger: "codex/event"
    data: { method, params }
```

### Approval Requests (Codex ↔ Client)

```
Codex Agent
    ↓ (stdout)
JsonRpcRequest { method: "execCommandApproval", ... }
    ↓
Read Loop (codex.rs:401-427)
    ↓
1. Register in approvals map with key "agentId:requestId"
2. Notify upstream via notify_codex_event()
3. Wait on oneshot channel (60s timeout)
    ↓
MCP Client receives event, decides
    ↓
Client calls decide_approval({ key, decision: "allow"|"deny" })
    ↓
Manager sends decision to waiting channel
    ↓
Read Loop sends JsonRpcResponse back to Codex
    ↓
Codex Agent continues with approval decision
```

### Response Flow (Client → Codex)

```
MCP Client calls tool (e.g., send_user_message)
    ↓
MCP Tool Handler (mcp.rs)
    ↓
Manager method (e.g., send_user_message)
    ↓
Agent.rpc_call(method, params)
    ↓
1. Generate request ID
2. Create oneshot channel
3. Store in agent.pending map
4. Send JsonRpcRequest to agent's stdin
    ↓
Read Loop receives JsonRpcResponse
    ↓
1. Extract request ID from response
2. Find oneshot sender in pending map
3. Send result to channel
    ↓
rpc_call() awaits on oneshot receiver
    ↓
Returns result to MCP client
```

## Thread Safety

- **Manager**: `Arc<RwLock<HashMap>>` for agents - allows concurrent access
- **Agent**: `Arc<Agent>` - shared across threads
- **Pending requests**: `Arc<Mutex<HashMap>>` - thread-safe request tracking
- **Approvals**: `Arc<Mutex<HashMap>>` - thread-safe approval tracking
- **Reader/Writer**: `Arc<Mutex<FramedRead/Write>>` - exclusive access to stdio

## Concurrency Model

1. **One read loop per agent** - Tokio task spawned on agent creation
2. **Concurrent tool calls** - Multiple clients can call tools simultaneously
3. **Concurrent RPC** - Multiple RPC calls to same agent are serialized by mutex
4. **Event forwarding** - Non-blocking, fire-and-forget to upstream
5. **Approval handling** - Blocks calling task until decision or timeout

## Scalability Considerations

### Agents
- Each agent is a separate OS process
- Limited by system resources (memory, file descriptors)
- Typical: 1-10 agents per orchestrator

### Conversations per Agent
- Limited by Codex's internal memory management
- Each conversation maintains:
  - Full message history
  - Rollout file (JSONL on disk)
  - Token usage tracking
- Typical: 1-5 active conversations per agent
- Archived conversations don't consume resources

### Performance
- **Bottleneck**: Agent subprocess stdio throughput
- **Mitigation**: Use multiple agents for parallel workloads
- **Best Practice**: Archive finished conversations to free memory

## Testing Architecture

The test suite uses a **stub Codex** implementation that mimics the real Codex behavior:

```rust
// stub_codex.rs - Simplified MCP server
fn main() {
    // Maintains in-memory state:
    // - conversations: Vec<ConversationMetadata>
    // - Supports pagination, approvals, notifications

    match method {
        "newConversation" => { /* Create & store */ }
        "listConversations" => { /* Return with pagination */ }
        "sendUserMessage" => {
            /* Respond + send notification */
        }
        "execCommandApproval" => { /* Request sent by stub */ }
        ...
    }
}
```

This allows testing all orchestrator features without requiring:
- Real Codex binary
- Authentication
- AI model access
- Network connectivity

## Future Enhancements

1. **Agent pooling** - Reuse agents across MCP clients
2. **Load balancing** - Distribute conversations across agents
3. **Persistence** - Store agent metadata across restarts
4. **Metrics** - Track agent/conversation resource usage
5. **Hot reload** - Replace agent processes without dropping conversations
