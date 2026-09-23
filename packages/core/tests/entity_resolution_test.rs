//! Entity resolution: turning a name in a message into the node that bears it.
//!
//! These drive `resolve_entities_by_title` against a real database, which is
//! the only way to exercise what actually decides the answer — FTS5's
//! tokenizer, its bm25 ranking, and the `node_title_fts` triggers. The
//! rendering side (how a resolution reaches the prompt, and the
//! no-match-vs-did-not-run distinction) is unit-tested in `context_ops`.
//!
//! The precision question this tier had to answer before it could be built:
//! *when an entity exists and is named in a message, does a title search find
//! it — and not everything else?* The cases below are that measurement, run
//! against a seeded workspace rather than against fixture prompts whose
//! entities are absent from the graph by design.

#[cfg(test)]
mod entity_resolution_tests {
    use anyhow::Result;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::models::Node;
    use nodespace_core::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Returns the SERVICE as well as the store: `title` is computed by
    /// `NodeService::create_node` (crud.rs), not by the raw store insert, so a
    /// fixture seeded through the store alone carries a NULL title and is
    /// invisible to this index. Seeding through the service is also what the
    /// real write path does.
    async fn create_test_store() -> Result<(Arc<SqliteStore>, Arc<NodeService>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        Ok((store, Arc::new(service), temp_dir))
    }

    /// Seed an entity through the service, so `compute_title` runs and the
    /// node lands in `node_title_fts` under its name.
    async fn seed_entity(
        service: &Arc<NodeService>,
        node_type: &str,
        name: &str,
    ) -> Result<String> {
        let node = Node::new(node_type.to_string(), name.to_string(), json!({}));
        let id = node.id.clone();
        service.create_node(node).await?;
        Ok(id)
    }

    /// The motivating case: a name buried in an ordinary sentence resolves to
    /// the node that bears it, and the surrounding words do not drag in
    /// everything else.
    #[tokio::test]
    async fn a_named_entity_in_a_sentence_resolves_to_its_node() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        let northwind = seed_entity(&service, "text", "Northwind Trading").await?;
        seed_entity(&service, "text", "Contoso Ltd").await?;
        seed_entity(&service, "text", "Fabrikam Inc").await?;

        let hits = store
            .resolve_entities_by_title("Add Northwind Trading to the companies we sell to", 12)
            .await?;

        assert_eq!(
            hits.first().map(|h| h.id.as_str()),
            Some(northwind.as_str()),
            "the named entity must rank first, got: {hits:?}"
        );
        Ok(())
    }

    /// Ambiguity is returned, not resolved. One name on two types is the case
    /// the caller most needs to see; collapsing it here would silently pick.
    #[tokio::test]
    async fn one_name_on_two_types_returns_both() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        seed_entity(&service, "text", "Acme").await?;
        seed_entity(&service, "task", "Acme").await?;

        let hits = store
            .resolve_entities_by_title("what's happening with Acme", 12)
            .await?;

        assert_eq!(
            hits.len(),
            2,
            "both same-named nodes must come back, got: {hits:?}"
        );
        Ok(())
    }

    /// A partial name still reaches its node. "the Northwind deal" shares only
    /// one token with "Northwind Trading", which is why the query ORs its
    /// tokens rather than ANDing them.
    #[tokio::test]
    async fn a_partial_name_still_resolves() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        let id = seed_entity(&service, "text", "Northwind Trading").await?;

        let hits = store
            .resolve_entities_by_title("when did we sign the Northwind deal", 12)
            .await?;

        assert_eq!(hits.first().map(|h| h.id.as_str()), Some(id.as_str()));
        Ok(())
    }

    /// A message naming nothing in the graph resolves to nothing. This is the
    /// signal that means CREATE, so it must be empty rather than a weak match.
    #[tokio::test]
    async fn a_message_naming_no_entity_resolves_to_nothing() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        seed_entity(&service, "text", "Northwind Trading").await?;

        let hits = store
            .resolve_entities_by_title("what should I work on today", 12)
            .await?;

        assert!(
            hits.is_empty(),
            "a message naming nothing must resolve to nothing, got: {hits:?}"
        );
        Ok(())
    }

    /// Stop words alone must not match. "the companies we sell to" is all
    /// filler; without stop-word filtering a common word would drag in every
    /// node whose title happens to contain it.
    #[tokio::test]
    async fn stop_words_alone_do_not_resolve() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        seed_entity(&service, "text", "The Big Project").await?;

        let hits = store
            .resolve_entities_by_title("what is the to of in on", 12)
            .await?;

        assert!(
            hits.is_empty(),
            "a message of pure stop words must resolve to nothing, got: {hits:?}"
        );
        Ok(())
    }

    /// An archived node is not a live entity. Resolving a name to one would
    /// pull something the user has put away back into the turn as if current.
    #[tokio::test]
    async fn archived_nodes_are_not_resolved() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        let id = seed_entity(&service, "text", "Northwind Trading").await?;
        store.update_lifecycle_status(&id, "archived").await?;

        let hits = store
            .resolve_entities_by_title("Add Northwind Trading to the list", 12)
            .await?;

        assert!(
            hits.is_empty(),
            "an archived node must not resolve, got: {hits:?}"
        );
        Ok(())
    }

    /// A schema is titled by its type name so search can find it, but a type is
    /// not an entity: "the list" must not resolve to the Ordered List schema.
    #[tokio::test]
    async fn schema_type_names_are_not_resolved() -> Result<()> {
        let (store, _service, _t) = create_test_store().await?;

        let hits = store
            .resolve_entities_by_title("Add a task to the ordered list", 12)
            .await?;

        assert!(
            hits.iter().all(|h| h.node_type != "schema"),
            "a schema must not resolve as an entity, got: {hits:?}"
        );
        Ok(())
    }

    /// A body node that merely MENTIONS a name is not an entity. A node with a
    /// parent carries no title (`compute_title` returns None for a non-root
    /// node of a content-titled type), so it never enters `node_title_fts` and
    /// a title search for the name it contains finds nothing.
    ///
    /// The create path has to supply rootness to the title computation rather
    /// than derive it: the node row is inserted before its parent edge, in the
    /// same transaction, so a store lookup at insert time sees no parent and
    /// would title the child with its own body text.
    #[tokio::test]
    async fn a_body_node_mentioning_the_name_is_not_an_entity() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        let parent = seed_entity(&service, "text", "Meeting Notes").await?;
        let child = Node::new(
            "text".to_string(),
            "we should call Northwind Trading about the renewal".to_string(),
            json!({}),
        );
        service
            .create_node_with_parent(nodespace_core::services::CreateNodeParams {
                id: Some(child.id.clone()),
                node_type: child.node_type.clone(),
                content: child.content.clone(),
                parent_id: Some(parent.clone()),
                position: nodespace_core::services::InsertPositionOwned::End,
                properties: child.properties.clone(),
                lifecycle_status: None,
            })
            .await?;

        let stored = store.get_node(&child.id).await?.expect("child was created");
        assert_eq!(stored.title, None, "a child node carries no title");

        let hits = store
            .resolve_entities_by_title("Northwind Trading", 12)
            .await?;
        assert!(
            hits.is_empty(),
            "a body node must not resolve as an entity: {hits:?}"
        );
        Ok(())
    }

    /// A name late in a long message still resolves.
    ///
    /// The token budget is taken from the FRONT of the message, so anything
    /// ahead of the name competes with it. This is the shape that made the
    /// tier inert on every turn but the first: the daemon was passing the
    /// BLENDED retrieval query — up to two prior conversational turns
    /// prepended before the current message — and those turns consumed the
    /// whole budget. "Add Northwind Trading to the companies we sell to"
    /// tokenised to `set up new type places hold`, the prior turn's opening
    /// words, and the entity never reached the index.
    ///
    /// Worse than failing silent: empty candidates render as "none found",
    /// which asserts the named thing does not EXIST. A lookup that could not
    /// see the name would have been laundered into an instruction to create a
    /// duplicate.
    ///
    /// `build_workspace_context` now takes the current message separately for
    /// this reason. This test pins the property that fix depends on — that a
    /// name preceded by other words is still found — at the level where the
    /// truncation actually happens.
    #[tokio::test]
    async fn a_name_late_in_a_long_message_still_resolves() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        let id = seed_entity(&service, "text", "Northwind Trading").await?;

        // Deliberately more leading non-stop-word tokens than the resolver's
        // budget, so a front-truncating implementation cannot reach the name.
        let message = "set up new type places hold events booking capacity \
                       roster venue schedule then add Northwind Trading";

        let hits = store.resolve_entities_by_title(message, 12).await?;

        assert_eq!(
            hits.first().map(|h| h.id.as_str()),
            Some(id.as_str()),
            "a name after many leading words must still resolve — if this fails, \
             the token budget is being consumed before the entity: {hits:?}"
        );
        Ok(())
    }

    /// Two multi-token names in one message both resolve.
    ///
    /// This is what the token cap has to be wide enough for. Entity names run
    /// to two or three tokens apiece, so a message naming two of them needs
    /// room for both — a cap tight enough to hold only the first would make
    /// the second silently unresolvable, and "resolved to nothing" is rendered
    /// as a positive claim that it does not exist.
    #[tokio::test]
    async fn two_multi_token_names_in_one_message_both_resolve() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        let northwind = seed_entity(&service, "text", "Northwind Trading").await?;
        let contoso = seed_entity(&service, "text", "Contoso Holdings").await?;

        let hits = store
            .resolve_entities_by_title("move Northwind Trading under Contoso Holdings", 12)
            .await?;

        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert!(
            ids.contains(&northwind.as_str()) && ids.contains(&contoso.as_str()),
            "both named entities must resolve — a cap too tight drops the second \
             and renders it as nonexistent: {hits:?}"
        );
        Ok(())
    }

    /// KNOWN RESIDUAL, pinned so it is visible rather than folklore.
    ///
    /// The capitalised-first selection fixes the case where the filler
    /// competing for the token budget is lowercase. When the competing tokens
    /// are capitalised too — a Title Case register — front-first truncation
    /// applies within the capitalised class and the entity can still be cut.
    ///
    /// Less severe than the bug it replaced, in the way that matters: this
    /// degrades to a WEAK match rather than to `NoMatch`, so it does not
    /// produce a false "this does not exist" claim that would license creating
    /// a duplicate. The entity is still found here, just not ranked first.
    ///
    /// If a future change makes the entity rank first, invert the assertion —
    /// that is an improvement, not a regression.
    #[tokio::test]
    async fn a_title_case_message_can_outrank_the_entity_with_filler() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        seed_entity(&service, "text", "Northwind Trading").await?;
        seed_entity(&service, "text", "Customer Record").await?;

        let hits = store
            .resolve_entities_by_title(
                "Could You Kindly Update The Customer Record And Billing Address For Northwind Trading",
                12,
            )
            .await?;

        assert!(
            !hits.is_empty(),
            "the residual degrades ranking, not resolution — an empty result here \
             would mean a false 'does not exist': {hits:?}"
        );
        Ok(())
    }

    /// The limit is honoured, so one common word cannot flood the caller.
    #[tokio::test]
    async fn the_limit_bounds_the_result_set() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        for i in 0..10 {
            seed_entity(&service, "text", &format!("Project Alpha {i}")).await?;
        }

        let hits = store.resolve_entities_by_title("Project Alpha", 3).await?;

        assert_eq!(hits.len(), 3, "limit must bound the result set");
        Ok(())
    }

    /// An exact, shorter title outranks a longer one containing it. bm25
    /// length-normalizes, which is what makes "Acme" beat "Acme Holdings
    /// International" for the query "Acme" — the behaviour the tier relies on
    /// to put the most likely referent first.
    #[tokio::test]
    async fn an_exact_title_outranks_a_longer_one_containing_it() -> Result<()> {
        let (store, service, _t) = create_test_store().await?;
        seed_entity(&service, "text", "Acme Holdings International Group").await?;
        let exact = seed_entity(&service, "text", "Acme").await?;

        let hits = store.resolve_entities_by_title("Acme", 12).await?;

        assert_eq!(
            hits.first().map(|h| h.id.as_str()),
            Some(exact.as_str()),
            "the exact title must rank first, got: {hits:?}"
        );
        Ok(())
    }
}
