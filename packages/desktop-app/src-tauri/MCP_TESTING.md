# MCP stdio Server Testing Guide

## Overview

The MCP (Model Context Protocol) stdio server enables AI agents to interact with NodeSpace programmatically via JSON-RPC 2.0 over stdin/stdout.

## Architecture

- **Single-process**: MCP runs as an async Tokio task within the Tauri app
- **Shared NodeService**: Uses the same NodeService instance as Tauri commands
- **Database**: Real Turso database at `~/.nodespace/database/nodespace-dev.db`
- **Tauri Events**: Operations emit events for UI reactivity

## Available Methods

### 1. create_node

Create a new node in the database.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "create_node",
  "params": {
    "node_type": "text",
    "content": "My new note",
    "parent_id": null,
    "container_node_id": null,
    "before_sibling_id": null,
    "properties": {}
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "node_id": "uuid-here",
    "success": true
  }
}
```

### 2. get_node

Retrieve a node by ID.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "get_node",
  "params": {
    "node_id": "uuid-here"
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "id": "uuid-here",
    "node_type": "text",
    "content": "My new note",
    "parent_id": null,
    "container_node_id": null,
    "before_sibling_id": null,
    "created_at": "2025-01-19T10:30:00Z",
    "modified_at": "2025-01-19T10:30:00Z",
    "properties": {},
    "embedding_vector": null,
    "mentions": [],
    "mentioned_by": []
  }
}
```

### 3. update_node

Update node content, type, or properties.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "update_node",
  "params": {
    "node_id": "uuid-here",
    "content": "Updated content",
    "node_type": "text",
    "properties": {
      "custom_field": "value"
    }
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "success": true
  }
}
```

### 4. delete_node

Delete a node by ID.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "delete_node",
  "params": {
    "node_id": "uuid-here"
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "result": {
    "success": true
  }
}
```

### 5. query_nodes

Query nodes with filtering.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "query_nodes",
  "params": {
    "node_type": "text",
    "parent_id": null,
    "container_node_id": null,
    "limit": 10,
    "offset": 0
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "result": {
    "nodes": [...],
    "count": 3
  }
}
```

## Error Handling

**Error Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32600,
    "message": "Invalid request"
  }
}
```

**Error Codes:**
- `-32700`: Parse error (invalid JSON)
- `-32600`: Invalid request
- `-32601`: Method not found
- `-32602`: Invalid params
- `-32603`: Internal error
- `-32000`: Node not found (NodeSpace-specific)
- `-32001`: Node creation failed
- `-32002`: Node update failed
- `-32003`: Node delete failed
- `-32004`: Validation error

## Testing Methods

### Method 1: Manual Testing with echo

```bash
# Start NodeSpace in MCP mode
./target/debug/nodespace-app

# In another terminal, send a request
echo '{"jsonrpc":"2.0","id":1,"method":"create_node","params":{"node_type":"text","content":"Test"}}' | nc localhost 3000
```

### Method 2: Test Client (Interactive)

```bash
# Build and run the test client
cargo run --example mcp_test_client
```

The test client will prompt you to manually provide responses. This is useful for understanding the protocol flow.

### Method 3: Automated Testing (Future)

For true automation, you would:

1. Build the NodeSpace app
2. Spawn it as a subprocess with stdio piping
3. Send JSON-RPC requests programmatically
4. Parse responses

```rust
use std::process::{Command, Stdio};
use std::io::Write;

let mut child = Command::new("./target/debug/nodespace-app")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .spawn()
    .unwrap();

let stdin = child.stdin.as_mut().unwrap();
stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"get_node\",\"params\":{\"node_id\":\"test\"}}\n").unwrap();

// Read from child.stdout...
```

## Tauri Events

When MCP operations are performed, the following Tauri events are emitted for UI reactivity:

- `node-created`: When a node is created
  ```json
  {
    "node_id": "uuid-here",
    "node_type": "text"
  }
  ```

- `node-updated`: When a node is updated
  ```json
  {
    "node_id": "uuid-here"
  }
  ```

- `node-deleted`: When a node is deleted
  ```json
  {
    "node_id": "uuid-here"
  }
  ```

## Debugging

Enable debug logging:

```bash
RUST_LOG=debug ./target/debug/nodespace-app
```

MCP logs will show:
- `📥 MCP request: {...}` - Incoming requests
- `📤 MCP response for method 'X' (id=Y)` - Outgoing responses
- `✅ MCP request X succeeded` - Successful operations
- `❌ MCP request X failed: ...` - Failed operations

## Integration with AI Agents

### Claude Code Configuration

Add to your Claude Code MCP configuration:

```json
{
  "mcpServers": {
    "nodespace": {
      "command": "/path/to/NodeSpace.app/Contents/MacOS/NodeSpace",
      "args": [],
      "transport": "stdio"
    }
  }
}
```

### Example AI Agent Usage

```typescript
// AI agent spawns NodeSpace with stdio piping
const nodespace = spawn('/path/to/NodeSpace', [], {
  stdio: ['pipe', 'pipe', 'inherit']
});

// Send JSON-RPC request
nodespace.stdin.write(JSON.stringify({
  jsonrpc: "2.0",
  id: 1,
  method: "create_node",
  params: {
    node_type: "task",
    content: "AI-generated task",
    properties: {}
  }
}) + '\n');

// Read response from stdout
nodespace.stdout.on('data', (data) => {
  const response = JSON.parse(data.toString());
  console.log('Created node:', response.result.node_id);
});
```

## Known Limitations

1. **Hierarchy updates**: Parent/container/ordering updates are not exposed via MCP (use Tauri commands for UI operations)
2. **Embedding updates**: Embedding vector updates not supported through MCP
3. **No authentication**: Development mode has no authentication (production will require auth)
4. **Single connection**: stdio protocol supports one connection at a time

## Next Steps

- Phase 2: Implement `create_nodes_from_markdown` for bulk imports
- Phase 3: Add natural language query processing
- Phase 4: Implement authentication and permissions
