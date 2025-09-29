#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT=$(git -C "${BASH_SOURCE[0]%/*}" rev-parse --show-toplevel 2>/dev/null || pwd)

URI=${1:-"file://${REPO_ROOT}/dap/src/main.rs"}
SERVER_CMD=${SERVER_CMD:-rust-analyzer}
SERVER_CMD_JSON=${SERVER_CMD//"/\\"}

STDOUT_LOG=${STDOUT_LOG:-stdout.log}
STDERR_LOG=${STDERR_LOG:-stderr.log}

SERVER_BIN=${SERVER_CMD%% *}
echo "Running codex-lsp against LSP command: $SERVER_CMD" >&2
if ! command -v "$SERVER_BIN" >/dev/null 2>&1; then
  echo "warning: command not found: $SERVER_BIN" >&2
else
  echo "--- $SERVER_BIN --version ---" >&2
  "$SERVER_BIN" --version >&2 || echo "(version check exited $?)" >&2
fi

set +e
python3 - "$URI" "$SERVER_CMD_JSON" <<'PY' | \
  cargo run -p codex-lsp --bin codex-lsp >"$STDOUT_LOG" 2>"$STDERR_LOG"
import json
import sys

uri, server_cmd = sys.argv[1:]
messages = [
    {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "run_lsp_symbols", "version": "1"},
        },
    },
    {
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "lsp_document_symbol",
            "arguments": {"uri": uri, "serverCommand": server_cmd},
        },
    },
    {"jsonrpc": "2.0", "id": 3, "method": "shutdown"},
    {"jsonrpc": "2.0", "method": "exit"},
]

for msg in messages:
    data = json.dumps(msg)
    sys.stdout.write(f"Content-Length: {len(data)}\r\n\r\n{data}")
PY
STATUS=$?
set -e

echo "codex-lsp exit status: $STATUS"

echo "--- codex-lsp stdout ($STDOUT_LOG) ---"
cat "$STDOUT_LOG"

echo "--- codex-lsp stderr ($STDERR_LOG) ---" >&2
cat "$STDERR_LOG" >&2

if grep -q "codex-lsp:" "$STDERR_LOG"; then
  echo "Detected codex-lsp error lines:" >&2
  grep "codex-lsp:" "$STDERR_LOG" >&2
fi

exit "$STATUS"
