//! Diagnostic commands for debugging node persistence issues
//!
//! These commands provide insight into the database state for debugging
//! issues where nodes don't persist on some machines.

use nodespace_core::services::CreateNodeParams;
use nodespace_core::{NodeQuery, NodeService, SurrealStore};
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

/// Diagnostic info about the database state
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDiagnostics {
    /// Path to the database directory
    pub database_path: String,
    /// Whether the database directory exists
    pub database_exists: bool,
    /// Size of the database directory in bytes (if exists)
    pub database_size_bytes: Option<u64>,
    /// Total number of nodes in the database
    pub total_node_count: i64,
    /// Number of root nodes (nodes without parent)
    pub root_node_count: i64,
    /// Recent node IDs (last 10 created)
    pub recent_node_ids: Vec<String>,
    /// Schema count
    pub schema_count: i64,
    /// Last error message if any operation failed
    pub errors: Vec<String>,
}

/// Get directory size recursively
fn get_directory_size(path: &PathBuf) -> Option<u64> {
    if !path.exists() {
        return None;
    }

    let mut total_size = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_file() {
                if let Ok(metadata) = fs::metadata(&entry_path) {
                    total_size += metadata.len();
                }
            } else if entry_path.is_dir() {
                if let Some(dir_size) = get_directory_size(&entry_path) {
                    total_size += dir_size;
                }
            }
        }
    }
    Some(total_size)
}

/// Get comprehensive database diagnostics
///
/// Returns detailed information about the database state to help debug
/// persistence issues on different machines.
///
/// # Arguments
/// * `store` - SurrealStore instance from Tauri state
///
/// # Returns
/// * `DatabaseDiagnostics` - Struct with all diagnostic info
///
/// # Example Frontend Usage
/// ```typescript
/// const diagnostics = await invoke('get_database_diagnostics');
/// console.log('Database path:', diagnostics.databasePath);
/// console.log('Node count:', diagnostics.totalNodeCount);
/// ```
#[tauri::command]
pub async fn get_database_diagnostics(
    store: State<'_, Arc<SurrealStore>>,
) -> Result<DatabaseDiagnostics, String> {
    let mut errors: Vec<String> = Vec::new();

    // Get database path from environment or default
    let db_path = std::env::var("NODESPACE_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .map(|h| h.join(".nodespace").join("database").join("nodespace"))
                .unwrap_or_else(|| PathBuf::from(".nodespace/database/nodespace"))
        });

    let database_exists = db_path.exists();
    let database_size_bytes = get_directory_size(&db_path);

    // Query total node count using NodeQuery
    let total_node_count = match store
        .query_nodes(NodeQuery {
            limit: Some(10000),
            ..Default::default()
        })
        .await
    {
        Ok(nodes) => nodes.len() as i64,
        Err(e) => {
            errors.push(format!("Failed to count nodes: {}", e));
            0
        }
    };

    // Query root node count (nodes without parent - use get_children with None parent)
    let root_node_count = match store.get_children(None).await {
        Ok(nodes) => nodes.len() as i64,
        Err(e) => {
            errors.push(format!("Failed to count root nodes: {}", e));
            0
        }
    };

    // Get recent node IDs - use query with limit, sorted by created_at desc would be ideal
    // For now just get some node IDs
    let recent_node_ids = match store
        .query_nodes(NodeQuery {
            limit: Some(10),
            ..Default::default()
        })
        .await
    {
        Ok(nodes) => nodes.iter().map(|n| n.id.clone()).collect(),
        Err(e) => {
            errors.push(format!("Failed to get recent nodes: {}", e));
            Vec::new()
        }
    };

    // Count schemas - query for node_type = 'schema'
    let schema_count = match store
        .query_nodes(NodeQuery {
            node_type: Some("schema".to_string()),
            limit: Some(100),
            ..Default::default()
        })
        .await
    {
        Ok(nodes) => nodes.len() as i64,
        Err(e) => {
            errors.push(format!("Failed to count schemas: {}", e));
            0
        }
    };

    Ok(DatabaseDiagnostics {
        database_path: db_path.to_string_lossy().to_string(),
        database_exists,
        database_size_bytes,
        total_node_count,
        root_node_count,
        recent_node_ids,
        schema_count,
        errors,
    })
}

/// Result of a persistence test
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestPersistenceResult {
    /// The test node ID that was used
    pub test_id: String,
    /// Whether create_node succeeded
    pub created: bool,
    /// The ID returned from create_node
    pub created_id: Option<String>,
    /// Error from create if failed
    pub create_error: Option<String>,
    /// Whether the node was found when reading back
    pub verified: bool,
    /// Error from verify if failed
    pub verify_error: Option<String>,
    /// Whether the content matched what we wrote
    pub content_matched: bool,
    /// Whether cleanup (delete) succeeded
    pub cleanup_success: bool,
}

/// Test node creation and verification - creates a test node and verifies it persisted
///
/// This command creates a test node, immediately reads it back, and returns
/// diagnostic info about the operation. Useful for testing if persistence works.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
///
/// # Returns
/// * Result with creation and verification details
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('test_node_persistence');
/// console.log('Created:', result.created);
/// console.log('Verified:', result.verified);
/// ```
#[tauri::command]
pub async fn test_node_persistence(
    service: State<'_, NodeService>,
) -> Result<TestPersistenceResult, String> {
    let test_id = format!("diagnostic-test-{}", uuid::Uuid::new_v4());
    let test_content = format!("Diagnostic test node created at {}", chrono::Utc::now());

    // Step 1: Create a test node
    let create_result = service
        .create_node_with_parent(CreateNodeParams {
            id: Some(test_id.clone()),
            node_type: "text".to_string(),
            content: test_content.clone(),
            parent_id: None,
            insert_after_node_id: None,
            properties: serde_json::json!({}),
        })
        .await;

    let created = match &create_result {
        Ok(id) => Some(id.clone()),
        Err(e) => {
            return Ok(TestPersistenceResult {
                test_id: test_id.clone(),
                created: false,
                created_id: None,
                create_error: Some(format!("{}", e)),
                verified: false,
                verify_error: Some("Skipped - create failed".to_string()),
                content_matched: false,
                cleanup_success: false,
            });
        }
    };

    // Step 2: Immediately read it back
    let verify_result = service.get_node(&test_id).await;

    let (verified, verify_error, content_matched) = match verify_result {
        Ok(Some(node)) => {
            let matched = node.content == test_content;
            (true, None, matched)
        }
        Ok(None) => (
            false,
            Some("Node not found after creation!".to_string()),
            false,
        ),
        Err(e) => (false, Some(format!("Verify error: {}", e)), false),
    };

    // Step 3: Clean up test node
    let cleanup_success = if verified {
        // Try to delete the test node
        service.delete_node_unchecked(&test_id).await.is_ok()
    } else {
        false
    };

    Ok(TestPersistenceResult {
        test_id,
        created: true,
        created_id: created,
        create_error: None,
        verified,
        verify_error,
        content_matched,
        cleanup_success,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostics_serialization() {
        let diag = DatabaseDiagnostics {
            database_path: "/test/path".to_string(),
            database_exists: true,
            database_size_bytes: Some(1024),
            total_node_count: 10,
            root_node_count: 2,
            recent_node_ids: vec!["id1".to_string(), "id2".to_string()],
            schema_count: 5,
            errors: vec![],
        };

        let json = serde_json::to_string(&diag).unwrap();
        assert!(json.contains("databasePath"));
        assert!(json.contains("totalNodeCount"));
    }
}
