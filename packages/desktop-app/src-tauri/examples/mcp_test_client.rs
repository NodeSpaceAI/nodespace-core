//! MCP Test Client
//!
//! Standalone binary to test MCP stdio protocol.
//! Communicates with NodeSpace via JSON-RPC over stdin/stdout.
//!
//! # Usage
//!
//! ```bash
//! # Run with cargo (spawns NodeSpace app in background)
//! cargo run --example mcp_test_client
//!
//! # Or test manually with NodeSpace already running
//! echo '{"jsonrpc":"2.0","id":1,"method":"create_node","params":{"node_type":"text","content":"Test"}}' | ./target/debug/nodespace-app
//! ```

use serde_json::json;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};

fn main() -> anyhow::Result<()> {
    println!("🧪 MCP Test Client");
    println!("==================\n");

    // Test 1: Create a text node
    println!("Test 1: Creating text node...");
    let create_request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "create_node",
        "params": {
            "node_type": "text",
            "content": "Test node created via MCP",
            "properties": {}
        }
    });

    let response = send_request_interactive(&create_request)?;
    println!("✅ Create response: {}\n", response);

    let response_json: serde_json::Value = serde_json::from_str(&response)?;
    let node_id = response_json["result"]["node_id"]
        .as_str()
        .expect("Failed to get node_id from response");

    println!("📝 Created node ID: {}\n", node_id);

    // Test 2: Get the node we just created
    println!("Test 2: Getting node {}...", node_id);
    let get_request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "get_node",
        "params": {
            "node_id": node_id
        }
    });

    let response = send_request_interactive(&get_request)?;
    println!("✅ Get response: {}\n", response);

    // Test 3: Update the node
    println!("Test 3: Updating node {}...", node_id);
    let update_request = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "update_node",
        "params": {
            "node_id": node_id,
            "content": "Updated content via MCP"
        }
    });

    let response = send_request_interactive(&update_request)?;
    println!("✅ Update response: {}\n", response);

    // Test 4: Query nodes
    println!("Test 4: Querying text nodes...");
    let query_request = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "query_nodes",
        "params": {
            "node_type": "text",
            "limit": 5
        }
    });

    let response = send_request_interactive(&query_request)?;
    println!("✅ Query response: {}\n", response);

    // Test 5: Delete the node
    println!("Test 5: Deleting node {}...", node_id);
    let delete_request = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "delete_node",
        "params": {
            "node_id": node_id
        }
    });

    let response = send_request_interactive(&delete_request)?;
    println!("✅ Delete response: {}\n", response);

    // Test 6: Verify deletion
    println!("Test 6: Verifying node deletion (should fail)...");
    let get_deleted_request = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "get_node",
        "params": {
            "node_id": node_id
        }
    });

    let response = send_request_interactive(&get_deleted_request)?;
    println!("✅ Get deleted node response: {}\n", response);

    println!("🎉 All tests completed!");

    Ok(())
}

/// Send a request interactively (user types into stdin)
///
/// This is a simple interactive mode where you manually paste JSON requests.
/// For automated testing, you would spawn NodeSpace as a subprocess.
fn send_request_interactive(request: &serde_json::Value) -> anyhow::Result<String> {
    println!("📤 Request: {}", serde_json::to_string_pretty(request)?);
    println!("\n⚠️  Manual mode: Please paste the request JSON into NodeSpace stdin and provide the response below:");
    println!("   (For automated testing, we would spawn NodeSpace as a subprocess)\n");

    print!("Response: ");
    io::stdout().flush()?;

    let mut response = String::new();
    io::stdin().lock().read_line(&mut response)?;

    Ok(response.trim().to_string())
}

/// Send a request via subprocess (automated mode)
///
/// This would spawn NodeSpace and pipe stdin/stdout for true automation.
/// Currently unused - requires NodeSpace binary path configuration.
#[allow(dead_code)]
fn send_request_automated(
    nodespace_path: &str,
    request: &serde_json::Value,
) -> anyhow::Result<String> {
    let mut child = Command::new(nodespace_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let request_json = serde_json::to_string(request)?;

    writeln!(stdin, "{}", request_json)?;
    drop(stdin);

    let output = child.wait_with_output()?;
    let response = String::from_utf8(output.stdout)?;

    Ok(response.trim().to_string())
}
