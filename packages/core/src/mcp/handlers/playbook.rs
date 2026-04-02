//! Playbook MCP Handlers
//!
//! MCP tool handlers for playbook/workflow operations.
//! These handlers share the CEL evaluation infrastructure with the engine
//! but operate independently (read-only, no action execution).

use crate::mcp::types::MCPError;
use crate::playbook::cel;
use crate::playbook::graph_resolver::GraphResolver;
use crate::playbook::types::*;
use crate::services::NodeService;
use serde_json::{json, Value};
use std::sync::Arc;

/// Handle `get_workflow_state` — evaluate a node against active playbook rules.
///
/// Queries active playbook nodes from the database, parses their rules, and
/// evaluates conditions against the specified node. Returns which conditions
/// pass/fail for each matching rule.
///
/// This is read-only — no actions are executed, no playbooks are modified.
/// Uses the same CEL evaluation + graph traversal infrastructure as the engine.
///
/// Parameters:
/// - `node_id` (required): The node to evaluate
///
/// Returns: Array of rule results showing condition pass/fail status.
pub async fn handle_get_workflow_state(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let node_id = params["node_id"]
        .as_str()
        .ok_or_else(|| MCPError::invalid_params("Missing 'node_id' parameter".to_string()))?;

    // Fetch the target node
    let node = node_service
        .get_node(node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to fetch node: {}", e)))?
        .ok_or_else(|| MCPError::invalid_params(format!("Node '{}' not found", node_id)))?;

    // Query all active playbook nodes
    let playbook_nodes = node_service
        .query_nodes_by_type("playbook", Some("active"))
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to query playbooks: {}", e)))?;

    // Parse rules and find those matching this node's type
    let mut matched_rules: Vec<(String, usize, ParsedRule)> = Vec::new();

    for pb_node in &playbook_nodes {
        let rule_defs = match parse_rules_from_properties(&pb_node.properties) {
            Ok(defs) => defs,
            Err(_) => continue, // Skip unparseable playbooks
        };

        for (idx, def) in rule_defs.iter().enumerate() {
            let parsed = match parse_rule(def) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let matches = match &parsed.trigger {
                ParsedTrigger::GraphEvent { node_type, .. } => *node_type == node.node_type,
                ParsedTrigger::Scheduled { node_type, .. } => *node_type == node.node_type,
            };

            if matches {
                matched_rules.push((pb_node.id.clone(), idx, parsed));
            }
        }
    }

    // Evaluate conditions using a synthetic event
    let synthetic_event = crate::db::events::DomainEvent::NodeCreated {
        node_type: node.node_type.clone(),
        node_id: node.id.clone(),
    };

    let mut resolver = GraphResolver::new(Arc::clone(node_service));
    let mut results = Vec::new();

    for (playbook_id, rule_index, rule) in &matched_rules {
        let mut condition_results = Vec::new();

        for (cond_idx, condition) in rule.conditions.iter().enumerate() {
            let result = cel::evaluate_conditions(
                &[condition.clone()],
                &node,
                &synthetic_event,
                Some(&mut resolver),
            );

            let (passed, message) = match result {
                cel::ConditionResult::Pass => (true, "passed".to_string()),
                cel::ConditionResult::Fail { .. } => (false, "failed".to_string()),
                cel::ConditionResult::CompileError { message, .. } => {
                    (false, format!("compile error: {}", message))
                }
            };

            condition_results.push(json!({
                "index": cond_idx,
                "expression": condition,
                "passed": passed,
                "message": message,
            }));
        }

        let all_passed = condition_results
            .iter()
            .all(|c| c["passed"].as_bool().unwrap_or(false));

        results.push(json!({
            "playbook_id": playbook_id,
            "rule_index": rule_index,
            "rule_name": rule.name,
            "all_conditions_passed": all_passed,
            "conditions": condition_results,
        }));
    }

    Ok(json!({
        "node_id": node_id,
        "node_type": node.node_type,
        "matched_rules": results.len(),
        "rules": results,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SurrealStore;
    use crate::models::Node;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_service() -> (Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store: Arc<SurrealStore> = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
        (node_service, temp_dir)
    }

    async fn create_schema(svc: &Arc<NodeService>, type_name: &str) {
        let schema_node = Node::new_with_id(
            type_name.to_string(),
            "schema".to_string(),
            type_name.to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": format!("{} schema", type_name),
                "fields": [{"name": "status", "type": "string"}],
                "relationships": []
            }),
        );
        svc.create_node(schema_node).await.unwrap();
    }

    #[tokio::test]
    async fn missing_node_id_returns_error() {
        let (svc, _tmp) = create_test_service().await;
        let result = handle_get_workflow_state(&svc, json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nonexistent_node_returns_error() {
        let (svc, _tmp) = create_test_service().await;
        let result = handle_get_workflow_state(&svc, json!({"node_id": "nonexistent"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn node_with_no_matching_playbooks_returns_empty() {
        let (svc, _tmp) = create_test_service().await;
        create_schema(&svc, "wf_widget").await;

        let node = Node::new("wf_widget".to_string(), "test widget".to_string(), json!({}));
        let node_id = node.id.clone();
        svc.create_node(node).await.unwrap();

        let result = handle_get_workflow_state(&svc, json!({"node_id": node_id}))
            .await
            .unwrap();
        assert_eq!(result["matched_rules"], 0);
        assert_eq!(result["rules"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn evaluates_matching_playbook_conditions() {
        let (svc, _tmp) = create_test_service().await;
        create_schema(&svc, "wf_task").await;

        // Create a playbook that triggers on wf_task
        let playbook = Node::new(
            "playbook".to_string(),
            "test playbook".to_string(),
            json!({
                "rules": [{
                    "name": "check-status",
                    "trigger": {
                        "type": "graph_event",
                        "on": "node_created",
                        "node_type": "wf_task"
                    },
                    "conditions": ["node.status == 'open'"],
                    "actions": []
                }]
            }),
        );
        svc.create_node(playbook).await.unwrap();

        // Create a matching node
        let node = Node::new(
            "wf_task".to_string(),
            "my task".to_string(),
            json!({"status": "open"}),
        );
        let node_id = node.id.clone();
        svc.create_node(node).await.unwrap();

        let result = handle_get_workflow_state(&svc, json!({"node_id": node_id}))
            .await
            .unwrap();
        assert_eq!(result["matched_rules"], 1);

        let rule = &result["rules"][0];
        assert_eq!(rule["rule_name"], "check-status");
        assert_eq!(rule["all_conditions_passed"], true);
        assert_eq!(rule["conditions"][0]["passed"], true);
    }

    #[tokio::test]
    async fn reports_failing_conditions() {
        let (svc, _tmp) = create_test_service().await;
        create_schema(&svc, "wf_task2").await;

        let playbook = Node::new(
            "playbook".to_string(),
            "test playbook".to_string(),
            json!({
                "rules": [{
                    "name": "check-done",
                    "trigger": {
                        "type": "graph_event",
                        "on": "node_created",
                        "node_type": "wf_task2"
                    },
                    "conditions": ["node.status == 'done'"],
                    "actions": []
                }]
            }),
        );
        svc.create_node(playbook).await.unwrap();

        let node = Node::new(
            "wf_task2".to_string(),
            "my task".to_string(),
            json!({"status": "open"}),
        );
        let node_id = node.id.clone();
        svc.create_node(node).await.unwrap();

        let result = handle_get_workflow_state(&svc, json!({"node_id": node_id}))
            .await
            .unwrap();
        assert_eq!(result["matched_rules"], 1);

        let rule = &result["rules"][0];
        assert_eq!(rule["all_conditions_passed"], false);
        assert_eq!(rule["conditions"][0]["passed"], false);
    }
}
