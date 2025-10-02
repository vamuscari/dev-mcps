#!/bin/bash
set -e

# Run orchestrator with debug logging and test it
RUST_LOG=debug cargo run --release -p codex-orchestrator &
ORCH_PID=$!

sleep 2

echo "Testing orchestrator with MCP client..."
# Send test commands via stdin
{
  echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
  sleep 1
  echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}'
  sleep 1
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"spawn_agent","arguments":{"id":"test-debug"}}}'
  sleep 2
  echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"new_conversation","arguments":{"agentId":"test-debug","params":{"prompt":"Say hello"}}}}'
  sleep 5
} | nc -U /dev/stdin 2>&1 | head -50

kill $ORCH_PID 2>/dev/null || true
