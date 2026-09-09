//! Schema-related operations for NodeService.

use super::*;

impl NodeService {
    /// Query nodes by type with optional lifecycle_status filter.
    ///
    /// Used by the playbook engine to load all active playbooks at startup.
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
        // Acceptable for desktop (low playbook counts). If scaling becomes
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

        // Resolve the uniqueness flags from the type's schema fields.
        let schema = self
            .store
            .get_schema_node(node_type)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let flags = schema
            .as_ref()
            .and_then(|s| s.fields.iter().find(|f| f.name == field))
            .map(|f| {
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

        let conflicting_id = self
            .store
            .find_conflicting_unique(node_type, field, value, exclude_id, case_insensitive)
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
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        let content_changed = update
            .content
            .as_ref()
            .is_some_and(|new_content| new_content != &existing.content);

        let title_update = if content_changed || update.has_property_fields() {
            let mut merged = existing;
            if let Some(ref new_content) = update.content {
                merged.content = new_content.clone();
            }
            update.apply_to_properties(&mut merged.properties);
            self.compute_title(&merged, None).await?
        } else {
            None
        };

        self.store
            .update_task_node(id, expected_version, update, title_update)
            .await
            .map_err(|e| {
                // Get the full error chain for pattern matching
                // anyhow errors chain with context, so we need to check the full string
                let error_msg = format!("{:#}", e); // Use alternate format for full chain
                let root_cause = e.root_cause().to_string();

                if error_msg.contains("VersionMismatch")
                    || root_cause.contains("VersionMismatch")
                    || root_cause.contains("failed transaction")
                {
                    // A failed transaction surfaces as a "failed transaction" error.
                    // Our only abort is for version mismatch, so treat failed transactions as OCC errors.
                    // Note: This is a simplification - ideally the abort message would be preserved.
                    NodeServiceError::VersionConflict {
                        node_id: id.to_string(),
                        expected_version,
                        actual_version: 0, // Actual version unknown when transaction fails
                    }
                } else if error_msg.contains("not found")
                    || error_msg.contains("Record not found")
                    || error_msg.contains("$current[0].version")
                    || root_cause.contains("not found")
                    || root_cause.contains("$current")
                {
                    // The store returns various error formats for missing records
                    // "Record not found" - explicit record error
                    // "$current[0].version" - when the LET query returns empty and IF fails
                    NodeServiceError::node_not_found(id)
                } else {
                    NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                        context: format!("Failed to update task node '{}': {}", id, e),
                    })
                }
            })
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

        // Validate destination field does not already exist
        if schema.fields.iter().any(|f| f.name == to) {
            return Err(NodeServiceError::invalid_update(format!(
                "Field '{}' already exists in schema '{}'; cannot rename to an existing field",
                to, type_id
            )));
        }

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
                let schema = schema.clone();
                Box::pin(async move {
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
        let schema = self
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

        self.update_node_unchecked(type_id, update)
            .await
            .map_err(|e| {
                NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                    context: format!(
                        "Failed to update friendly_name for field '{}' on type '{}': {}",
                        field_name, type_id, e
                    ),
                })
            })?;

        Ok(())
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
                if crate::models::schema::is_builtin_relationship(name) {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Relationship {} '{}' is reserved for a built-in structural relationship \
                         ({}); choose a different name",
                        which,
                        name,
                        crate::models::schema::BUILTIN_RELATIONSHIP_NAMES.join(", ")
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
                if crate::models::schema::is_builtin_relationship(name) {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Relationship {} '{}' is reserved for a built-in structural relationship \
                         ({}); choose a different name",
                        which,
                        name,
                        crate::models::schema::BUILTIN_RELATIONSHIP_NAMES.join(", ")
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

        // Look up the schema for the node's type
        let schema_node = self.get_schema_node(&node.node_type).await?;

        let Some(schema) = schema_node else {
            // No schema → nothing required → complete by definition
            return Ok(CompletenessResult {
                node_id: node_id.to_string(),
                is_complete: true,
                missing_relationships: vec![],
            });
        };

        let mut missing = Vec::new();

        for relationship in &schema.relationships {
            // Only check relationships explicitly marked as required
            if relationship.required != Some(true) {
                continue;
            }

            // Check whether at least one edge of this relationship type exists
            let existing_count = self
                .store
                .check_relationship_exists(node_id, &relationship.name)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to check required relationship '{}': {}",
                        relationship.name, e
                    ))
                })?;
            if existing_count == 0 {
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
