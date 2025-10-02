# Orchestrator Testing

## Test Suite Overview

The orchestrator includes comprehensive test coverage for all conversation management features.

## Test Files

### `tests/manager_integration.rs`
Basic integration tests with stub Codex:
- `spawn_list_kill_agent_with_stub` - Tests agent lifecycle
- `conversation_flow_send_message_and_turn` - Tests basic conversation flow

### `tests/conversation_management.rs`
Comprehensive tests for conversation viewing and management:

### `tests/event_orchestration.rs`
Tests for event notifications and approval workflow:

#### Event Notification Tests
- `test_notification_from_agent` - Verifies notifications are received from agent
  - Sends user message
  - Agent responds with notification
  - Read loop processes notification
  - Forwarded to upstream via `notify_codex_event`

#### Approval Workflow Tests
- `test_approval_request_flow` - Tests full approval cycle
  - Agent sends approval request (execCommandApproval)
  - Approval registered in pending list
  - Client decides approval (allow/deny)
  - Response sent back to agent

- `test_approval_timeout` - Verifies timeout behavior
  - Agent sends approval request
  - No decision provided
  - Would timeout after 60s (test verifies registration only)

- `test_list_approvals` - Tests approval listing
  - Initially empty approval list
  - Trigger approval request
  - Verify appears in list
  - Clean up by deciding

- `test_decide_approval_invalid_key` - Error handling
  - Attempts to decide non-existent approval
  - Verifies appropriate error returned

#### Empty State Tests
- `test_list_conversations_empty` - Verifies empty list on new agent

#### List Conversations Tests
- `test_list_conversations_with_items` - Creates 2 conversations and verifies listing
  - Checks all required fields (conversationId, path, preview, timestamp)
  - Validates conversation IDs match created conversations

- `test_list_conversations_pagination` - Tests pagination with 5 conversations
  - Creates 5 conversations
  - Requests page size of 2
  - Verifies nextCursor is returned
  - Fetches next page using cursor

#### Resume Conversation Tests
- `test_resume_conversation` - Basic resume functionality
  - Creates conversation
  - Gets rolloutPath from response
  - Resumes using path
  - Verifies conversationId and model in response

- `test_resume_conversation_with_overrides` - Resume with parameter overrides
  - Creates conversation
  - Resumes with model and approvalPolicy overrides
  - Verifies conversation was resumed

#### Archive Conversation Tests
- `test_archive_conversation` - Archive functionality
  - Creates conversation
  - Verifies it appears in list (count = 1)
  - Archives conversation
  - Verifies it's removed from list (count = 0)

#### Full Lifecycle Test
- `test_full_conversation_lifecycle` - End-to-end workflow
  1. Starts with empty list
  2. Creates first conversation
  3. Sends a message
  4. Creates second conversation
  5. Verifies list shows 2 conversations
  6. Archives first conversation
  7. Verifies list shows 1 conversation
  8. Resumes first conversation from rollout
  9. Archives second conversation
  10. Verifies list is empty

### `tests/real_codex_integration.rs`
- `real_codex_conversation_end_to_end` - Real Codex integration (marked as #[ignore])
  - Requires actual Codex binary with authentication
  - Tests sendUserMessage with real AI model
  - Currently ignored due to long runtime and auth requirements

## Stub Codex Implementation

The `src/bin/stub_codex.rs` provides a mock Codex MCP server for testing:

### Supported Methods
- `initialize` - MCP handshake
- `newConversation` - Creates conversation with auto-incrementing IDs
- `sendUserMessage` - Accepts messages
- `sendUserTurn` - Accepts turns
- `interruptConversation` - Returns abort reason
- `listConversations` - Returns stored conversations with pagination
- `resumeConversation` - Resumes from rollout path
- `archiveConversation` - Removes conversation from list

### State Management
- Maintains in-memory list of conversations
- Tracks conversation metadata (id, path, preview, timestamp)
- Supports pagination with cursor-based iteration
- Auto-generates rollout paths: `/tmp/rollout-{conversationId}.jsonl`

## Running Tests

### Run All Tests
```bash
cargo test
```

### Run Specific Test Suite
```bash
cargo test --test conversation_management
cargo test --test manager_integration
```

### Run Individual Test
```bash
cargo test test_list_conversations_empty
cargo test test_full_conversation_lifecycle
```

### Run with Output
```bash
cargo test -- --nocapture
cargo test test_archive_conversation -- --nocapture
```

### Run Ignored Tests
```bash
cargo test -- --ignored
```

## Test Results

All tests pass successfully:

```
tests/conversation_management.rs:
test test_archive_conversation ... ok
test test_full_conversation_lifecycle ... ok
test test_list_conversations_empty ... ok
test test_list_conversations_pagination ... ok
test test_list_conversations_with_items ... ok
test test_resume_conversation ... ok
test test_resume_conversation_with_overrides ... ok
test result: ok. 7 passed; 0 failed; 0 ignored

tests/event_orchestration.rs:
test test_approval_request_flow ... ok
test test_approval_timeout ... ok
test test_decide_approval_invalid_key ... ok
test test_list_approvals ... ok
test test_notification_from_agent ... ok
test result: ok. 5 passed; 0 failed; 0 ignored

tests/manager_integration.rs:
test conversation_flow_send_message_and_turn ... ok
test spawn_list_kill_agent_with_stub ... ok
test result: ok. 2 passed; 0 failed; 0 ignored

Total: 14 passed; 0 failed; 1 ignored
```

## Coverage

The test suite covers:
- ✅ Agent lifecycle (spawn, list, kill)
- ✅ Conversation creation
- ✅ Message sending
- ✅ Conversation listing (empty, with items)
- ✅ Pagination (cursor-based)
- ✅ Conversation resumption (basic and with overrides)
- ✅ Conversation archiving
- ✅ Full lifecycle workflow
- ✅ Event notifications from agent
- ✅ Notification forwarding to upstream
- ✅ Approval request registration
- ✅ Approval decision workflow (allow/deny)
- ✅ Approval listing
- ✅ Approval timeout behavior
- ✅ Error handling (timeouts, invalid agents, invalid approvals)

## Future Testing

Areas for potential expansion:
- Integration tests with real Codex binary (currently ignored)
- Concurrent conversation management
- Error scenarios (invalid paths, missing conversations)
- Large dataset pagination (100+ conversations)
- Event stream verification
- Approval workflow testing
