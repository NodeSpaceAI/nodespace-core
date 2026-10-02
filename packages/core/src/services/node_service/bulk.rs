//! Bulk operations for NodeService.

use super::crud::FieldOwnershipInfo;
use super::*;

impl NodeService {
    /// Attach each bulk row's title, derived by the same rule as single-node
    /// creation ([`Self::derive_title`]), with one schema lookup per type
    /// and — for a templated, `extends`-chain type — one
    /// `resolve_field_owners` chain resolution per type rather than per row.
    pub(crate) async fn with_titles(
        &self,
        rows: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<crate::db::BulkNodeRow>, NodeServiceError> {
        let mut schemas: std::collections::HashMap<String, Option<crate::models::SchemaNode>> =
            std::collections::HashMap::new();
        let mut chain_fields: std::collections::HashMap<
            String,
            (Vec<crate::models::SchemaField>, Vec<String>),
        > = std::collections::HashMap::new();
        let mut out = Vec::with_capacity(rows.len());
        for (id, node_type, content, parent_id, order, properties) in rows {
            if !schemas.contains_key(&node_type) {
                let schema = self.title_schema(&node_type).await;
                schemas.insert(node_type.clone(), schema);
            }
            let schema = schemas.get(&node_type).and_then(Option::as_ref);
            if schema.and_then(|s| s.title_template.as_ref()).is_some()
                && !chain_fields.contains_key(&node_type)
            {
                let (fields, _owners, chain) = self.resolve_field_owners(&node_type).await?;
                chain_fields.insert(node_type.clone(), (fields, chain));
            }
            let node = Node {
                id,
                node_type,
                content,
                version: 1,
                properties,
                mentions: vec![],
                mentioned_in: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None,
                lifecycle_status: "active".to_string(),
            };
            // Every caller is a bulk hierarchy insert, so this is a create:
            // always held to the templated-type content rule.
            Self::reject_content_on_templated_type(
                &node,
                schemas.get(&node.node_type).and_then(Option::as_ref),
            )?;
            let title = self
                .derive_title(
                    &node,
                    parent_id.is_none(),
                    schemas.get(&node.node_type).and_then(Option::as_ref),
                    chain_fields.get(&node.node_type),
                )
                .await?;
            out.push((
                node.id,
                node.node_type,
                node.content,
                parent_id,
                order,
                node.properties,
                title,
            ));
        }
        Ok(out)
    }

    /// Bulk create multiple nodes in a transaction
    ///
    /// Creates multiple nodes atomically. If any node fails validation or insertion,
    /// the entire transaction is rolled back.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of nodes to create
    ///
    /// # Returns
    ///
    /// Vector of created node IDs in the same order as input
    ///
    /// # Errors
    ///
    /// Returns error if any node fails validation or insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let nodes = vec![
    ///     Node::new("text".to_string(), "Note 1".to_string(), json!({})),
    ///     Node::new("text".to_string(), "Note 2".to_string(), json!({})),
    /// ];
    /// let ids = service.bulk_create(nodes).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_create(&self, mut nodes: Vec<Node>) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Validate all nodes first, re-bucketing each node's properties by
        // declaring owner across its `extends` chain (ADR-078) before
        // persisting — the same sequence every other write path owes a node,
        // via `rebucket_and_validate`. Without this, an inherited field
        // normalized into the node's own bucket stayed there rather than
        // moving to its declaring ancestor's, so a base-scoped reader never
        // found it and the ancestor's behaviour never checked it.
        // `apply_defaults: false` preserves bulk_create's existing contract
        // of validating exactly what the caller supplied, not filling in
        // what they didn't.
        let mut schemas: std::collections::HashMap<String, Option<crate::models::SchemaNode>> =
            std::collections::HashMap::new();
        let mut chain_fields: std::collections::HashMap<
            String,
            (Vec<crate::models::SchemaField>, Vec<String>),
        > = std::collections::HashMap::new();
        let mut instantiable_types = std::collections::HashSet::new();
        let mut ownership_by_type = std::collections::HashMap::new();
        for node in &mut nodes {
            // Every node of this batch is created a root, so a type that
            // needs a parent is refused (ADR-089). Asked once per distinct
            // type: `instantiable_types` gains the type in step 1.
            if !instantiable_types.contains(&node.node_type) {
                self.store
                    .assert_may_be_root(&node.node_type, Some(&node.id))
                    .await
                    .map_err(NodeServiceError::from_store)?;
            }

            // Step 1: Type and id validation
            self.ensure_creatable_in_batch(node, &mut instantiable_types)
                .await?;

            // A create: always held to the templated-type content rule.
            if !schemas.contains_key(&node.node_type) {
                let schema = self.title_schema(&node.node_type).await;
                schemas.insert(node.node_type.clone(), schema);
            }
            Self::reject_content_on_templated_type(
                node,
                schemas.get(&node.node_type).and_then(Option::as_ref),
            )?;

            // Step 2: Re-bucketing, then behavior and chain-aware schema
            // validation
            let ownership = self
                .field_ownership_in_batch(&node.node_type, &mut ownership_by_type)
                .await?;
            self.rebucket_and_validate_with(node, ownership, false)?;
            // A new play carries no suspension, on this create path as on
            // the single-node one (ADR-087 §5).
            if self
                .type_is_a(&node.node_type, crate::models::CoreNodeType::Play)
                .await?
            {
                Self::ensure_play_created_unsuspended(node)?;
            }

            // Step 3: Title. An untemplated type's caller-supplied title is
            // honored as given, computed only when absent — matching the
            // single-node create path's own `if node.title.is_none()` rule
            // (`insert_node_in_tx_no_invariant_dispatch`). A templated
            // type's title, though, is ALWAYS (re)derived here, even when
            // one was supplied — a strictly broader rule than the
            // single-node path's, which gets away with the plain
            // `is_none()` check only because its one real caller always
            // sends `title: None` for every row. A *batched* caller applying
            // writes made elsewhere can't make that same assumption — it may
            // send a non-null placeholder for every row, including templated
            // ones — so an `is_none()`-only check here would silently miss
            // exactly the case this method exists to fix. A templated type's
            // title is never legitimately caller-controlled anyway — its `content`
            // must already be empty (the rule enforced above) — so
            // overriding a non-null placeholder is correct, not just
            // permissive. Read AFTER `rebucket_and_validate` so a templated
            // interpolation sees the final, chain-bucketed property layout,
            // not the pre-rebucket one. `bulk_create` writes no `has_child`
            // edge itself — a hierarchy import goes through
            // `bulk_create_hierarchy*` instead — so every row here is a
            // root at write time, the same treatment `create_node`'s own
            // `is_root: true` gives a plain, non-hierarchical create.
            let schema = schemas.get(&node.node_type).and_then(Option::as_ref);
            let has_template = schema.and_then(|s| s.title_template.as_ref()).is_some();
            if has_template && !chain_fields.contains_key(&node.node_type) {
                let (fields, _owners, chain) = self.resolve_field_owners(&node.node_type).await?;
                chain_fields.insert(node.node_type.clone(), (fields, chain));
            }
            if has_template || node.title.is_none() {
                let new_title = self
                    .derive_title(node, true, schema, chain_fields.get(&node.node_type))
                    .await?;
                node.title = new_title;
            }
        }

        // Collection-name collisions are suggest-don't-block (ADR-065): detect
        // them before the write and journal them after commit, the same
        // timing single-node `create_node` keeps. A collision can be with a
        // collection already stored or with an earlier row of this batch —
        // a sync page carrying two devices' same-named collections is exactly
        // the case this journal exists for.
        let mut collisions: Vec<(String, String)> = Vec::new();
        let mut batch_collections: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        // Within the batch, match the way `get_collection_by_name` matches a
        // stored row — this row's lowercased content against an earlier
        // active collection's lowercased title — so a batch detects exactly
        // what the same rows created one at a time would have.
        // Which of the batch's types are collections, resolved through each
        // type's `extends` chain once rather than per row.
        let mut collection_types: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        for node in nodes.iter() {
            if !collection_types.contains_key(&node.node_type) {
                let is_collection = self
                    .type_is_a(&node.node_type, crate::models::CoreNodeType::Collection)
                    .await?;
                collection_types.insert(node.node_type.clone(), is_collection);
            }
        }
        for node in nodes
            .iter()
            .filter(|n| collection_types.get(&n.node_type) == Some(&true))
        {
            let name = node.content.to_lowercase();
            let stored = self
                .store
                .get_collection_by_name(&node.content)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to check collection name collision: {}",
                        e
                    ))
                })?
                .map(|existing| existing.id);
            if let Some(existing) = stored.or_else(|| batch_collections.get(&name).cloned()) {
                collisions.push((node.id.clone(), existing));
            }
            if let (true, Some(title)) = (crate::governance::participates(node), &node.title) {
                batch_collections
                    .entry(title.to_lowercase())
                    .or_insert_with(|| node.id.clone());
            }
        }

        // Insert every row, then run invariant-rule dispatch (ADR-060 §1) for
        // each, all in one transaction: a rejected row rolls back the whole
        // batch, which is also what makes the all-or-nothing contract above
        // hold. Events carry this instance's client_id/execution_context
        // (via `emit_event`), which the ADR-073 local-origin gate depends on.
        let service = self.clone();
        let ids = self
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let mut rules_by_type: std::collections::HashMap<
                        String,
                        Vec<crate::playbook::types::OrderedRuleRef>,
                    > = std::collections::HashMap::new();
                    for node in &nodes {
                        crate::db::SqliteStore::create_node_in_tx(tx.store_tx(), node)
                            .await
                            .map_err(NodeServiceError::from_store)?;
                        service.emit_event_in_tx(
                            tx,
                            DomainEvent::NodeCreated {
                                node_id: node.id.clone(),
                                node_type: node.node_type.clone(),
                            },
                        );
                        if !rules_by_type.contains_key(&node.node_type) {
                            let rules = service.invariant_rules_for_creation(&node.node_type);
                            rules_by_type.insert(node.node_type.clone(), rules);
                        }
                    }
                    for node in &nodes {
                        let matched = rules_by_type[&node.node_type].clone();
                        service
                            .run_creation_invariant_rules_in_tx(tx, node, matched)
                            .await?;
                    }
                    Ok(nodes.into_iter().map(|n| n.id).collect::<Vec<_>>())
                })
            })
            .await?;

        for (new_id, existing_id) in collisions {
            // Best-effort: the batch is already durably committed.
            self.store
                .mark_collection_name_collision(&new_id, &existing_id)
                .await;
        }

        Ok(ids)
    }

    /// Insert prepared hierarchy rows on `tx`, buffer one `NodeCreated` per
    /// row, then run invariant-rule dispatch (ADR-060 §1) for every inserted
    /// node, looking each distinct type's rules up once. The shared
    /// insert-and-dispatch step of every bulk hierarchy create: a rejected
    /// row fails the caller's transaction, rolling back every row of the
    /// batch along with any invariant action's own writes.
    ///
    /// Dispatch runs after the whole batch is inserted, so an invariant
    /// action — which runs on `tx` — sees every row and edge of the batch.
    /// A rule's conditions do not: they are evaluated through pooled reads,
    /// which cannot see this transaction's uncommitted rows, the same limit
    /// every single-node dispatch has.
    async fn insert_bulk_hierarchy_rows_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        rows: Vec<crate::db::BulkNodeRow>,
    ) -> Result<Vec<String>, NodeServiceError> {
        let mut rules_by_type: std::collections::HashMap<
            String,
            Vec<crate::playbook::types::OrderedRuleRef>,
        > = std::collections::HashMap::new();
        for (_, node_type, ..) in &rows {
            if !rules_by_type.contains_key(node_type) {
                let rules = self.invariant_rules_for_creation(node_type);
                rules_by_type.insert(node_type.clone(), rules);
            }
        }

        // Only rows a rule can run for need a `Node` built — a batch whose
        // types carry no invariant rule copies nothing.
        let to_dispatch: Vec<Node> = rows
            .iter()
            .filter(|(_, node_type, ..)| !rules_by_type[node_type].is_empty())
            .map(
                |(id, node_type, content, _parent, _order, properties, title)| Node {
                    id: id.clone(),
                    node_type: node_type.clone(),
                    content: content.clone(),
                    version: 1,
                    properties: if properties.is_null() {
                        serde_json::json!({})
                    } else {
                        properties.clone()
                    },
                    mentions: vec![],
                    mentioned_in: vec![],
                    created_at: chrono::Utc::now(),
                    modified_at: chrono::Utc::now(),
                    title: title.clone(),
                    lifecycle_status: "active".to_string(),
                },
            )
            .collect();

        let node_types: Vec<String> = rows
            .iter()
            .map(|(_, node_type, ..)| node_type.clone())
            .collect();

        let ids = self
            .store
            .bulk_create_hierarchy_in_tx(tx.store_tx(), rows)
            .await
            .map_err(NodeServiceError::from_store)?;

        for (id, node_type) in ids.iter().zip(node_types) {
            self.emit_event_in_tx(
                tx,
                DomainEvent::NodeCreated {
                    node_id: id.clone(),
                    node_type,
                },
            );
        }

        for node in &to_dispatch {
            let matched = rules_by_type[&node.node_type].clone();
            self.run_creation_invariant_rules_in_tx(tx, node, matched)
                .await?;
        }

        Ok(ids)
    }

    /// Bulk create nodes with hierarchy in a single transaction
    ///
    /// Creates multiple nodes with parent-child relationships atomically.
    /// This method is optimized for markdown import where all node data
    /// (IDs, hierarchy, ordering) is pre-calculated.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of tuples: (id, node_type, content, parent_id, order, properties)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<String>)` - Vector of created node IDs in insertion order
    /// * `Err` - If validation or transaction fails
    ///
    /// # Performance
    ///
    /// This method provides ~10-15x speedup over sequential node creation
    /// by batching all database operations into a single transaction.
    pub async fn bulk_create_hierarchy(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        let Some(nodes_normalized) = self.prepare_bulk_hierarchy_nodes(nodes).await? else {
            return Ok(Vec::new());
        };

        // Find the embedding root once: every node in a bulk import sits under
        // the first node's parent, and a bulk import writes no `member_of`
        // edge that could cut an access boundary between them (ADR-059 §7).
        // Performance optimization: Single DB query instead of N queries
        let root_id = if let Some((_, _, _, Some(first_parent), _, _)) = nodes_normalized.first() {
            self.get_embedding_root_id(first_parent).await.ok()
        } else {
            None
        };

        // Titles are derived before the transaction opens — they read
        // schemas, and the write guard need not be held for that. The insert
        // and its invariant-rule dispatch share one transaction, so a
        // rejected row fails the whole import.
        let rows = self.with_titles(nodes_normalized).await?;
        let service = self.clone();
        let result = self
            .with_transaction(move |tx| {
                Box::pin(async move { service.insert_bulk_hierarchy_rows_in_tx(tx, rows).await })
            })
            .await?;

        // Queue root for embedding regeneration once
        // All nodes share the same root, so we only need one queue operation
        #[cfg(feature = "nlp")]
        if let Some(root_id) = root_id {
            self.queue_root_for_embedding(&root_id).await;
        }

        Ok(result)
    }

    /// A bulk hierarchy row as the node it will be stored as, for
    /// validation: its flat properties moved under its type's own namespace.
    fn bulk_row_as_node(
        id: String,
        node_type: String,
        content: String,
        properties: &serde_json::Value,
    ) -> Node {
        let properties = Self::normalize_flat_properties_to_namespace(&node_type, properties);
        Node {
            id,
            node_type,
            content,
            version: 1,
            properties,
            mentions: vec![],
            mentioned_in: vec![],
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            title: None, // Bulk nodes don't need titles (validated only)
            lifecycle_status: "active".to_string(),
        }
    }

    /// Shared preamble for [`Self::bulk_create_hierarchy`] and
    /// [`Self::bulk_create_hierarchy_in_tx`]: resolves each unique
    /// node type's `extends` chain (ADR-078) once, normalizes flat
    /// properties to namespaced format, then runs each node through
    /// [`Self::rebucket_and_validate_with`]: re-bucketed by declaring owner,
    /// and validated against behaviors and (where applicable) its
    /// chain-resolved schema fields. Returns
    /// `Ok(None)` for an empty input (every caller treats that as "nothing
    /// to do"), otherwise the normalized, bucketed, validated node tuples
    /// ready for insertion.
    ///
    /// Chain-resolved via `resolve_field_owners` rather than a per-type
    /// `get_schema_for_type` + hand-rolled `fields` JSON parse: an
    /// extending type's inherited fields are declared by an ancestor
    /// schema, so an own-type-only fetch would neither validate them nor
    /// know which bucket to re-file them under — the same gap
    /// `NodeService::rebucket_and_validate` closes for the single-node
    /// write paths. This also removes the old silent failure mode where a
    /// present-but-malformed `fields` array parsed via `.ok()` collapsed
    /// to "no schema, skip validation" for every row of that type; see the
    /// explicit malformed-schema check below, which fails loudly instead.
    async fn prepare_bulk_hierarchy_nodes(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<
        Option<
            Vec<(
                String,
                String,
                String,
                Option<String>,
                f64,
                serde_json::Value,
            )>,
        >,
        NodeServiceError,
    > {
        if nodes.is_empty() {
            return Ok(None);
        }

        // Performance optimization: resolve each unique type's chain once,
        // not once per row.
        let unique_types: std::collections::HashSet<&str> = nodes
            .iter()
            .map(|(_, node_type, _, _, _, _)| node_type.as_str())
            .collect();

        let mut ownership_by_type: std::collections::HashMap<String, FieldOwnershipInfo> =
            std::collections::HashMap::new();
        for node_type in unique_types {
            if !crate::models::CoreNodeType::Schema.is_exactly(node_type) {
                // Fail loudly on a malformed schema rather than silently
                // treating it as "no schema" for every row of this type: a
                // present-but-corrupt `fields` array and a genuinely
                // schema-less type are very different outcomes for
                // bulk-imported data. This checks the type's own declared
                // schema; a malformed *ancestor* schema mid-chain still falls
                // back to `resolve_field_owners`'s existing "contributes
                // nothing" posture, same as every other ADR-078 write path.
                if let Some(schema_json) = self.get_schema_for_type(node_type).await? {
                    if let Some(fields_json) = schema_json.get("fields") {
                        serde_json::from_value::<Vec<crate::models::SchemaField>>(
                            fields_json.clone(),
                        )
                        .map_err(|e| {
                            NodeServiceError::bulk_operation_failed(format!(
                                "Malformed schema fields for type '{}': {}",
                                node_type, e
                            ))
                        })?;
                    }
                }
            }

            ownership_by_type.insert(
                node_type.to_string(),
                self.field_ownership_for_write(node_type).await?,
            );
        }

        // Normalize flat properties to namespaced format, then re-bucket by
        // declaring owner across the chain (ADR-078) and validate — an
        // inherited field must move to its declaring ancestor's bucket, or it
        // sits in the node's own bucket duplicating (and shadowing) the
        // authoritative value a base-scoped reader looks for, unseen by the
        // ancestor's behaviour.
        // Parser emits: { "status": "open" }
        // Storage expects: { "task": { "status": "open" } } (or the
        // declaring ancestor's bucket, once re-bucketed)
        let mut instantiable_types = std::collections::HashSet::new();
        let mut nodes_normalized = Vec::with_capacity(nodes.len());
        for (id, node_type, content, parent_id, order, properties) in nodes {
            let mut node = Self::bulk_row_as_node(id, node_type, content, &properties);
            self.ensure_creatable_in_batch(&node, &mut instantiable_types)
                .await?;
            let ownership = &ownership_by_type[&node.node_type];
            self.rebucket_and_validate_with(&mut node, ownership, false)?;
            nodes_normalized.push((
                node.id,
                node.node_type,
                node.content,
                parent_id,
                order,
                node.properties,
            ));
        }

        Ok(Some(nodes_normalized))
    }

    /// `_in_tx` twin of [`Self::bulk_create_hierarchy`] (ADR-069 §1b/S3).
    /// Shares the same schema-cache/validation preamble via
    /// [`Self::prepare_bulk_hierarchy_nodes`]; the insert lands on
    /// `tx.store_tx()` via the store's own `bulk_create_hierarchy_in_tx`
    /// instead of opening a new transaction — this is what lets
    /// `create_description_subtree` compose into `handle_create_schema`'s
    /// outer transaction alongside the schema node and its relationship
    /// declarations. Root embedding-queueing is intentionally NOT
    /// reproduced here: it is derived state outside the boundary by design
    /// (ADR-069 §5) and the one current caller's root here is a schema
    /// node's description subtree, which is not itself embedded — a future
    /// caller that needs it should queue after `with_transaction` commits.
    /// Emits one `NodeCreated` event per inserted node, buffered the same
    /// way `create_node_in_tx` buffers its own, and runs invariant-rule
    /// dispatch for each on `tx`.
    pub(crate) async fn bulk_create_hierarchy_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        let Some(nodes_normalized) = self.prepare_bulk_hierarchy_nodes(nodes).await? else {
            return Ok(Vec::new());
        };

        let rows = self.with_titles(nodes_normalized).await?;
        self.insert_bulk_hierarchy_rows_in_tx(tx, rows).await
    }

    /// Bulk create nodes with trusted input (skips schema validation)
    ///
    /// Optimized for import paths where the source is trusted (like markdown parser).
    /// This method:
    /// - Normalizes flat properties to namespaced format
    /// - Skips schema DB queries for a type that extends nothing (no lookup
    ///   overhead)
    /// - Skips schema validation (parser output is trusted)
    /// - Still validates via behaviors (type-specific rules), on the
    ///   properties as they will be stored
    ///
    /// # Import Pipeline Optimization
    ///
    /// The markdown parser only creates known node types with correct properties:
    /// - Task nodes get `{"status": "open"}`
    /// - Header, text, code-block nodes get `{}`
    ///
    /// Since the parser is trusted, we skip the expensive schema lookup and
    /// validation, but still normalize properties to the correct storage format.
    ///
    /// # Invariant rules still run
    ///
    /// "Trusted" covers the parser's output *shape* — known types, well-formed
    /// properties — which is what schema validation checks. It says nothing
    /// about the product rules a user has authored as ADR-060 invariant rules
    /// (a `reject` on a type, a derived property it must carry), and the
    /// daemon's directory import — a real user-facing write — goes through
    /// here. So invariant-rule dispatch runs exactly as on every other create:
    /// in the insert's own transaction, a rejected row failing the whole
    /// import. Skipping it would leave the rule unevaluated for good, since
    /// the reactive engine never runs invariant rules. Its cost is one rule
    /// lookup per distinct node type when no invariant rule matches.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of (id, node_type, content, parent_id, order, properties) tuples
    ///
    /// # Returns
    ///
    /// Vector of created node IDs
    pub async fn bulk_create_hierarchy_trusted(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Normalize flat properties to namespaced format, then validate via
        // behaviors only (type-specific rules, no schema).
        // Parser emits: { "status": "open" }
        // Storage expects: { "task": { "status": "open" } }
        //
        // The parser's own types extend nothing, so each field's bucket is
        // the node's own and no schema is read. A type that does extend
        // another has its inherited fields moved to their declaring buckets
        // first, where the ancestor's behaviour reads them.
        let mut ownership_by_type: std::collections::HashMap<String, FieldOwnershipInfo> =
            std::collections::HashMap::new();
        let mut instantiable_types = std::collections::HashSet::new();
        let mut nodes_normalized = Vec::with_capacity(nodes.len());
        for (id, node_type, content, parent_id, order, properties) in nodes {
            if !ownership_by_type.contains_key(&node_type) {
                let chain = self.type_chain(&node_type).await?;
                let ownership = if chain.len() > 1 {
                    self.field_ownership_for_write(&node_type).await?
                } else {
                    (Vec::new(), std::collections::HashMap::new(), chain)
                };
                ownership_by_type.insert(node_type.clone(), ownership);
            }
            let mut node = Self::bulk_row_as_node(id, node_type, content, &properties);

            // Type, id and behavior validation only - skip schema validation
            self.ensure_creatable_in_batch(&node, &mut instantiable_types)
                .await?;
            let ownership = &ownership_by_type[&node.node_type];
            self.rebucket_and_validate_behaviors(&mut node, ownership, false)?;
            nodes_normalized.push((
                node.id,
                node.node_type,
                node.content,
                parent_id,
                order,
                node.properties,
            ));
        }

        // Collect embeddable root node IDs (nodes with no parent AND embeddable type)
        // Only these need embedding markers - matches single-create logic
        let mut embeddable_types: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        for (_, node_type, _, parent_id, _, _) in nodes_normalized.iter() {
            if parent_id.is_none() && !embeddable_types.contains_key(node_type) {
                let embeddable = self.is_embeddable_type(node_type).await;
                embeddable_types.insert(node_type.clone(), embeddable);
            }
        }
        let root_ids: Vec<String> = nodes_normalized
            .iter()
            .filter_map(|(id, node_type, _, parent_id, _, _)| {
                if parent_id.is_none() && embeddable_types.get(node_type) == Some(&true) {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        // One transaction for the insert and its invariant-rule dispatch. Its
        // event buffer holds one Created event per node and flushes them
        // together after commit, so a large import reaches WatchNodes
        // subscribers as a single burst rather than one event at a time.
        let rows = self.with_titles(nodes_normalized).await?;
        let service = self.clone();
        let result = self
            .with_transaction(move |tx| {
                Box::pin(async move { service.insert_bulk_hierarchy_rows_in_tx(tx, rows).await })
            })
            .await?;

        // Create stale embedding markers in bulk (single transaction)
        if !root_ids.is_empty() {
            match self
                .store
                .create_stale_embedding_markers_bulk(&root_ids)
                .await
            {
                Ok(count) => {
                    tracing::debug!("Created {} stale embedding markers", count);
                    // Wake the embedding processor once for all new roots
                    #[cfg(feature = "nlp")]
                    if let Some(waker) = self.embedding_waker.get() {
                        tracing::debug!(
                            "🔔 Waking embedding processor for {} bulk-imported roots",
                            count
                        );
                        waker.wake();
                    }
                }
                Err(e) => {
                    // Log but don't fail - embeddings can be regenerated later
                    tracing::warn!("Failed to create stale embedding markers: {}", e);
                }
            }
        }

        Ok(result)
    }

    /// Bulk update multiple nodes in a transaction
    ///
    /// Updates multiple nodes atomically using a map of node IDs to NodeUpdate structs.
    ///
    /// # Arguments
    ///
    /// * `updates` - Vector of (node_id, NodeUpdate) tuples
    ///
    /// # Errors
    ///
    /// Returns error if any update fails. Transaction is rolled back on failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let updates = vec![
    ///     ("node-1".to_string(), NodeUpdate::new().with_content("Updated 1".to_string())),
    ///     ("node-2".to_string(), NodeUpdate::new().with_content("Updated 2".to_string())),
    /// ];
    /// service.bulk_update(updates).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_update(
        &self,
        updates: Vec<(String, NodeUpdate)>,
    ) -> Result<(), NodeServiceError> {
        if updates.is_empty() {
            return Ok(());
        }

        // Step 1: Batch-fetch all nodes in a single query
        // This replaces the N+1 pattern where we called get_node() for each update
        let ids: Vec<String> = updates.iter().map(|(id, _)| id.clone()).collect();
        let existing_nodes = self.store.get_nodes_by_ids(&ids).await.map_err(|e| {
            NodeServiceError::bulk_operation_failed(format!(
                "Failed to batch fetch nodes for validation: {}",
                e
            ))
        })?;

        // Step 2: Build the MERGED update candidate for each node, validate it, and
        // record the property-change set for the event.
        //
        // Previously bulk_update wholesale-REPLACED properties with the raw
        // client value (`updated.properties = properties.clone()`) and emitted an
        // empty `changed_properties`. That diverged from the single-update path
        // (which normalizes flat client props → deep-merges into the existing
        // namespaced props) AND silently no-opped every property-change-driven
        // subscriber/play rule. Mirror single-update here: normalize + deep-merge,
        // validate the merged candidate, persist the merged value, and emit the real
        // `changed_properties` computed from old→new.
        //
        // Validation snapshot is taken before the store transaction; any concurrent
        // write landing between here and store.bulk_update is overwritten — intentional
        // under the last-write-wins contract (see store doc).
        let mut merged_updates: Vec<(String, crate::models::NodeUpdate)> =
            Vec::with_capacity(updates.len());
        let mut pending_events: Vec<(String, Node, Vec<crate::db::events::PropertyChange>)> =
            Vec::with_capacity(updates.len());
        // Nodes this batch archives or unarchives: their embedding roots are
        // re-queued once the batch commits (ADR-087 §2).
        #[cfg(feature = "nlp")]
        let mut participation_changed: Vec<String> = Vec::new();

        let mut schemas: std::collections::HashMap<String, Option<crate::models::SchemaNode>> =
            std::collections::HashMap::new();
        let mut ownership_by_type = std::collections::HashMap::new();
        for (id, update) in &updates {
            let existing = existing_nodes
                .get(id)
                .ok_or_else(|| NodeServiceError::node_not_found(id))?;

            let mut updated = existing.clone();
            let mut node_type_changed = false;
            let mut content_changed = false;
            if let Some(node_type) = &update.node_type {
                node_type_changed = updated.node_type != *node_type;
                updated.node_type = node_type.clone();
            }
            if let Some(content) = &update.content {
                content_changed = updated.content != *content;
                updated.content = content.clone();
            }

            // NOTE: Sibling ordering is handled via the has_child order field; bulk
            // updates don't reorder — use move_node.

            // Baseline for the property diff, captured unconditionally —
            // not just when `update.properties` is `Some`. `rebucket_and_validate`
            // below can move an inherited field between buckets even on a
            // content-only/lifecycle-only update (self-healing a stale bucket
            // layout on every write, same as the single-node update paths), so
            // `updated.properties` may end up mutated either way.
            let old_props = existing.properties.clone();

            let mut properties_changed = false;
            if let Some(properties) = &update.properties {
                properties_changed = true;
                if crate::models::CoreNodeType::Schema.is_exactly(&updated.node_type) {
                    // Schema nodes use a flat (non-namespaced) format — deep-merge as-is.
                    Self::deep_merge_namespaced_properties(
                        &mut updated.properties,
                        properties.clone(),
                    );
                } else {
                    let normalized = Self::normalize_flat_properties_to_namespace(
                        &updated.node_type,
                        properties,
                    );
                    Self::deep_merge_namespaced_properties(&mut updated.properties, normalized);
                }
            }

            // Validate the MERGED candidate (PROTECTED + USER-EXTENSIBLE rules),
            // re-bucketing by declaring owner across the `extends` chain
            // (ADR-078) before persisting — same sequence as the single-node
            // update paths, via `rebucket_and_validate`, `node_type_changed`
            // included so a type change defaults the new type's missing
            // fields before validating (mirrors crud.rs's `update_node_unchecked`
            // et al.). `changed_properties` is computed AFTER this (not from
            // `old_props` directly above) so it diffs against the properties
            // that actually land in storage, not a pre-rebucket snapshot; for
            // an unextended type `rebucket_and_validate` is a no-op reshuffle,
            // so this changes nothing for the common case.
            Self::ensure_schema_core_status_unchanged(existing, &updated)?;
            Self::ensure_schema_structure_unchanged(existing, &updated)?;
            self.ensure_retype_allowed(None, existing, &updated).await?;
            if update.content.is_some() || node_type_changed {
                if !schemas.contains_key(&updated.node_type) {
                    let schema = self.title_schema(&updated.node_type).await;
                    schemas.insert(updated.node_type.clone(), schema);
                }
                Self::reject_content_on_templated_type(
                    &updated,
                    schemas.get(&updated.node_type).and_then(Option::as_ref),
                )
                .map_err(|e| {
                    NodeServiceError::bulk_operation_failed(format!(
                        "Failed to validate node {}: {}",
                        id, e
                    ))
                })?;
            }
            let ownership = self
                .field_ownership_in_batch(&updated.node_type, &mut ownership_by_type)
                .await;
            ownership
                .and_then(|ownership| {
                    self.rebucket_and_validate_with(&mut updated, ownership, node_type_changed)
                })
                .map_err(|e| {
                    NodeServiceError::bulk_operation_failed(format!(
                        "Failed to validate node {}: {}",
                        id, e
                    ))
                })?;
            // A play's suspension is the engine's, on this path as on every
            // other (ADR-087 §5).
            self.settle_play_update(
                existing,
                &mut updated,
                Self::patch_enables_play(update.properties.as_ref()),
            )
            .await?;

            let changed_properties =
                super::compute_property_changes(&old_props, &updated.properties);

            // Recompute the title exactly when content, type, or properties
            // change — the same trigger the single-node update paths use
            // (`update_with_version_check_returning_node_in_tx`) — so a
            // batched update re-renders a `title_template` the same way a
            // live per-row edit does. This intentionally does
            // NOT read the caller's own `update.title`: no NodeService-level
            // update path honors a caller-supplied title (only the
            // lower-level `SqliteStore::update_node` does), so bulk_update
            // doesn't either — the previous verbatim passthrough had no
            // per-row analog and was the actual bug.
            let title_update = if content_changed || node_type_changed || properties_changed {
                Some(self.compute_title(&updated, None).await?)
            } else {
                None
            };

            // Persist the caller's intent for type/content/title/lifecycle. Properties
            // are always re-persisted with the current (possibly rebucketed) value —
            // not just when the caller's update touched them — so a bucket move made
            // above by `rebucket_and_validate` is never silently dropped from storage
            // while still appearing in the `NodeUpdated` event below; the store's
            // `COALESCE` only skips a column on a literal `None`; writing the current
            // value back is a no-op for a genuinely untouched, unextended node's
            // properties (same posture as the single-node update paths, which always
            // send `Some(updated.properties.clone())`).
            merged_updates.push((
                id.clone(),
                crate::models::NodeUpdate {
                    node_type: update.node_type.clone(),
                    content: update.content.clone(),
                    properties: Some(updated.properties.clone()),
                    title: title_update.clone(),
                    lifecycle_status: update.lifecycle_status.clone(),
                },
            ));
            // The node as the store will hold it once this update lands —
            // what an invariant rule's condition must read, the same as the
            // single-node update path's store-returned node. Reflects the
            // recomputed title too, so a rule reading `title` sees the final
            // rendered value, not the pre-update one.
            updated.version = existing.version + 1;
            updated.modified_at = chrono::Utc::now();
            if let Some(new_title) = &title_update {
                updated.title = new_title.clone();
            }
            if let Some(status) = &update.lifecycle_status {
                updated.lifecycle_status = status.clone();
            }
            #[cfg(feature = "nlp")]
            if crate::governance::participation_changed(existing, &updated) {
                participation_changed.push(id.clone());
            }
            pending_events.push((id.clone(), updated, changed_properties));
        }

        // Step 3: All validations passed — perform the bulk update, then emit
        // one NodeUpdated per node carrying the real changed_properties, then
        // run invariant-rule dispatch (ADR-060 §2) keyed on those changes, all
        // in one transaction. A rejected node rolls back the whole batch and
        // its buffered events, and its error is returned as-is — a
        // `PlayRuleRejected` exactly as single-node `update_node` returns it.
        let service = self.clone();
        self.with_transaction(move |tx| {
            Box::pin(async move {
                crate::db::SqliteStore::bulk_update_in_tx(tx.store_tx(), merged_updates)
                    .await
                    .map_err(|e| {
                        NodeServiceError::bulk_operation_failed(format!(
                            "Failed to execute bulk update transaction: {}",
                            e
                        ))
                    })?;
                for (id, node, changed_properties) in &pending_events {
                    service.emit_event_in_tx(
                        tx,
                        DomainEvent::NodeUpdated {
                            node_id: id.clone(),
                            node_type: node.node_type.clone(),
                            node: node.clone(),
                            changed_properties: changed_properties.clone(),
                        },
                    );
                }
                for (_, node, changed_properties) in &pending_events {
                    service
                        .dispatch_invariant_rules_for_update_in_tx(tx, node, changed_properties)
                        .await?;
                }
                Ok(())
            })
        })
        .await?;

        // An unarchived node is embedded again, and an archived child leaves
        // its root's aggregate. An archived root's own vectors went with the
        // write. Best-effort and after the commit, like every queueing path.
        #[cfg(feature = "nlp")]
        for id in &participation_changed {
            self.queue_root_for_embedding(id).await;
        }

        Ok(())
    }

    /// Bulk delete multiple nodes in a transaction
    ///
    /// Deletes multiple nodes atomically. If any deletion fails, the entire
    /// transaction is rolled back.
    ///
    /// # Arguments
    ///
    /// * `ids` - Vector of node IDs to delete
    ///
    /// # Errors
    ///
    /// Returns error if any deletion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let ids = vec!["node-1".to_string(), "node-2".to_string()];
    /// service.bulk_delete(ids).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_delete(&self, ids: Vec<String>) -> Result<(), NodeServiceError> {
        if ids.is_empty() {
            return Ok(());
        }

        // Delete in ONE transaction so the documented all-or-nothing contract
        // actually holds. The old loop called store.delete_node per id (each its own
        // autocommit), so a failure on the Nth left the first N-1 committed while the
        // caller got Err and reasonably assumed nothing was deleted → orphaned state /
        // double-delete on retry. Coalesce the Deleted events: one per node.
        let _batch = self.begin_batch_emit();
        self.store
            .bulk_delete(&ids, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::bulk_operation_failed(format!(
                    "Failed to bulk delete nodes: {}",
                    e
                ))
            })?;
        // _batch drops here → one flush per deleted node

        Ok(())
    }
}
