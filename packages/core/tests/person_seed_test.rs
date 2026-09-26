//! Local PersonNode seeding tests (ADR-037)
//!
//! ADR-037 mandates that every install — free included — seeds exactly one
//! local PersonNode (the local user). On Pro upgrade this node is bound to a
//! Supabase identity, not recreated. These tests verify:
//! 1. Constructing a NodeService on a fresh database seeds exactly one person.
//! 2. The seeded person has no auth_status (that lives on DatabaseSettingsNode).
//! 3. Re-opening the same database does NOT create a second person (idempotent).

#[cfg(test)]
mod person_seed_tests {
    use anyhow::Result;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::services::NodeService;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn fresh_install_seeds_exactly_one_local_person() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;

        let people = service.query_nodes_by_type("person", None).await?;
        assert_eq!(
            people.len(),
            1,
            "a fresh install must seed exactly one local PersonNode"
        );
        // auth_status lives on DatabaseSettingsNode, not on PersonNode
        assert!(
            people[0]
                .properties
                .get("person")
                .and_then(|p| p.get("auth_status"))
                .is_none(),
            "seeded PersonNode must not carry auth_status — that belongs on DatabaseSettingsNode"
        );

        Ok(())
    }

    #[tokio::test]
    async fn reopening_database_does_not_seed_a_second_person() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        // First open seeds the local person.
        {
            let mut store = Arc::new(SqliteStore::new(db_path.clone()).await?);
            let service = NodeService::new(&mut store).await?;
            assert_eq!(service.query_nodes_by_type("person", None).await?.len(), 1);
        }

        // Second open of the same database must be idempotent — still one person.
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        assert_eq!(
            service.query_nodes_by_type("person", None).await?.len(),
            1,
            "re-opening an existing database must not seed a second PersonNode"
        );

        Ok(())
    }
}

/// Uniqueness on `person.email` is a suggest-don't-block rule: `find_duplicate_for`
/// surfaces an existing match for the UI, but a colliding write is never rejected.
#[cfg(test)]
mod person_email_uniqueness_tests {
    use anyhow::Result;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::Node;
    use nodespace_core::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn service() -> Result<(NodeService, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        Ok((service, temp_dir))
    }

    async fn create_person(service: &NodeService, name: &str, email: &str) -> Result<String> {
        let node = Node::new(
            "person".to_string(),
            String::new(),
            json!({ "person": { "name": name, "email": email } }),
        );
        let id = service.create_node(node).await?;
        Ok(id)
    }

    #[tokio::test]
    async fn returns_existing_person_on_case_folded_email_match() -> Result<()> {
        let (service, _t) = service().await?;
        let existing = create_person(&service, "Alice", "Alice@Example.com").await?;

        // person.email is flagged unique + case-insensitive, so a differently-cased
        // claim resolves to the existing person.
        let dup = service
            .find_duplicate_for("person", "email", "alice@example.com", None)
            .await?;
        assert_eq!(
            dup.map(|n| n.id),
            Some(existing),
            "a case-folded email match must surface the existing person"
        );
        Ok(())
    }

    #[tokio::test]
    async fn returns_none_for_a_unique_email() -> Result<()> {
        let (service, _t) = service().await?;
        create_person(&service, "Alice", "alice@example.com").await?;

        let dup = service
            .find_duplicate_for("person", "email", "nobody@example.com", None)
            .await?;
        assert!(dup.is_none(), "a never-seen email has no duplicate");
        Ok(())
    }

    #[tokio::test]
    async fn returns_none_for_empty_email() -> Result<()> {
        let (service, _t) = service().await?;
        create_person(&service, "Alice", "alice@example.com").await?;

        assert!(service
            .find_duplicate_for("person", "email", "", None)
            .await?
            .is_none());
        assert!(service
            .find_duplicate_for("person", "email", "   ", None)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn returns_none_when_field_is_not_flagged_unique() -> Result<()> {
        let (service, _t) = service().await?;
        create_person(&service, "Alice", "alice@example.com").await?;

        // `name` is not flagged unique, so a matching name is never a duplicate.
        let dup = service
            .find_duplicate_for("person", "name", "Alice", None)
            .await?;
        assert!(
            dup.is_none(),
            "a non-unique field must never report a duplicate"
        );
        Ok(())
    }

    #[tokio::test]
    async fn creating_two_persons_with_the_same_email_succeeds() -> Result<()> {
        // Locks in suggest-don't-block: a uniqueness collision NEVER rejects a write.
        // Two offline devices can each validly create the same person, so a duplicate
        // email at data entry must persist, not fail.
        let (service, _t) = service().await?;

        let first = create_person(&service, "Alice", "a@x.com").await?;
        let second = create_person(&service, "Alice (dup)", "a@x.com").await?;
        assert_ne!(first, second);

        // Both nodes exist.
        assert!(service.get_node(&first).await?.is_some());
        assert!(service.get_node(&second).await?.is_some());

        // And the read-only lookup can still see one of them as a suggested duplicate.
        let dup = service
            .find_duplicate_for("person", "email", "a@x.com", None)
            .await?;
        assert!(
            dup.is_some(),
            "the lookup should still surface a duplicate even though both writes succeeded"
        );
        Ok(())
    }
}
