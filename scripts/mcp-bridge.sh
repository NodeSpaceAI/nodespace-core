#!/usr/bin/env bash
# MCP Bridge - Translates stdio (Claude Code) to HTTP (NodeSpace)
#
# This script reads JSON-RPC 2.0 messages from stdin (from Claude Code),
# POSTs them to the NodeSpace MCP HTTP server, and writes responses to stdout.
#
# Usage: Configure Claude Code to use this script as MCP server
# Example ~/.claude.json:
#   {
#     "mcpServers": {
#       "nodespace": {
#         "type": "stdio",
#         "command": "/path/to/scripts/mcp-bridge.sh"
#       }
#     }
#   }

set -euo pipefail

# Check dependencies
if ! command -v curl &> /dev/null; then
    echo "[ERROR] curl is required but not found. Please install curl." >&2
    exit 1
fi

# Configuration
MCP_SERVER_URL="${MCP_SERVER_URL:-http://localhost:3001/mcp}"
CONNECT_TIMEOUT="${CONNECT_TIMEOUT:-5}"
REQUEST_TIMEOUT="${REQUEST_TIMEOUT:-30}"

# Colors for logging (stderr)
RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

# Log to stderr
log_debug() {
    if [[ "${DEBUG:-}" == "1" ]]; then
        echo -e "${YELLOW}[DEBUG]${NC} $*" >&2
    fi
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*" >&2
}

log_info() {
    if [[ "${DEBUG:-}" == "1" ]]; then
        echo -e "${GREEN}[INFO]${NC} $*" >&2
    fi
}

# Extract JSON field without jq
# Usage: extract_json_field "$json" "fieldname"
extract_json_field() {
    local json="$1"
    local field="$2"

    # Use sed to extract the field value
    # Handles: "field": value, "field":value, "field" : value
    echo "$json" | sed -n 's/.*"'"$field"'"[[:space:]]*:[[:space:]]*\([^,}]*\).*/\1/p' | head -n 1
}

# Read stdin line by line and forward to HTTP server
while IFS= read -r line; do
    if [[ -z "$line" ]]; then
        log_debug "Empty line, skipping"
        continue
    fi

    log_debug "Request: $line"

    # Forward to HTTP server and capture response
    response=$(curl \
        --silent \
        --show-error \
        --fail \
        --connect-timeout "$CONNECT_TIMEOUT" \
        --max-time "$REQUEST_TIMEOUT" \
        --request POST \
        --header "Content-Type: application/json" \
        --data "$line" \
        "$MCP_SERVER_URL" 2>&1) || {
        error_code=$?
        log_error "Failed to connect to NodeSpace at $MCP_SERVER_URL (exit code: $error_code)"

        # Extract JSON-RPC id from request for error response (without jq)
        request_id=$(extract_json_field "$line" "id")
        # If extraction failed or id is empty, use null
        [[ -z "$request_id" ]] && request_id="null"

        # Send JSON-RPC error response
        echo "{\"jsonrpc\":\"2.0\",\"id\":$request_id,\"error\":{\"code\":-32603,\"message\":\"Failed to connect to NodeSpace MCP server at $MCP_SERVER_URL\"}}"
        continue
    }

    log_debug "Response: $response"

    # Send response to stdout
    echo "$response"
done

log_info "MCP bridge closed"
