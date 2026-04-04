#!/bin/bash
# Mock ACP agent for E2E testing.
#
# Speaks JSON-RPC 2.0 over NDJSON (stdin/stdout):
# - Responds to "initialize" with capabilities
# - Responds to "session/new" with acknowledgement
# - Echoes "session/prompt" messages back as assistant responses
# - Exits cleanly on stdin EOF

while IFS= read -r line; do
    # Extract id and method using python3 (available on macOS and most Linux)
    id=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('id','null'))" 2>/dev/null)
    method=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('method',''))" 2>/dev/null)

    case "$method" in
        initialize)
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"1\",\"capabilities\":{\"streaming\":false}}}"
            ;;
        session/new)
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"mock-session-1\"}}"
            ;;
        session/prompt)
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"role\":\"assistant\",\"content\":\"Mock response from test agent\"}}"
            ;;
        *)
            echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"method\":\"$method\",\"echo\":true}}"
            ;;
    esac
done
