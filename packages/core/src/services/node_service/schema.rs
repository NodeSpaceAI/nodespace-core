//! Schema-related operations for NodeService.

use super::*;
use crate::models::schema::RelationshipDirection;

/// Result of [`NodeService::update_task_node_in_tx`] — the task-node twin of
/// `crud.rs`'s `VersionCheckedUpdateOutcome`. See that type's own doc for why
/// a version conflict is `Ok` rather than `Err`. `Updated` is boxed simply
/// because `TaskNode` is a large, non-`Copy` struct worth keeping off the
/// stack when this variant is passed around — unlike `VersionCheckedUpdateOutcome`,
/// there's no zero-size sibling variant here for the boxing to protect from
/// paying `TaskNode`'s size (`VersionConflict(i64)` is already small).
pub(crate) enum TaskVersionCheckedUpdateOutcome {
    VersionConflict(i64),
    Updated(Box<crate::models::TaskNode>),
}

impl NodeService {
    /// Query nodes by type with optional lifecycle_status filter.
    ///
    /// Used by the playbook engine to load all active plays at startup.
    /// If `lifecycle_status` is `None`, returns all lifecycle statuses.
    pub async fn query_nodes_by_type(
        &self,
        node_type: &str,
        lifecycle_status: Option<&str>,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let query = crate::models::NodeQuery {
            node_type: Some(node_type.to_string()),
            ..Default::default()
        };

        let nodes = self
            .store
            .query_nodes(query)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // In-memory filter: NodeQuery doesn't support lifecycle_status yet.
        // Acceptable for desktop (low play counts). If scaling becomes
        // a concern, add lifecycle_status to NodeQuery/SqliteStore query.
        let filtered: Vec<Node> = if let Some(status) = lifecycle_status {
            nodes
                .into_iter()
                .filter(|n| n.lifecycle_status == status)
                .collect()
        } else {
            nodes
        };

        Ok(filtered)
    }

    /// Find an existing node whose value on a uniqueness-flagged field matches
    /// `value` for the given `node_type`.
    ///
    /// This is the single read-only entry point behind the `unique` schema rule.
    /// It resolves the `unique` / `uniqueCaseInsensitive` flags from the type's
    /// schema fields, and only if the field is flagged does it look for a
    /// conflicting active node. A match is a normal result, not an error: this
    /// method never mutates and never fails on a hit. Callers use it to *suggest*
    /// a likely duplicate (so the UI can show the existing node's name) — writes
    /// are never rejected on a collision. Uniqueness is scoped per-database
    /// (ADR-053); email in particular is a claim, not an identity key.
    ///
    /// `exclude_id` excludes a node from matching itself — pass the id of the
    /// node currently being edited/created so an in-progress write that has
    /// already landed its own copy of `value` (e.g. a UI that checks after
    /// saving, or a re-check on an unchanged value) can't spuriously surface
    /// itself as "the" existing duplicate, silently hiding a real one. Without
    /// it, `None` behaves exactly as before this parameter was added.
    ///
    /// Returns `Ok(None)` when the field is not flagged unique, when `value` is
    /// empty/whitespace, or when no conflicting node exists.
    pub async fn find_duplicate_for(
        &self,
        node_type: &str,
        field: &str,
        value: &str,
        exclude_id: Option<&str>,
    ) -> Result<Option<Node>, NodeServiceError> {
        // Empty/whitespace values are never treated as a duplicate.
        if value.trim().is_empty() {
            return Ok(None);
        }

        // Resolved via `resolve_field_owners` rather than a direct
        // `get_schema_node(node_type)` lookup: the latter returns only
        // `node_type`'s own directly-declared fields, not the ADR-078
        // `extends`-chain-merged set. A `unique`/`uniqueCaseInsensitive`
        // field declared only on an ancestor schema and inherited (not
        // redeclared) by a subtype was therefore invisible here, so a real
        // conflicting value on a subtype instance never surfaced a duplicate
        // suggestion. Same fix pattern as `workflow_state.rs`,
        // `validation.rs`, `graph_resolver.rs`'s
        // `is_declared_many_relationship`, and `rel_ops.rs`'s
        // `resolve_relationship_name`/`get_node_relationships`. When
        // `node_type` has no schema at all, `resolve_field_owners` returns
        // an empty field set — same outcome the old direct lookup produced
        // for a missing schema.
        let (fields, owners, _chain) = self.resolve_field_owners(node_type).await?;

        let flags = fields.iter().find(|f| f.name == field).map(|f| {
            (
                f.unique.unwrap_or(false),
                f.unique_case_insensitive.unwrap_or(false),
            )
        });

        let (is_unique, case_insensitive) = match flags {
            Some(flags) => flags,
            None => return Ok(None),
        };

        if !is_unique {
            return Ok(None);
        }

        // An inherited field's value is stored under its owning ancestor
        // schema's bucket, not `node_type`'s own bucket
        // (`bucket_properties_by_owner`, ADR-078) — `owners` (from the same
        // `resolve_field_owners` call above) says which. Falls back to
        // `node_type` for the common, unextended case.
        let bucket = owners.get(field).map(String::as_str).unwrap_or(node_type);
        let conflicting_id = self
            .store
            .find_conflicting_unique(
                node_type,
                bucket,
                field,
                value,
                exclude_id,
                case_insensitive,
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        match conflicting_id {
            Some(id) => self
                .store
                .get_node(&id)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string())),
            None => Ok(None),
        }
    }

    /// Get schema definition for a given node type
    pub async fn get_schema_for_type(
        &self,
        node_type: &str,
    ) -> Result<Option<serde_json::Value>, NodeServiceError> {
        self.store
            .get_schema(node_type)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get a task node with strong typing
    ///
    /// Returns strongly-typed `TaskNode` instead of generic `Node`.
    ///
    /// # Arguments
    ///
    /// * `id` - The task node ID
    ///
    /// # Returns
    ///
    /// * `Ok(Some(TaskNode))` - Task found with strongly-typed fields
    /// * `Ok(None)` - Task not found
    /// * `Err(_)` - Service error
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
    /// if let Some(task) = service.get_task_node("my-task-id").await? {
    ///     // Direct field access - no JSON parsing
    ///     println!("Status: {:?}", task.status);
    ///     println!("Content: {}", task.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_task_node(
        &self,
        id: &str,
    ) -> Result<Option<crate::models::TaskNode>, NodeServiceError> {
        self.store.get_task_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Failed to get task node '{}': {}", id, e),
            })
        })
    }

    /// Update a task node with type-safe field updates
    ///
    /// Updates task-specific fields (status, priority, due_date).
    /// Uses optimistic concurrency control (OCC) to prevent lost updates.
    ///
    /// # Type Safety
    ///
    /// This method provides end-to-end type safety for task updates:
    /// - Frontend sends strongly-typed `TaskNodeUpdate` (not generic NodeUpdate)
    /// - Backend updates task fields directly (not via JSON properties)
    /// - Returns strongly-typed `TaskNode` with updated fields
    ///
    /// # Arguments
    ///
    /// * `id` - The task node ID
    /// * `expected_version` - Version for OCC check (prevents lost updates)
    /// * `update` - TaskNodeUpdate with fields to update
    ///
    /// # Returns
    ///
    /// * `Ok(TaskNode)` - Updated task with new version
    /// * `Err(VersionMismatch)` - Version conflict, refresh and retry
    /// * `Err(NodeNotFound)` - Task doesn't exist
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::models::{TaskNodeUpdate, TaskStatus};
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Update task status
    /// let update = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
    /// let task = service.update_task_node("task-123", 1, update).await?;
    /// println!("New status: {:?}", task.status);
    /// println!("New version: {}", task.version);
    /// # Ok(())
    /// # }
    /// ```
    /// Validate a `TaskStatus` against `task.status`'s declared vocabulary
    /// (`core_values` + `user_values`), mirroring `validate_node_with_fields`'s
    /// enum check (`crud.rs`). `TaskStatus::from_str` is infallible by
    /// construction — any unrecognized string becomes `TaskStatus::User(_)` —
    /// so this is the only place `update_task_node` actually rejects a value
    /// the schema hasn't declared.
    async fn validate_task_status(
        &self,
        status: &crate::models::TaskStatus,
    ) -> Result<(), NodeServiceError> {
        let schema = self
            .get_schema_node("task")
            .await?
            .ok_or_else(|| NodeServiceError::invalid_update("Schema 'task' not found"))?;

        let valid_values = schema.get_enum_values("status").unwrap_or_default();
        let status_str = status.as_str();
        let is_valid = valid_values.iter().any(|ev| ev.value == status_str);

        if !is_valid {
            let valid_labels: Vec<_> = valid_values
                .iter()
                .map(|ev| format!("{} ({})", ev.label, ev.value))
                .collect();
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid value '{}' for enum field 'status'. Valid values: {}",
                status_str,
                valid_labels.join(", ")
            )));
        }

        Ok(())
    }

    pub async fn update_task_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::TaskNodeUpdate,
    ) -> Result<crate::models::TaskNode, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "TaskNodeUpdate contains no changes",
            ));
        }

        // Enforce `status` against the schema's declared vocabulary
        // (core_values + user_values) — the same check `validate_node_with_fields`
        // already performs for schema-only types (ADR-076). `update_task_node`
        // is the sole call path into the store-layer write (confirmed: no other
        // caller reaches it directly), and the store layer trusts this having
        // already run rather than re-validating itself. Read-only and schema-scoped
        // (not node-scoped), so it's safe to run before the transaction opens below —
        // same posture as `update_with_version_check_returning_node`'s own pre-tx checks.
        if let Some(ref status) = update.status {
            self.validate_task_status(status).await?;
        }

        let service = self.clone();
        let service_for_tx = service.clone();
        let id_for_tx = id.to_string();
        let outcome = service
            .with_transaction(move |tx| {
                Box::pin(async move {
                    service_for_tx
                        .update_task_node_in_tx(tx, &id_for_tx, expected_version, update)
                        .await
                })
            })
            .await?;

        match outcome {
            TaskVersionCheckedUpdateOutcome::Updated(task) => Ok(*task),
            TaskVersionCheckedUpdateOutcome::VersionConflict(actual_version) => {
                Err(NodeServiceError::VersionConflict {
                    node_id: id.to_string(),
                    expected_version,
                    actual_version,
                })
            }
        }
    }

    /// Tx-scoped twin of [`Self::update_task_node`] (ADR-060 §2) — the
    /// `update_task_node` counterpart to `crud.rs`'s
    /// `update_with_version_check_returning_node_in_tx`. Same
    /// read-existing → compute title → version-checked write → diff →
    /// buffer event → synchronous invariant dispatch → re-read final state
    /// pipeline as that method; see its own doc for why each step is
    /// ordered the way it is (in particular, why the event is buffered
    /// *before* dispatch, and why the final state is re-read rather than
    /// returning the pre-dispatch snapshot — an invariant rule's action can
    /// self-referentially write back to this same node).
    ///
    /// Reads `existing` and computes `title_update` from it here, inside
    /// `tx`, rather than reusing a pre-transaction snapshot — the same
    /// posture `update_with_version_check_returning_node_in_tx` takes, so a
    /// concurrent write landing between a hypothetical pre-tx read and this
    /// tx's write can never leave the title computed against stale content.
    pub(crate) async fn update_task_node_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        id: &str,
        expected_version: i64,
        update: crate::models::TaskNodeUpdate,
    ) -> Result<TaskVersionCheckedUpdateOutcome, NodeServiceError> {
        let existing = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // Sync the indexed `title` column, mirroring the generic update path's guard
        // (`content_changed || properties_changed`, see crud.rs). A task-schema
        // `title_template` makes the title depend on task properties as well as
        // content, so recomputing on content alone would leave the title stale after
        // a property-only change, and a combined content+property update must compute
        // from the *fully-merged* node (not a pre-update snapshot) or the title lands
        // one write behind. We build the post-update node with the same shared merge
        // the store performs and compute the title from it. When no template is set
        // (the built-in "task" schema today), compute_title falls through to
        // `strip_markdown(content)`, so a property-only update recomputes to the same
        // value — a harmless no-op write, not a behavior change.
        let content_changed = update
            .content
            .as_ref()
            .is_some_and(|new_content| new_content != &existing.content);

        let title_update = if content_changed || update.has_property_fields() {
            let mut merged = existing.clone();
            if let Some(ref new_content) = update.content {
                merged.content = new_content.clone();
            }
            update.apply_to_properties(&mut merged.properties);
            self.compute_title(&merged, None).await?
        } else {
            None
        };

        let result = crate::db::SqliteStore::update_task_node_with_version_check_in_tx(
            tx.store_tx(),
            id,
            expected_version,
            update,
            title_update,
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let updated_node = match result {
            Ok(node) => node,
            Err(actual_version) => {
                return Ok(TaskVersionCheckedUpdateOutcome::VersionConflict(
                    actual_version,
                ))
            }
        };

        // Real changed_properties, diffed from the namespaced `properties.task.*`
        // storage shape — see `compute_property_changes`'s own doc. Required here
        // for the same reason it's required in the generic update path: an
        // `_in_tx` store write bypasses the store's own notifier, so this is the
        // only source of an accurate diff for property_changed-triggered plays
        // (reactive or invariant) and WatchNodes consumers.
        let changed_properties =
            compute_property_changes(&existing.properties, &updated_node.properties);

        // Buffered, not broadcast yet (`BatchState::Transactional`) — only
        // flushed if this whole transaction commits.
        self.emit_event(DomainEvent::NodeUpdated {
            node_id: updated_node.id.clone(),
            node_type: updated_node.node_type.clone(),
            node: updated_node.clone(),
            changed_properties: changed_properties.clone(),
        });

        // ADR-060 §2: synchronous invariant-rule dispatch for property_changed
        // triggers, inside this same transaction — the exact gap this issue
        // closes (a Task's `status` change is the motivating example). A
        // rejecting rule's `Err` propagates out through the `?` below, through
        // this whole function, and through the caller's `with_transaction`,
        // rolling back everything above — including the buffered event.
        self.dispatch_invariant_rules_for_update_in_tx(tx, &updated_node, &changed_properties)
            .await?;

        // Re-read the trigger node's final state, tx-consistent, rather than
        // converting `updated_node` directly — an invariant rule's own action
        // can be a self-referential `update_node` on the SAME node, writing a
        // second time inside this same transaction (see
        // `update_with_version_check_returning_node_in_tx`'s identical
        // re-read for the full rationale).
        let final_node = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // An invariant rule's action is a generic `update_node` with no
        // guard against changing `node_type` — unlike the rest of this
        // pipeline, which is task-shape-preserving by construction. If a
        // rule's own self-referential action retypes the trigger node away
        // from "task" (a deliberately unusual thing for a rule to do, and
        // not the shape any known rule uses today), this conversion fails
        // and the whole transaction rolls back via the `?` below — a safe,
        // no-partial-write outcome, just surfaced as a generic
        // `invalid_update` rather than an invariant-specific error variant.
        let task_node = crate::db::SqliteStore::node_to_task_node(final_node).ok_or_else(|| {
            NodeServiceError::invalid_update(format!(
                "Node '{}' is no longer a task node after update",
                id
            ))
        })?;

        Ok(TaskVersionCheckedUpdateOutcome::Updated(Box::new(
            task_node,
        )))
    }

    /// Get a schema node with strong typing
    ///
    /// Returns strongly-typed `SchemaNode` instead of generic `Node`.
    ///
    /// # Arguments
    ///
    /// * `id` - The schema node ID (e.g., "task", "date")
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SchemaNode))` - Schema found with strongly-typed fields
    /// * `Ok(None)` - Schema not found
    /// * `Err(_)` - Service error
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
    /// if let Some(schema) = service.get_schema_node("task").await? {
    ///     // Direct field access - no JSON parsing
    ///     println!("Is core: {}", schema.is_core);
    ///     println!("Fields: {:?}", schema.fields.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_schema_node(
        &self,
        id: &str,
    ) -> Result<Option<crate::models::SchemaNode>, NodeServiceError> {
        self.store.get_schema_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Failed to get schema node '{}': {}", id, e),
            })
        })
    }

    /// A node type's `extends` chain, nearest scope first (ADR-078).
    ///
    /// `["issue", "task"]` for an issue extending task; `["task"]` for an
    /// unextended type, which is every type in the system until something
    /// declares `extends`. The ordering is the contract every consumer
    /// depends on — it is both the property-bucket read order (a nearer
    /// bucket wins a key collision) and the scope-projection order.
    ///
    /// Resolved live rather than cached. The hot paths that cannot afford
    /// that (trigger matching, per-row filter evaluation) own their own
    /// caches over the same edges; this is the uncached resolver for write
    /// paths and one-off reads.
    pub async fn resolve_type_chain(
        &self,
        node_type: &str,
    ) -> Result<Vec<String>, NodeServiceError> {
        // One query for every extends edge, rather than an async walk one
        // parent at a time. The map holds one entry per *extending* schema,
        // so it is empty in a database where nothing extends anything.
        let parent_map = self.store.get_extends_parent_map().await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to load extends edges: {e}"))
        })?;

        if parent_map.is_empty() {
            return Ok(vec![node_type.to_string()]);
        }

        let lookup = move |id: &str| parent_map.get(id).cloned();
        Ok(crate::schema::extends_chain::resolve_ancestor_chain(
            node_type, &lookup,
        ))
    }

    /// Which schema in `node_type`'s chain declares each field, and the
    /// effective field set, resolved together.
    ///
    /// The write path needs both at once: the field list to validate and
    /// default against, and the owning schema per field to decide which
    /// bucket a value is stored under. Resolving them in one pass avoids
    /// walking the chain twice for a single write.
    ///
    /// Returns `(effective_fields, field_name -> owning_schema_id, chain)`.
    ///
    /// The chain comes back too because every caller that buckets also needs
    /// to *read* across those same buckets when validating, and resolving it
    /// twice would mean two passes over the same edges.
    pub async fn resolve_field_owners(
        &self,
        node_type: &str,
    ) -> Result<
        (
            Vec<crate::models::SchemaField>,
            std::collections::HashMap<String, String>,
            Vec<String>,
        ),
        NodeServiceError,
    > {
        let chain = self.resolve_type_chain(node_type).await?;

        let mut owners: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut chain_fields: Vec<Vec<crate::models::SchemaField>> =
            Vec::with_capacity(chain.len());

        for schema_id in &chain {
            let Some(schema) = self.get_schema_node(schema_id).await? else {
                // A missing mid-chain schema contributes nothing rather than
                // failing the write — same posture as the schema-layer
                // resolver, since deletion with a live chain is out of scope.
                continue;
            };
            for field in &schema.fields {
                // First writer wins, and the chain is nearest-first, so a
                // field declared by a nearer scope keeps ownership. Well-formed
                // chains have no collisions (redeclaration is rejected at write
                // time); this only matters for the retroactive-collision edge
                // case ADR-078 leaves unresolved.
                owners
                    .entry(field.name.clone())
                    .or_insert_with(|| schema_id.clone());
            }
            chain_fields.push(schema.fields);
        }

        Ok((
            crate::schema::extends_chain::flatten_chain_fields(chain_fields),
            owners,
            chain,
        ))
    }

    /// The effective/merged relationship set across `node_type`'s extends
    /// chain (ADR-078) — the relationship counterpart to
    /// [`Self::resolve_field_owners`]. A relationship declared only on an
    /// ancestor schema (inherited, not redeclared) is returned exactly as if
    /// it were the node's own.
    ///
    /// Nearest-first, first-declared wins on a name collision — the same
    /// shadowing rule [`crate::schema::extends_chain::flatten_chain_fields`]
    /// applies to fields, via the shared
    /// [`crate::schema::extends_chain::flatten_chain_by_name`] generalization
    /// (a relationship carries no `SchemaField`-shaped data, so the two
    /// merges share the dedup shape rather than a field-specific type).
    ///
    /// Excludes the `extends`/`extended_by` type-system relationship
    /// (`is_type_system_relationship`). A schema that declares `extends` has
    /// it stored as an ordinary row in the same declaration table other
    /// relationships live in (see `TYPE_SYSTEM_RELATIONSHIPS`'s doc — it is
    /// deliberately not excluded from *storage* reads, since
    /// `declared_parent`/`declared_extends_parent` need to find it there).
    /// But it is a statement about the schema graph, not a data relationship
    /// any real node instance ever carries — surfacing it here would let a
    /// condition segment literally named `extends`/`extended_by` pass this
    /// function's "is this a real, traversable relationship" check and be
    /// classified `NotYetMet` instead of the correct `Unresolvable`, since no
    /// data node ever has such an edge to eventually satisfy it.
    ///
    /// Returns `(relationships, relationship_name -> owning_schema_id)`,
    /// mirroring [`Self::resolve_field_owners`]'s shape: a caller that needs
    /// to decide whether a given name resolves as a field or a relationship
    /// (e.g. save-time Play-path validation) has to compare *where in the
    /// chain* each kind's declaration lives, not just whether the name is a
    /// member of each independently-merged set — a nearer schema's own
    /// relationship must shadow a farther ancestor's field of the same name,
    /// and vice versa, since extends-chain shadowing is defined per
    /// declared name, not per field-vs-relationship kind. The owners map is
    /// what makes that chain-position comparison possible.
    pub async fn resolve_relationships(
        &self,
        node_type: &str,
    ) -> Result<
        (
            Vec<crate::models::schema::SchemaRelationship>,
            std::collections::HashMap<String, String>,
        ),
        NodeServiceError,
    > {
        let chain = self.resolve_type_chain(node_type).await?;

        let mut owners: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut chain_relationships: Vec<Vec<crate::models::schema::SchemaRelationship>> =
            Vec::with_capacity(chain.len());

        for schema_id in &chain {
            let Some(schema) = self.get_schema_node(schema_id).await? else {
                // A missing mid-chain schema contributes nothing rather than
                // failing the read — same posture as `resolve_field_owners`.
                continue;
            };
            let relationships: Vec<_> = schema
                .relationships
                .into_iter()
                .filter(|rel| !crate::models::schema::is_type_system_relationship(&rel.name))
                .collect();
            for rel in &relationships {
                // First writer wins, and the chain is nearest-first, so a
                // relationship declared by a nearer scope keeps ownership —
                // same rationale as `resolve_field_owners`'s owners loop.
                owners
                    .entry(rel.name.clone())
                    .or_insert_with(|| schema_id.clone());
            }
            chain_relationships.push(relationships);
        }

        Ok((
            crate::schema::extends_chain::flatten_chain_by_name(chain_relationships, |r| {
                r.name.as_str()
            }),
            owners,
        ))
    }

    /// Rename a field across all node instances and update the schema definition.
    ///
    /// Only `name` is rewritten — `friendly_name` is left exactly as stored,
    /// even when it was originally auto-derived from the old `name` and is
    /// now stale (e.g. renaming `priority` to `urgency_level` leaves a
    /// `friendly_name` of "Priority" pointing at the new key). This is
    /// deliberate: a stored `friendly_name` may equally have been an
    /// explicit caller choice, and silently re-deriving it on every rename
    /// risks clobbering that choice with no way to tell the two cases apart
    /// after the fact. A rename that wants an updated label passes one via
    /// `update_schema`'s field-update path instead.
    pub async fn rename_schema_field(
        &self,
        type_id: &str,
        from: &str,
        to: &str,
    ) -> Result<u64, NodeServiceError> {
        // ADR-069 §1b/S3, closing F3: the data migration and the schema
        // definition rewrite land in ONE transaction. Previously two
        // independent atomic writes — a failure rewriting the definition
        // left every instance's property data rekeyed to `to` while the
        // schema still declared `from`, breaking `compute_title`, CEL
        // expressions, and query filters for the whole type. Any failure now
        // rolls back both.
        let type_id_owned = type_id.to_string();
        let from_owned = from.to_string();
        let to_owned = to.to_string();
        let service = self.clone();
        let service_for_tx = service.clone();
        let affected: u64 = service
            .with_transaction(move |tx| {
                let service = service_for_tx.clone();
                let type_id = type_id_owned.clone();
                let from = from_owned.clone();
                let to = to_owned.clone();
                Box::pin(async move {
                    // Step 0: Validate under the write guard, against the
                    // schema Step 2 rewrites — see the helper's doc.
                    let schema = service
                        .validate_schema_field_rename(&type_id, &from, &to)
                        .await?;

                    // Step 1: Migrate all node property data
                    let affected = crate::db::SqliteStore::rename_schema_field_in_tx(
                        tx.store_tx(),
                        &type_id,
                        &from,
                        &to,
                    )
                    .await
                    .map_err(|e| {
                        NodeServiceError::DatabaseError(
                            crate::db::DatabaseError::SqlExecutionError {
                                context: format!(
                                    "Failed to migrate field data '{}' -> '{}' for type '{}': {}",
                                    from, to, type_id, e
                                ),
                            },
                        )
                    })?;

                    // Step 2: Update schema definition — rename field in the fields list
                    let updated_fields: Vec<crate::models::schema::SchemaField> = schema
                        .fields
                        .into_iter()
                        .map(|mut f| {
                            if f.name == from {
                                f.name = to.clone();
                            }
                            f
                        })
                        .collect();

                    // Declarations live in the relationship table, not in
                    // properties — the rebuilt properties carry fields only.
                    let mut properties = serde_json::json!({
                        "isCore": schema.is_core,
                        "schemaVersion": schema.schema_version,
                        "fields": updated_fields,
                    });
                    if let Some(ref t) = schema.title_template {
                        properties["titleTemplate"] = serde_json::Value::String(t.clone());
                    }
                    if let Some(ref t) = schema.properties_header_summary_template {
                        properties["propertiesHeaderSummaryTemplate"] =
                            serde_json::Value::String(t.clone());
                    }

                    let update = crate::models::NodeUpdate {
                        properties: Some(properties),
                        ..Default::default()
                    };

                    service
                        .update_node_unchecked_in_tx(tx, &type_id, update)
                        .await
                        .map_err(|e| {
                            NodeServiceError::DatabaseError(
                                crate::db::DatabaseError::SqlExecutionError {
                                    context: format!(
                                        "Failed to update schema definition after field rename \
                                         '{}' -> '{}' for type '{}': {}",
                                        from, to, type_id, e
                                    ),
                                },
                            )
                        })?;

                    Ok(affected)
                })
            })
            .await?;

        Ok(affected)
    }

    /// Everything `rename_schema_field` checks before migrating anything:
    /// the schema exists, `from` is a User-protected field it declares, and
    /// `to` collides with neither its extends-chain-merged effective field
    /// set nor any descendant's own field. Returns the schema the checks ran
    /// against, which Step 2 then rewrites.
    ///
    /// Must be called from INSIDE `rename_schema_field`'s `with_transaction`
    /// closure, before its first write. The reads below go through pooled
    /// readers, which see the latest committed state; the store's single
    /// writer guard is held for the whole closure, so no other schema
    /// mutation (an `add_fields` via `update_schema`, a concurrent rename)
    /// can commit between these checks and the writes that rely on them.
    /// Validating before the transaction opened left exactly that window,
    /// and Step 2's wholesale `fields` rewrite then silently discarded
    /// whatever landed in it. Pooled readers cannot see this transaction's
    /// own writes, which is why this runs before any of them.
    ///
    /// This closes only the rename's OWN read-then-overwrite window. Every
    /// other writer that rebuilds `fields` from a read has to close its own:
    /// `update_schema_field_friendly_name` reads inside its transaction the
    /// same way, and `handle_update_schema` (whose read runs outside it)
    /// version-checks the schema node before writing.
    async fn validate_schema_field_rename(
        &self,
        type_id: &str,
        from: &str,
        to: &str,
    ) -> Result<crate::models::SchemaNode, NodeServiceError> {
        // Validate schema exists
        let schema = self
            .get_schema_node(type_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(type_id))?;

        // Validate source field exists in schema
        if !schema.fields.iter().any(|f| f.name == from) {
            return Err(NodeServiceError::invalid_update(format!(
                "Field '{}' not found in schema '{}'",
                from, type_id
            )));
        }

        // Reject renaming a Core/System-protected field. `can_modify_field`
        // exists exactly for this — only a `User`-protected field's storage
        // key may change. Checked before the migration below runs: renaming
        // rekeys every existing node's property data and rewrites the schema
        // as it goes, so a check that ran afterwards would find the damage
        // already durably applied.
        if !schema.can_modify_field(from) {
            let protection = schema
                .get_field(from)
                .map(|f| f.protection.clone())
                .unwrap_or_default();
            return Err(NodeServiceError::invalid_update(format!(
                "Field '{}' in schema '{}' is {}-protected and cannot be renamed — only \
                 User-protected fields may be renamed. Core and System fields are immutable \
                 through update_schema.",
                from, type_id, protection
            )));
        }

        // Validate destination field does not already exist — checked
        // against the extends-chain-merged effective set (own fields plus
        // every ancestor's), not `schema.fields` alone, so a rename cannot
        // shadow an inherited field the same way `validate_no_field_redeclaration`
        // already blocks a *new* field declaration from doing.
        let (effective_fields, field_owners, _chain) = self.resolve_field_owners(type_id).await?;
        if effective_fields.iter().any(|f| f.name == to) {
            let declaring_schema = field_owners.get(to).map(String::as_str).unwrap_or(type_id);
            return Err(NodeServiceError::invalid_update(format!(
                "Field '{}' already exists in schema '{}' (declared by '{}' — own field or \
                 inherited via extends); cannot rename to an existing field",
                to, type_id, declaring_schema
            )));
        }

        // Also reject a destination colliding with a DESCENDANT's own field.
        // The data migration below rekeys every instance in `type_id`'s
        // descendant closure (ADR-078: a subtype stores an inherited field
        // under the SAME bucket key as its ancestor), always under `type_id`'s
        // own bucket. A descendant schema's own field of the same name is a
        // different DB bucket, but the SAME extends-chain-merged effective
        // name — and nearest-first shadowing means the descendant's own
        // declaration would permanently win over the freshly-renamed
        // ancestor field in every effective-field view (compute_title, CEL,
        // query filters) for that descendant's instances, with no error ever
        // raised. Checked against each descendant's own `fields` (not its
        // resolved effective set): ADR-078 redeclaration checks already
        // guarantee no descendant redeclares anything `type_id` currently
        // owns, so only a descendant's OWN name can newly collide here.
        let subtypes = self.store.get_subtype_closure(type_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!(
                "Failed to resolve descendant closure for '{type_id}': {e}"
            ))
        })?;
        for descendant_id in subtypes.iter().filter(|id| id.as_str() != type_id) {
            let Some(descendant_schema) = self.get_schema_node(descendant_id).await? else {
                continue;
            };
            if descendant_schema.fields.iter().any(|f| f.name == to) {
                return Err(NodeServiceError::invalid_update(format!(
                    "Field '{}' already exists as '{}''s own field ('{}' extends '{}'); \
                     renaming would permanently shadow the ancestor's field in every \
                     effective-field view for '{}' instances. Choose a different destination \
                     name.",
                    to, descendant_id, descendant_id, type_id, descendant_id
                )));
            }
        }

        Ok(schema)
    }

    /// Update a field's `friendly_name` in place, without touching `name` or
    /// any node's property data.
    ///
    /// The display-only counterpart to [`rename_schema_field`](Self::rename_schema_field),
    /// whose own doc comment names this the path a caller reaches for when it
    /// wants an updated label without renaming the storage key. No node
    /// property values are read or written — `friendly_name` lives only in
    /// the schema definition itself, so this is a single schema-node update
    /// with no migration step.
    pub async fn update_schema_field_friendly_name(
        &self,
        type_id: &str,
        field_name: &str,
        friendly_name: &str,
    ) -> Result<(), NodeServiceError> {
        // Read, validate and write under one write guard, for the same
        // reason `rename_schema_field` does (see
        // `validate_schema_field_rename`): `fields` is rewritten wholesale
        // from the schema read here, so a rename or `add_fields` committing
        // between an unguarded read and this write would be silently
        // reverted — for a rename, leaving the definition disagreeing with
        // instance data that has already been rekeyed. Pooled reads inside
        // the closure see the latest committed state, and nothing can commit
        // until this transaction does.
        let type_id_owned = type_id.to_string();
        let field_name_owned = field_name.to_string();
        let friendly_name_owned = friendly_name.to_string();
        let service = self.clone();
        let service_for_tx = service.clone();
        service
            .with_transaction(move |tx| {
                let service = service_for_tx.clone();
                let type_id = type_id_owned.clone();
                let field_name = field_name_owned.clone();
                let friendly_name = friendly_name_owned.clone();
                Box::pin(async move {
                    let type_id = type_id.as_str();
                    let field_name = field_name.as_str();
                    let friendly_name = friendly_name.as_str();

                    let schema = service
                        .get_schema_node(type_id)
                        .await?
                        .ok_or_else(|| NodeServiceError::node_not_found(type_id))?;

                    if !schema.fields.iter().any(|f| f.name == field_name) {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Field '{}' not found in schema '{}'",
                            field_name, type_id
                        )));
                    }

                    // Same protection-level guard as `rename_schema_field`'s identity
                    // rename: a Core/System field is immutable through `update_schema`,
                    // and a relabel is a modification of the field definition just as
                    // much as a storage-key rename is.
                    if !schema.can_modify_field(field_name) {
                        let protection = schema
                            .get_field(field_name)
                            .map(|f| f.protection.clone())
                            .unwrap_or_default();
                        return Err(NodeServiceError::invalid_update(format!(
                            "Field '{}' in schema '{}' is {}-protected and cannot be relabeled — only \
                             User-protected fields may be modified. Core and System fields are immutable \
                             through update_schema.",
                            field_name, type_id, protection
                        )));
                    }

                    // `SchemaField::friendly_name` is documented as always populated in
                    // storage, with every reader assuming so unconditionally — a blank
                    // value must never reach it here any more than it can through
                    // `apply_friendly_name_defaults` on create/`add_fields`. Mirrors that
                    // function's treatment of an omitted value exactly: derive from the
                    // field's own name, disambiguating against a sibling field's label
                    // if the derived form collides.
                    let resolved_friendly_name = if friendly_name.trim().is_empty() {
                        let derived = crate::models::schema::derive_friendly_name(field_name);
                        let collides = schema
                            .fields
                            .iter()
                            .any(|f| f.name != field_name && f.friendly_name == derived);
                        if collides {
                            crate::schema::disambiguate_friendly_name(&derived, field_name)
                        } else {
                            derived
                        }
                    } else {
                        friendly_name.to_string()
                    };

                    let updated_fields: Vec<crate::models::schema::SchemaField> = schema
                        .fields
                        .into_iter()
                        .map(|mut f| {
                            if f.name == field_name {
                                f.friendly_name = resolved_friendly_name.clone();
                            }
                            f
                        })
                        .collect();

                    // Same persistence shape as `rename_schema_field`'s Step 2 — fields
                    // only, declarations live in the relationship table and are untouched.
                    let mut properties = serde_json::json!({
                        "isCore": schema.is_core,
                        "schemaVersion": schema.schema_version,
                        "fields": updated_fields,
                    });
                    if let Some(ref t) = schema.title_template {
                        properties["titleTemplate"] = serde_json::Value::String(t.clone());
                    }
                    if let Some(ref t) = schema.properties_header_summary_template {
                        properties["propertiesHeaderSummaryTemplate"] = serde_json::Value::String(t.clone());
                    }

                    let update = crate::models::NodeUpdate {
                        properties: Some(properties),
                        ..Default::default()
                    };


                    service
                        .update_node_unchecked_in_tx(tx, type_id, update)
                        .await
                        .map_err(|e| {
                            NodeServiceError::DatabaseError(
                                crate::db::DatabaseError::SqlExecutionError {
                                    context: format!(
                                        "Failed to update friendly_name for field '{}' on type \
                                         '{}': {}",
                                        field_name, type_id, e
                                    ),
                                },
                            )
                        })
                })
            })
            .await
    }

    /// Replace a schema's relationship declarations — the write path for
    /// declaration edges. `create_schema`/`update_schema` route through here;
    /// core-schema seeding (which runs before a `NodeService` exists) calls
    /// `SqliteStore::set_schema_declarations` directly, which enforces the same
    /// name invariants (reserved builtin names, per-schema uniqueness) at the
    /// store layer.
    ///
    /// Enforces, in order:
    /// 1. **Reserved names** — a declaration may not take a built-in structural
    ///    relationship name (`has_child`, `mentions`, `member_of`, `has_role`):
    ///    declarations and primitives share the one `relationship` table, so a
    ///    collision would corrupt every type-keyed query.
    /// 2. **Live-edge protection** — removing or retargeting a declaration that
    ///    already has instance edges is rejected (block by default, no cascade,
    ///    no detach), naming the number of affected edges.
    ///
    /// Emits one relationship domain event per actual change so declaration
    /// edges replicate exactly like instance edges.
    pub async fn set_schema_relationships(
        &self,
        schema_id: &str,
        relationships: &[crate::models::schema::SchemaRelationship],
    ) -> Result<(), NodeServiceError> {
        for rel in relationships {
            // Both names, for the two distinct reasons spelled out on
            // `reject_reserved_relationship_names`: a reserved forward name
            // makes stored edges ambiguous, a reserved reverse name makes the
            // declaration silently unreachable.
            for (which, name) in [("name", &rel.name), ("reverseName", &rel.reverse_name)] {
                if crate::models::schema::is_reserved_relationship_name(name) {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Relationship {} '{}' is reserved for a built-in structural relationship \
                         ({}); choose a different name",
                        which,
                        name,
                        crate::models::schema::RESERVED_RELATIONSHIP_NAMES.join(", ")
                    )));
                }
            }
        }

        // Block removing/retargeting a declaration that live instance edges
        // depend on. A rename arrives as remove+add, so it is covered by the
        // removal check; a retarget keeps the name but changes target_type.
        let existing = self
            .store
            .get_schema_declarations(schema_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        for old in &existing {
            let replacement = relationships.iter().find(|r| r.name == old.name);
            let removed = replacement.is_none();
            let retargeted = replacement.is_some_and(|r| r.target_type != old.target_type);
            if !(removed || retargeted) {
                continue;
            }
            let live = self
                .store
                .count_instance_edges_for_declaration(schema_id, &old.name)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            if live > 0 {
                let action = if removed { "remove" } else { "retarget" };
                return Err(NodeServiceError::invalid_update(format!(
                    "Cannot {} relationship '{}' on schema '{}': {} instance edge(s) exist \
                     under it. Delete those relationships first.",
                    action, old.name, schema_id, live
                )));
            }
        }

        let changes = self
            .store
            .set_schema_declarations(schema_id, relationships)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        for (rel_id, out_node, rel) in changes.created {
            let props = serde_json::to_value(&rel).unwrap_or_else(|_| serde_json::json!({}));
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id, schema_id, &out_node, &rel.name, props,
                ),
            });
        }
        for (rel_id, out_node, rel) in changes.updated {
            let props = serde_json::to_value(&rel).unwrap_or_else(|_| serde_json::json!({}));
            self.emit_event(DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id, schema_id, &out_node, &rel.name, props,
                ),
            });
        }
        for (rel_id, out_node, name) in changes.deleted {
            self.emit_event(DomainEvent::RelationshipDeleted {
                id: rel_id,
                from_id: crate::db::events::node_thing(schema_id),
                to_id: crate::db::events::node_thing(&out_node),
                relationship_type: name,
            });
        }

        Ok(())
    }

    /// `_in_tx` twin of [`Self::set_schema_relationships`] (ADR-069 §1b/S3).
    /// Identical reserved-name and live-instance-edge validation; the
    /// declaration write lands on `tx.store_tx()` via the store's own
    /// `set_schema_declarations_in_tx` instead of opening a new transaction
    /// — this is what lets `handle_create_schema` compose it into the same
    /// transaction as the schema node create and the description subtree.
    /// The validation reads (`get_schema_declarations`,
    /// `count_instance_edges_for_declaration`) run against the pooled
    /// reader exactly as the standalone method does — safe here because
    /// they read declarations that predate this transaction, not anything
    /// it is concurrently writing.
    pub(crate) async fn set_schema_relationships_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        schema_id: &str,
        relationships: &[crate::models::schema::SchemaRelationship],
    ) -> Result<(), NodeServiceError> {
        for rel in relationships {
            for (which, name) in [("name", &rel.name), ("reverseName", &rel.reverse_name)] {
                if crate::models::schema::is_reserved_relationship_name(name) {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Relationship {} '{}' is reserved for a built-in structural relationship \
                         ({}); choose a different name",
                        which,
                        name,
                        crate::models::schema::RESERVED_RELATIONSHIP_NAMES.join(", ")
                    )));
                }
            }
        }

        let existing = self
            .store
            .get_schema_declarations(schema_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        for old in &existing {
            let replacement = relationships.iter().find(|r| r.name == old.name);
            let removed = replacement.is_none();
            let retargeted = replacement.is_some_and(|r| r.target_type != old.target_type);
            if !(removed || retargeted) {
                continue;
            }
            let live = self
                .store
                .count_instance_edges_for_declaration(schema_id, &old.name)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            if live > 0 {
                let action = if removed { "remove" } else { "retarget" };
                return Err(NodeServiceError::invalid_update(format!(
                    "Cannot {} relationship '{}' on schema '{}': {} instance edge(s) exist \
                     under it. Delete those relationships first.",
                    action, old.name, schema_id, live
                )));
            }
        }

        let changes = crate::db::SqliteStore::set_schema_declarations_in_tx(
            tx.store_tx(),
            schema_id,
            relationships,
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        for (rel_id, out_node, rel) in changes.created {
            let props = serde_json::to_value(&rel).unwrap_or_else(|_| serde_json::json!({}));
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id, schema_id, &out_node, &rel.name, props,
                ),
            });
        }
        for (rel_id, out_node, rel) in changes.updated {
            let props = serde_json::to_value(&rel).unwrap_or_else(|_| serde_json::json!({}));
            self.emit_event(DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id, schema_id, &out_node, &rel.name, props,
                ),
            });
        }
        for (rel_id, out_node, name) in changes.deleted {
            self.emit_event(DomainEvent::RelationshipDeleted {
                id: rel_id,
                from_id: crate::db::events::node_thing(schema_id),
                to_id: crate::db::events::node_thing(&out_node),
                relationship_type: name,
            });
        }

        Ok(())
    }

    /// Get all schema nodes with their relationships
    ///
    /// Returns all schema definitions including fields and relationships.
    /// This is the primary entry point for NLP to understand the data model.
    ///
    /// # Returns
    ///
    /// Vector of all schema nodes, ordered by ID.
    ///
    /// # Example
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
    /// // Get all schemas to understand the data model
    /// let schemas = service.get_all_schemas().await?;
    /// for schema in schemas {
    ///     println!("Type: {} ({} fields, {} relationships)",
    ///         schema.id, schema.fields.len(), schema.relationships.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_all_schemas(
        &self,
    ) -> Result<Vec<crate::models::SchemaNode>, NodeServiceError> {
        self.store.get_all_schemas().await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Failed to get all schemas: {}", e),
            })
        })
    }

    /// Get a schema with full relationship information
    ///
    /// Convenience method that returns a SchemaNode with its relationships.
    /// Use this when you need the complete schema definition including relationships.
    ///
    /// **Returns `schema_id`'s own directly-declared fields and relationships
    /// only — not merged across the ADR-078 `extends` chain.** A schema that
    /// `extends` a parent will not have the parent's fields/relationships
    /// folded in here; reading `.fields`/`.relationships` straight off this
    /// return value silently drops anything inherited. Callers that need the
    /// effective set across the whole chain (as most schema-aware reads
    /// should) want [`Self::resolve_field_owners`] and
    /// [`Self::resolve_relationships`] instead — both already do this
    /// resolution and are the established way this codebase avoids that
    /// exact bug class.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID (e.g., "task", "invoice")
    ///
    /// # Returns
    ///
    /// The SchemaNode if found, None otherwise.
    ///
    /// # Example
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
    /// if let Some(schema) = service.get_schema_with_relationships("invoice").await? {
    ///     for rel in &schema.relationships {
    ///         let target = rel.target_type.as_deref().unwrap_or("*");
    ///         println!("{} -> {} ({:?})", rel.name, target, rel.cardinality);
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_schema_with_relationships(
        &self,
        schema_id: &str,
    ) -> Result<Option<crate::models::SchemaNode>, NodeServiceError> {
        // get_schema_node already includes relationships now
        self.get_schema_node(schema_id).await
    }

    /// Check whether a node satisfies all required relationships in its schema
    pub async fn check_node_completeness(
        &self,
        node_id: &str,
    ) -> Result<CompletenessResult, NodeServiceError> {
        // Look up the node
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Resolved via `resolve_relationships` rather than a direct
        // `get_schema_node(node_type)` lookup: the latter returns only
        // `node_type`'s own directly-declared relationships, not the
        // ADR-078 `extends`-chain-merged set. A relationship declared
        // `required: true` only on an ancestor schema and inherited (not
        // redeclared) by a subtype was therefore invisible here, so a
        // subtype instance genuinely missing that inherited required
        // relationship was silently reported complete. Same fix pattern as
        // `workflow_state.rs`, `validation.rs`, `graph_resolver.rs`'s
        // `is_declared_many_relationship`, and `rel_ops.rs`'s
        // `resolve_relationship_name`/relationship graph helpers. When
        // `node_type` has no schema at all, `resolve_relationships` returns
        // an empty relationship set — same "nothing required → complete by
        // definition" outcome the old direct lookup produced for a missing
        // schema.
        let (relationships, _) = self.resolve_relationships(&node.node_type).await?;

        let mut missing = Vec::new();

        for relationship in &relationships {
            // Only check relationships explicitly marked as required
            if relationship.required != Some(true) {
                continue;
            }

            let query_failed = |e: anyhow::Error| {
                NodeServiceError::query_failed(format!(
                    "Failed to check required relationship '{}': {}",
                    relationship.name, e
                ))
            };

            // An `out` declaration's edges are stored from this node's own end
            // (`in_node = node_id`, under `name`). An `in` declaration is the
            // target's view of a forward edge: stored under its `reverse_name`
            // with this node at `out_node` — the only shape, since writes
            // through the `in` name are normalized to it. Only edges from a
            // source satisfying the declared `target_type` (itself or an
            // ADR-078 descendant) count, since another schema may declare the
            // same forward name toward this type.
            let satisfied = match relationship.direction {
                RelationshipDirection::Out => {
                    self.store
                        .check_relationship_exists(node_id, &relationship.name)
                        .await
                        .map_err(query_failed)?
                        > 0
                }
                RelationshipDirection::In => {
                    let source_types = self
                        .store
                        .get_inbound_relationship_source_types(node_id, &relationship.reverse_name)
                        .await
                        .map_err(query_failed)?;
                    match &relationship.target_type {
                        None => !source_types.is_empty(),
                        Some(expected) => {
                            let mut any = false;
                            for source_type in &source_types {
                                if self.type_satisfies(source_type, expected).await? {
                                    any = true;
                                    break;
                                }
                            }
                            any
                        }
                    }
                }
            };

            if !satisfied {
                missing.push(relationship.name.clone());
            }
        }

        Ok(CompletenessResult {
            node_id: node_id.to_string(),
            is_complete: missing.is_empty(),
            missing_relationships: missing,
        })
    }
}
