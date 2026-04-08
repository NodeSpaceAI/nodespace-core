//! Integration tests for SkillUpdater (Issue #1061).
//!
//! Verifies that when schemas are created or deleted, the "Node Creation"
//! skill node's description is updated to include the new type names,
//! enabling semantic search to find the skill when users ask for
//! "create a new <custom_type>".

#[cfg(test)]
mod skill_updater_tests {
    use anyhow::Result;
    use nodespace_core::db::SurrealStore;
    use nodespace_core::models::Node;
    use nodespace_core::ops::skill_updater::{build_node_creation_description, SkillUpdater};
    use nodespace_core::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::time::{timeout, Duration};

    async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SurrealStore::new(db_path).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);
        Ok((service, temp_dir))
    }

    /// Create the "Node Creation" skill node in the test DB.
    async fn seed_node_creation_skill(service: &NodeService) -> Result<Node> {
        let mut node = Node::new(
            "skill".to_string(),
            "Node Creation".to_string(),
            json!({
                "description": "Create new instances of existing node types — add a task, text note, or an entry for a custom type. Use when user wants to add a new record or item.",
                "tool_whitelist": ["create_node", "get_node"],
                "max_iterations": 2,
                "output_format": "text"
            }),
        );
        node.title = Some("Node Creation".to_string());
        service.create_node(node.clone()).await?;
        let created = service
            .get_node(&node.id)
            .await?
            .expect("skill node should exist");
        Ok(created)
    }

    /// Create a minimal schema node for testing.
    async fn create_schema_node(service: &NodeService, schema_id: &str) -> Result<Node> {
        let mut node = Node::new(
            "schema".to_string(),
            schema_id.to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": format!("Custom {} type", schema_id),
                "fields": [],
                "relationships": []
            }),
        );
        node.id = schema_id.to_string();
        service.create_node(node.clone()).await?;
        let created = service
            .get_node(schema_id)
            .await?
            .expect("schema should exist");
        Ok(created)
    }

    #[tokio::test]
    async fn test_skill_updater_updates_description_on_schema_created() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        // Seed the "Node Creation" skill
        let skill = seed_node_creation_skill(&service).await?;

        // Start SkillUpdater
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let updater = Arc::new(SkillUpdater::new(Arc::clone(&service)));
        let updater_task = {
            let updater = Arc::clone(&updater);
            tokio::spawn(async move { updater.start(shutdown_rx).await })
        };

        // Create a schema — this emits a NodeCreated event with node_type="schema"
        create_schema_node(&service, "invoice").await?;

        // Allow time for the SkillUpdater to react (event is async)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Verify the skill description was updated
        let updated_skill = service
            .get_node(&skill.id)
            .await?
            .expect("skill node should still exist");

        let desc = updated_skill
            .properties
            .get("description")
            .or_else(|| {
                updated_skill
                    .properties
                    .get("skill")
                    .and_then(|s| s.get("description"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("");

        assert!(
            desc.contains("invoice"),
            "Skill description should mention 'invoice' after schema creation, got: {:?}",
            desc
        );

        // Shutdown
        let _ = shutdown_tx.send(true);
        let _ = timeout(Duration::from_millis(500), updater_task).await;

        Ok(())
    }

    #[tokio::test]
    async fn test_skill_updater_adds_multiple_schemas() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        seed_node_creation_skill(&service).await?;

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let updater = Arc::new(SkillUpdater::new(Arc::clone(&service)));
        let updater_task = {
            let updater = Arc::clone(&updater);
            tokio::spawn(async move { updater.start(shutdown_rx).await })
        };

        // Create two schemas
        create_schema_node(&service, "invoice").await?;
        create_schema_node(&service, "customer").await?;

        tokio::time::sleep(Duration::from_millis(300)).await;

        // Fetch any skill node
        let skill_nodes = service
            .query_nodes_simple(nodespace_core::models::NodeQuery {
                node_type: Some("skill".to_string()),
                content_contains: Some("Node Creation".to_string()),
                ..Default::default()
            })
            .await?;

        let skill = skill_nodes
            .into_iter()
            .find(|n| n.content == "Node Creation")
            .expect("Node Creation skill should exist");

        let desc = skill
            .properties
            .get("description")
            .or_else(|| {
                skill
                    .properties
                    .get("skill")
                    .and_then(|s| s.get("description"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Both schemas must appear, not just one
        assert!(
            desc.contains("invoice") && desc.contains("customer"),
            "Description should mention both custom schemas after creation, got: {:?}",
            desc
        );

        let _ = shutdown_tx.send(true);
        let _ = timeout(Duration::from_millis(500), updater_task).await;

        Ok(())
    }

    #[tokio::test]
    async fn test_skill_updater_removes_deleted_schema_from_description() -> Result<()> {
        let (service, _temp_dir) = create_test_service().await?;

        seed_node_creation_skill(&service).await?;

        // Create two schemas first
        create_schema_node(&service, "invoice").await?;
        create_schema_node(&service, "customer").await?;

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let updater = Arc::new(SkillUpdater::new(Arc::clone(&service)));
        let updater_task = {
            let updater = Arc::clone(&updater);
            tokio::spawn(async move { updater.start(shutdown_rx).await })
        };

        // Let startup sync run and verify both schemas appear
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Delete the invoice schema
        service.delete_node("invoice", 1).await.ok();

        // Allow time for the SkillUpdater to react
        tokio::time::sleep(Duration::from_millis(200)).await;

        let skill_nodes = service
            .query_nodes_simple(nodespace_core::models::NodeQuery {
                node_type: Some("skill".to_string()),
                content_contains: Some("Node Creation".to_string()),
                ..Default::default()
            })
            .await?;

        let skill = skill_nodes
            .into_iter()
            .find(|n| n.content == "Node Creation")
            .expect("Node Creation skill should exist");

        let desc = skill
            .properties
            .get("description")
            .or_else(|| {
                skill
                    .properties
                    .get("skill")
                    .and_then(|s| s.get("description"))
            })
            .and_then(|v| v.as_str())
            .unwrap_or("");

        assert!(
            !desc.contains("invoice"),
            "Description should NOT contain deleted schema 'invoice', got: {:?}",
            desc
        );

        let _ = shutdown_tx.send(true);
        let _ = timeout(Duration::from_millis(500), updater_task).await;

        Ok(())
    }

    #[test]
    fn test_build_description_lists_custom_types() {
        let types = vec![
            "invoice".to_string(),
            "customer".to_string(),
            "project".to_string(),
        ];
        let desc = build_node_creation_description(&types);
        assert!(desc.contains("invoice"));
        assert!(desc.contains("customer"));
        assert!(desc.contains("project"));
        assert!(
            !desc.contains("custom type"),
            "Should not mention generic 'custom type' when specific types are present"
        );
    }

    #[test]
    fn test_build_description_falls_back_to_base_when_empty() {
        let desc = build_node_creation_description(&[]);
        assert!(
            desc.contains("custom type"),
            "Base description should mention generic 'custom type'"
        );
    }
}
