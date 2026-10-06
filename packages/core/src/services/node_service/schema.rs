//! Schema-related operations for NodeService.

use super::*;
use crate::models::schema::{RelationshipDirection, EXTENDS_RELATIONSHIP};

impl NodeService {
    /// Every node of `node_type`, and of each type extending it.
    ///
    /// An archived node is left out unless `include_archived`: it participates
    /// in nothing (ADR-087 §2). That is how an archived play doesn't run, an
    /// archived skill isn't routed to and an archived tool isn't offered.
    pub async fn query_nodes_by_type(
        &self,
        node_type: &str,
        include_archived: bool,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let query = crate::models::NodeQuery {
            node_type: Some(node_type.to_string()),
            include_archived,
            ..Default::default()
        };

        self.store
            .query_nodes(query)
            .await
            .map_err(NodeServiceError::from_store)
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
        // `validation.rs`, `path_ops.rs`'s
        // `resolve_hop`, and `rel_ops.rs`'s
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
            .map_err(NodeServiceError::from_store)?;

        match conflicting_id {
            Some(id) => self
                .store
                .get_node(&id)
                .await
                .map_err(NodeServiceError::from_store),
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
            .map_err(NodeServiceError::from_store)
    }

    /// Update a task node's core fields (`status`, `priority`, `due_date`,
    /// `started_at`, `completed_at`) with optimistic concurrency control. See
    /// [`Self::update_person_node`] for why this delegates to the generic
    /// pipeline; `status`/`priority` are validated there against the
    /// schema's declared vocabulary (core + user values).
    pub async fn update_task_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::TaskNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "TaskNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "task", expected_version, update.to_properties_patch())
            .await
    }

    /// Update a person node's core fields (`first_name`, `last_name`, `email`)
    /// with optimistic concurrency control.
    ///
    /// The typed payload is the wire contract; the write itself runs the
    /// generic update pipeline (`update_node`), which already owns schema
    /// validation, `title_template` recompute, the version check, invariant
    /// dispatch and the `NodeUpdated` diff. Returns the stored `Node`; callers
    /// serialize it through `node_to_typed_value` like every other read.
    pub async fn update_person_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::PersonNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "PersonNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "person", expected_version, update.to_properties_patch())
            .await
    }

    /// Update a project node's core fields (`status`, `priority`,
    /// `start_date`, `end_date`, `repository`) with optimistic concurrency control. See
    /// [`Self::update_person_node`] for why this delegates to the generic
    /// pipeline; `status`/`priority` are validated there against the
    /// schema's declared vocabulary (core + user values).
    pub async fn update_project_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::ProjectNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "ProjectNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(
            id,
            "project",
            expected_version,
            update.to_properties_patch(),
        )
        .await
    }

    /// Update a spec's core fields (`objective`, `boundaries`, `spec_status`)
    /// with optimistic concurrency control. See [`Self::update_person_node`]
    /// for why this delegates to the generic pipeline: the status is checked
    /// there against the schema's closed vocabulary, and the seeded rules
    /// that guard approval and lock a superseded spec run on this write as on
    /// any other.
    pub async fn update_spec_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::SpecNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "SpecNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "spec", expected_version, update.to_properties_patch())
            .await
    }

    /// Update a plan's core fields (`approach`, `risks`, `plan_status`) with
    /// optimistic concurrency control. See [`Self::update_spec_node`].
    pub async fn update_plan_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::PlanNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "PlanNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "plan", expected_version, update.to_properties_patch())
            .await
    }

    /// Update a decision's core field (`decision_status`) with optimistic
    /// concurrency control. See [`Self::update_spec_node`].
    pub async fn update_decision_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::DecisionNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "DecisionNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(
            id,
            "decision",
            expected_version,
            update.to_properties_patch(),
        )
        .await
    }

    /// Update a saved query's fields (definition, `generated_by`,
    /// `generator_context`, `view_config`) with optimistic concurrency
    /// control. See [`Self::update_person_node`] for why this delegates to the
    /// generic pipeline; the resulting field shapes are checked there by
    /// `QueryNodeBehavior::validate`.
    pub async fn update_query_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::QueryNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "QueryNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "query", expected_version, update.to_properties_patch())
            .await
    }

    /// Update a play's fields (`rules`, `description`, `enabled`) with
    /// optimistic concurrency control. See [`Self::update_person_node`] for
    /// why this delegates to the generic pipeline: the rules are decoded there
    /// by `PlayNodeBehavior::validate` and, when they change, checked against
    /// the schemas by the play gate, exactly as for any other write.
    pub async fn update_play_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::PlayNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "PlayNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "play", expected_version, update.to_properties_patch())
            .await
    }

    /// Update a collection's core field (`description`) with optimistic
    /// concurrency control. See [`Self::update_person_node`] for why this
    /// delegates to the generic pipeline. The collection's name is its
    /// `content`, written through the rename operation.
    pub async fn update_collection_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::CollectionNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "CollectionNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(
            id,
            "collection",
            expected_version,
            update.to_properties_patch(),
        )
        .await
    }

    /// Update a skill's core fields (`use_for`, `not_for`,
    /// `tool_whitelist`, `max_iterations`) with optimistic
    /// concurrency control. See [`Self::update_person_node`] for why this
    /// delegates to the generic pipeline; the resulting field shapes are
    /// checked there by `SkillNodeBehavior::validate`.
    pub async fn update_skill_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::SkillNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "SkillNodeUpdate contains no changes",
            ));
        }
        self.update_typed_fields(id, "skill", expected_version, update.to_properties_patch())
            .await
    }

    /// The settings this database holds (ADR-095): the singleton's fields, with
    /// the schema defaults for any it does not store.
    ///
    /// Every consumer reads settings here, in the database it is routed to.
    /// Returns the node's version beside the fields so a read-modify-write can
    /// pass it to [`Self::update_database_settings_node`].
    pub async fn database_settings(
        &self,
    ) -> Result<(crate::models::DatabaseSettingsFields, i64), NodeServiceError> {
        let node = self
            .get_node(DATABASE_SETTINGS_NODE_ID)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(DATABASE_SETTINGS_NODE_ID))?;
        let fields = crate::models::DatabaseSettingsFields::from_properties(&node.properties)
            .map_err(|e| NodeServiceError::invalid_update(e.to_string()))?;
        Ok((fields, node.version))
    }

    /// Update the database-settings node's fields with optimistic concurrency
    /// control. See [`Self::update_person_node`] for why this delegates to the
    /// generic pipeline.
    ///
    /// A provider's routing verdicts describe the endpoint and model they were
    /// measured against, so a write that changes either one's `base_url` or
    /// `model` drops that provider's verdicts, whatever the client sent.
    pub async fn update_database_settings_node(
        &self,
        id: &str,
        expected_version: i64,
        mut update: crate::models::DatabaseSettingsNodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "DatabaseSettingsNodeUpdate contains no changes",
            ));
        }
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;
        // A subtype another build retyped the singleton to inherits these
        // fields in the base bucket (ADR-083 §2), so it is written the same.
        if !self
            .type_is_a(
                &existing.node_type,
                crate::models::CoreNodeType::DatabaseSettings,
            )
            .await?
        {
            return Err(NodeServiceError::invalid_update(format!(
                "Node '{}' is {} node, not a database-settings node",
                id,
                crate::utils::with_indefinite_article(&existing.node_type)
            )));
        }
        if let Some(Some(providers)) = update.providers.as_mut() {
            if let Ok(before) =
                crate::models::DatabaseSettingsFields::from_properties(&existing.properties)
            {
                for provider in providers.iter_mut() {
                    let unchanged = before.provider(&provider.id).is_some_and(|old| {
                        old.base_url == provider.base_url && old.model == provider.model
                    });
                    if !unchanged {
                        provider.routing_ok.clear();
                    }
                }
            }
        }
        let patch = NodeUpdate {
            properties: Some(update.to_properties_patch()),
            ..Default::default()
        };
        self.update_node(id, expected_version, patch).await
    }

    /// Record the engine's suspension of a play on the play node
    /// (ADR-087 §5): why it was taken out of service on this device, the
    /// diagnostic, and when.
    ///
    /// The three fields are system-owned and `local_only`. The write bumps no
    /// version, so it cannot make a user's edit of the same play conflict, and
    /// it emits a node-updated event so watchers see the suspension. The
    /// user's `enabled` switch is not touched.
    pub async fn record_play_suspension(
        &self,
        play_id: &str,
        reason: crate::models::PlaySuspensionReason,
        message: &str,
    ) -> Result<Node, NodeServiceError> {
        let before = self
            .get_node(play_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(play_id))?;
        if !self
            .type_is_a(&before.node_type, crate::models::CoreNodeType::Play)
            .await?
        {
            return Err(NodeServiceError::invalid_update(format!(
                "Node '{}' is {} node, not a play",
                play_id,
                crate::utils::with_indefinite_article(&before.node_type)
            )));
        }

        let path = |field: &str| format!("$.{}.{field}", crate::models::PLAY_NODE_TYPE);
        self.store
            .set_property_strings(
                play_id,
                &[
                    (
                        path(nodespace_types::PLAY_SUSPENDED_REASON_FIELD),
                        reason.as_str().to_string(),
                    ),
                    (
                        path(nodespace_types::PLAY_SUSPENDED_MESSAGE_FIELD),
                        message.to_string(),
                    ),
                    (
                        path(nodespace_types::PLAY_SUSPENDED_AT_FIELD),
                        chrono::Utc::now().to_rfc3339(),
                    ),
                ],
            )
            .await
            .map_err(NodeServiceError::from_store)?;

        let node = self
            .get_node(play_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(play_id))?;
        self.emit_event(DomainEvent::NodeUpdated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            node: node.clone(),
            changed_properties: super::compute_property_changes(
                &before.properties,
                &node.properties,
            ),
        });
        Ok(node)
    }

    /// Write a typed update's flat properties patch to a node that must be of
    /// `node_type`.
    ///
    /// The type check runs before the transaction. That is sound because the
    /// version check inside the write rejects any change that lands after this
    /// read: if the node is `node_type` at `expected_version`, a later retype
    /// bumps the version and the write conflicts; if the version already moved,
    /// the write conflicts regardless.
    async fn update_typed_fields(
        &self,
        id: &str,
        node_type: &str,
        expected_version: i64,
        patch: serde_json::Value,
    ) -> Result<Node, NodeServiceError> {
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;
        if existing.node_type != node_type {
            return Err(NodeServiceError::invalid_update(format!(
                "Node '{}' is {} node, not {} node",
                id,
                crate::utils::with_indefinite_article(&existing.node_type),
                crate::utils::with_indefinite_article(node_type)
            )));
        }
        let update = NodeUpdate {
            properties: Some(patch),
            ..Default::default()
        };
        self.update_node(id, expected_version, update).await
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
        // Delegates to the store-only free function (see its doc for the
        // query-cost rationale) so callers that hold a `SqliteStore` but not
        // a `NodeService` (`NodeEmbeddingService`) can get the same
        // resolution without duplicating it.
        crate::services::resolve_type_chain_from_store(&self.store, node_type).await
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
    /// The `extends` edge is never among them. It is a statement about the
    /// schema graph, not a data relationship a node instance carries, and a
    /// schema holds it as [`SchemaNode::extends`](crate::models::SchemaNode),
    /// not as an entry in `relationships`. So a condition segment literally
    /// named `extends`/`extended_by` is not a traversable relationship here,
    /// and is classified `Unresolvable` rather than `NotYetMet`.
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
            let relationships = schema.relationships;
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

    /// The context paths of `node_type` (ADR-094 §2): what a context read of
    /// a node of that type follows when it is given none. Each path comes
    /// with the schema that declares it.
    ///
    /// A type's context paths are its ancestors' and then its own, the
    /// farthest ancestor's first. A path two schemas in the chain both
    /// declare is listed once, under the farther one.
    pub async fn resolve_context_paths(
        &self,
        node_type: &str,
    ) -> Result<Vec<(nodespace_types::RelationshipPath, String)>, NodeServiceError> {
        let chain = self.resolve_type_chain(node_type).await?;
        let mut paths: Vec<(nodespace_types::RelationshipPath, String)> = Vec::new();
        for schema_id in chain.iter().rev() {
            // A missing mid-chain schema contributes nothing, as in
            // `resolve_field_owners`.
            let Some(schema) = self.get_schema_node(schema_id).await? else {
                continue;
            };
            for path in schema.context_paths {
                if !paths.iter().any(|(known, _)| *known == path) {
                    paths.push((path, schema_id.clone()));
                }
            }
        }
        Ok(paths)
    }

    /// Refuse to bring `fields`, declared by `owner`, into force on the
    /// instances of `scan_root` (and of every subtype extending it) while one
    /// of them holds a value under one of those names that the declaration
    /// rejects. `owner` is `scan_root` itself or one of its ancestors.
    ///
    /// Declaring a name doesn't touch the values already stored under it:
    /// `remove_fields` drops only the declaration, a rename moves only the
    /// rows holding its source name, and an `extends` re-target brings the
    /// new parent's buckets into scope as they are — including a bucket left
    /// from an earlier parent or a retype. Re-adding a name with a different
    /// type, as an enum whose values exclude a stored one, renaming onto a
    /// name with leftover values, or re-parenting onto such a bucket would
    /// otherwise leave every such node failing
    /// [`Self::validate_node_with_fields`] on any later write, including a
    /// content-only one. So would bringing a `required` field with no default
    /// into force on nodes that don't hold it. Values are resolved the way
    /// that validator resolves them and judged by the same
    /// [`Self::check_field_value`] and presence rule, so the two can't
    /// disagree.
    ///
    /// Refusing is preferred over clearing the values: that would be a silent
    /// destructive write across every instance.
    pub(crate) async fn reject_incompatible_instance_values(
        tx: &NodeServiceTx<'_>,
        scan_root: &str,
        owner: &str,
        fields: &[crate::models::schema::SchemaField],
    ) -> Result<(), NodeServiceError> {
        if fields.is_empty() {
            return Ok(());
        }
        let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
        let stored = crate::db::SqliteStore::get_effective_field_values_in_tx(
            tx.store_tx(),
            scan_root,
            owner,
            &names,
        )
        .await
        .map_err(NodeServiceError::from_store)?;

        for field in fields {
            let at_field = || stored.iter().filter(|v| v.field == field.name);

            // A required field with no default rejects every node lacking it.
            if field.required.unwrap_or(false) && field.default.is_none() {
                let missing: Vec<_> = at_field().filter(|v| v.value.is_none()).collect();
                if let Some(first) = missing.first() {
                    let set = if owner == first.node_type {
                        format!("{{\"{owner}\": {{\"{}\": <value>}}}}", field.name)
                    } else {
                        format!(
                            "{{\"{}\": {{}}, \"{owner}\": {{\"{}\": <value>}}}}",
                            first.node_type, field.name
                        )
                    };
                    return Err(NodeServiceError::invalid_update(format!(
                        "Cannot apply required field '{}' as type '{}' (declared by schema '{}') \
                         to '{}' instances: {} existing {} no value for it and it has no \
                         default (e.g. node '{}'). Give the field a default, declare it not \
                         required, or set it on each such node by updating it with properties \
                         {}, then retry.",
                        field.name,
                        field.field_type,
                        owner,
                        scan_root,
                        missing.len(),
                        if missing.len() == 1 {
                            "node has"
                        } else {
                            "nodes have"
                        },
                        first.node_id,
                        set
                    )));
                }
            }

            let rejected: Vec<_> = at_field()
                .filter_map(|v| {
                    let reason = Self::check_field_value(field, v.value.as_ref()?).err()?;
                    Some((v, reason))
                })
                .collect();
            if let Some((first, reason)) = rejected.first() {
                // The value may sit in a bucket normal reads don't show (left
                // from an earlier parent or a retype), so name the exact
                // payload that clears it: an update deep-merges per bucket,
                // and carrying the node's own type key keeps the input in
                // storage shape rather than read as flat fields.
                let bucket = first.bucket.as_deref().unwrap_or(owner);
                let clear = if bucket == first.node_type {
                    format!("{{\"{bucket}\": {{\"{}\": null}}}}", field.name)
                } else {
                    format!(
                        "{{\"{}\": {{}}, \"{bucket}\": {{\"{}\": null}}}}",
                        first.node_type, field.name
                    )
                };
                return Err(NodeServiceError::invalid_update(format!(
                    "Cannot apply field '{}' as type '{}' (declared by schema '{}') to '{}' \
                     instances: {} existing {} a value under that name the declaration rejects \
                     (e.g. node '{}': {}). Convert each value to fit, or clear it by updating \
                     the node with properties {}, then retry.",
                    field.name,
                    field.field_type,
                    owner,
                    scan_root,
                    rejected.len(),
                    if rejected.len() == 1 {
                        "node holds"
                    } else {
                        "nodes hold"
                    },
                    first.node_id,
                    reason,
                    clear
                )));
            }
        }

        Ok(())
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

                    // The migration moved only rows holding `from`. A row
                    // with no `from` value can still hold a leftover `to` —
                    // from a field of that name removed earlier — which the
                    // renamed declaration must accept. Checked after the
                    // migration, on this transaction's view, so it judges
                    // exactly the values the rename leaves behind. Presence
                    // is not re-judged: a rename moves a value, never removes
                    // one, so a node lacking a required field lacked it
                    // before too, and refusing over it would block the rename
                    // without protecting anything.
                    let renamed: Vec<_> = updated_fields
                        .iter()
                        .filter(|f| f.name == to)
                        .map(|f| crate::models::schema::SchemaField {
                            required: None,
                            ..f.clone()
                        })
                        .collect();
                    Self::reject_incompatible_instance_values(tx, &type_id, &type_id, &renamed)
                        .await?;

                    // Declarations live in the relationship table, not in
                    // properties, and are untouched by a field rename.
                    let renamed_schema = crate::models::SchemaNode {
                        fields: updated_fields,
                        ..schema
                    };
                    let update = crate::models::NodeUpdate {
                        properties: Some(crate::models::schema_node::to_properties(
                            &renamed_schema,
                        )),
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

                    // Same persistence shape as `rename_schema_field`'s Step 2:
                    // declarations live in the relationship table and are untouched.
                    let relabelled_schema = crate::models::SchemaNode {
                        fields: updated_fields,
                        ..schema
                    };
                    let update = crate::models::NodeUpdate {
                        properties: Some(crate::models::schema_node::to_properties(
                            &relabelled_schema,
                        )),
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

    /// Replace a schema's declaration edges — the write path for them.
    /// `relationships` is the full set, the `extends` edge included
    /// (`schema_node::to_declarations`): a declaration left out is removed.
    /// `create_schema`/`update_schema` route through here;
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
            .map_err(NodeServiceError::from_store)?;
        Self::reject_cleared_parent(schema_id, &existing, relationships)?;
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
                .map_err(NodeServiceError::from_store)?;
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
            .map_err(NodeServiceError::from_store)?;

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

    /// Refuse a declaration set that drops a schema's `extends` edge.
    ///
    /// A parent can be re-targeted, never cleared (ADR-078), and the write is
    /// a full replace: a list built from [`SchemaNode::relationships`], which
    /// does not hold the parent, would otherwise delete the edge and silently
    /// unlink the type from its base. No instance carries an `extends` edge,
    /// so the live-edge check below would not stop it.
    ///
    /// [`SchemaNode::relationships`]: crate::models::SchemaNode
    fn reject_cleared_parent(
        schema_id: &str,
        existing: &[crate::models::schema::SchemaRelationship],
        relationships: &[crate::models::schema::SchemaRelationship],
    ) -> Result<(), NodeServiceError> {
        let is_extends =
            |rel: &crate::models::schema::SchemaRelationship| rel.name == EXTENDS_RELATIONSHIP;
        match existing.iter().find(|rel| is_extends(rel)) {
            Some(parent) if !relationships.iter().any(is_extends) => {
                Err(NodeServiceError::invalid_update(format!(
                    "Schema '{}' extends '{}', and this write leaves its `extends` edge out. A \
                     parent can be re-targeted but not cleared: write the full declaration set, \
                     the `extends` edge included.",
                    schema_id,
                    parent.target_type.as_deref().unwrap_or("?")
                )))
            }
            _ => Ok(()),
        }
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
            .map_err(NodeServiceError::from_store)?;
        Self::reject_cleared_parent(schema_id, &existing, relationships)?;
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
                .map_err(NodeServiceError::from_store)?;
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
        .map_err(NodeServiceError::from_store)?;

        for (rel_id, out_node, rel) in changes.created {
            let props = serde_json::to_value(&rel).unwrap_or_else(|_| serde_json::json!({}));
            self.emit_event_in_tx(
                tx,
                DomainEvent::RelationshipCreated {
                    relationship: crate::db::events::RelationshipEvent::new(
                        rel_id, schema_id, &out_node, &rel.name, props,
                    ),
                },
            );
        }
        for (rel_id, out_node, rel) in changes.updated {
            let props = serde_json::to_value(&rel).unwrap_or_else(|_| serde_json::json!({}));
            self.emit_event_in_tx(
                tx,
                DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent::new(
                        rel_id, schema_id, &out_node, &rel.name, props,
                    ),
                },
            );
        }
        for (rel_id, out_node, name) in changes.deleted {
            self.emit_event_in_tx(
                tx,
                DomainEvent::RelationshipDeleted {
                    id: rel_id,
                    from_id: crate::db::events::node_thing(schema_id),
                    to_id: crate::db::events::node_thing(&out_node),
                    relationship_type: name,
                },
            );
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
    ///         schema.envelope.id, schema.fields.len(), schema.relationships.len());
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
        // `workflow_state.rs`, `validation.rs`, `path_ops.rs`'s
        // `resolve_hop`, and `rel_ops.rs`'s
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

#[cfg(test)]
mod typed_update_tests {
    use super::*;
    use crate::db::SqliteStore;
    use crate::models::{
        PersonNodeUpdate, PlayNodeUpdate, Priority, ProjectNodeUpdate, ProjectStatus,
        QueryNodeUpdate, TaskNodeUpdate, TaskStatus,
    };
    use crate::services::{CreateNodeParams, InsertPositionOwned};
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_service() -> (NodeService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let mut store = Arc::new(
            SqliteStore::new(temp_dir.path().join("test.db"))
                .await
                .unwrap(),
        );
        let service = NodeService::new(&mut store).await.unwrap();
        (service, temp_dir)
    }

    async fn create(service: &NodeService, node_type: &str, properties: serde_json::Value) -> Node {
        let id = service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: node_type.to_string(),
                // `person` takes its name from its title template and rejects
                // content; `project` requires it.
                content: if node_type == "person" {
                    String::new()
                } else {
                    "Name".to_string()
                },
                parent_id: None,
                position: InsertPositionOwned::End,
                properties,
                lifecycle_status: None,
            })
            .await
            .unwrap();
        service.get_node(&id).await.unwrap().unwrap()
    }

    fn set(value: &str) -> Option<Option<String>> {
        Some(Some(value.to_string()))
    }

    #[tokio::test]
    async fn person_update_writes_fields_and_recomputes_title() {
        let (service, _t) = create_test_service().await;
        let person = create(&service, "person", json!({ "first_name": "Ada" })).await;

        let updated = service
            .update_person_node(
                &person.id,
                person.version,
                PersonNodeUpdate {
                    last_name: set("Lovelace"),
                    email: set("ada@example.com"),
                    ..Default::default()
                },
            )
            .await
            .expect("typed person update succeeds");

        assert_eq!(updated.version, person.version + 1);
        assert_eq!(updated.title.as_deref(), Some("Ada Lovelace"));
        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["firstName"], "Ada", "an untouched field survives");
        assert_eq!(typed["lastName"], "Lovelace");
        assert_eq!(typed["email"], "ada@example.com");
        assert_eq!(typed["properties"], json!({}));
    }

    #[tokio::test]
    async fn person_update_null_clears_a_field() {
        let (service, _t) = create_test_service().await;
        let person = create(
            &service,
            "person",
            json!({ "first_name": "Ada", "last_name": "Lovelace" }),
        )
        .await;

        let updated = service
            .update_person_node(
                &person.id,
                person.version,
                PersonNodeUpdate {
                    last_name: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.title.as_deref(), Some("Ada"));
        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert!(typed.get("lastName").is_none());
    }

    #[tokio::test]
    async fn person_update_on_a_stale_version_conflicts() {
        let (service, _t) = create_test_service().await;
        let person = create(&service, "person", json!({ "first_name": "Ada" })).await;

        let err = service
            .update_person_node(
                &person.id,
                person.version + 5,
                PersonNodeUpdate {
                    first_name: set("Grace"),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(
            matches!(err, NodeServiceError::VersionConflict { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn typed_update_rejects_a_node_of_another_type() {
        let (service, _t) = create_test_service().await;
        let task = create(&service, "task", json!({})).await;

        let err = service
            .update_person_node(
                &task.id,
                task.version,
                PersonNodeUpdate {
                    first_name: set("Ada"),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("not a person node"), "{err}");
    }

    #[tokio::test]
    async fn empty_typed_update_is_rejected() {
        let (service, _t) = create_test_service().await;
        let person = create(&service, "person", json!({})).await;

        assert!(service
            .update_person_node(&person.id, person.version, PersonNodeUpdate::default())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn task_update_writes_fields_and_leaves_others() {
        let (service, _t) = create_test_service().await;
        let task = create(
            &service,
            "task",
            json!({ "priority": "low", "custom:estimate": 3 }),
        )
        .await;

        let updated = service
            .update_task_node(
                &task.id,
                task.version,
                TaskNodeUpdate {
                    status: Some(TaskStatus::InProgress),
                    due_date: set("2026-03-01"),
                    ..Default::default()
                },
            )
            .await
            .expect("typed task update succeeds");

        assert_eq!(updated.version, task.version + 1);
        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["status"], "in_progress");
        assert_eq!(typed["dueDate"], "2026-03-01");
        assert_eq!(
            typed["priority"], "low",
            "an absent priority is left unchanged"
        );
        assert_eq!(typed["properties"], json!({ "custom:estimate": 3 }));
    }

    #[tokio::test]
    async fn task_update_null_clears_priority_and_dates() {
        let (service, _t) = create_test_service().await;
        let task = create(
            &service,
            "task",
            json!({ "priority": "high", "due_date": "2026-03-01" }),
        )
        .await;

        let updated = service
            .update_task_node(
                &task.id,
                task.version,
                TaskNodeUpdate {
                    priority: Some(None),
                    due_date: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert!(typed.get("priority").is_none(), "{typed}");
        assert!(typed.get("dueDate").is_none(), "{typed}");
        assert_eq!(typed["status"], "open", "status is untouched");
    }

    /// The typed update returns the node as stored: an archived task stays
    /// archived in the response, and its extension field is still there.
    #[tokio::test]
    async fn task_update_response_keeps_lifecycle_status_and_extension_fields() {
        let (service, _t) = create_test_service().await;
        let id = service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "Retired task".to_string(),
                parent_id: None,
                position: InsertPositionOwned::End,
                properties: json!({ "custom:estimate": 3 }),
                lifecycle_status: Some("archived".to_string()),
            })
            .await
            .unwrap();
        let task = service.get_node(&id).await.unwrap().unwrap();

        let updated = service
            .update_task_node(
                &id,
                task.version,
                TaskNodeUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.lifecycle_status, "archived");
        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["lifecycleStatus"], "archived");
        assert_eq!(typed["status"], "done");
        assert_eq!(typed["properties"], json!({ "custom:estimate": 3 }));
    }

    #[tokio::test]
    async fn task_update_rejects_a_priority_outside_the_schema_vocabulary() {
        let (service, _t) = create_test_service().await;
        let task = create(&service, "task", json!({})).await;

        let err = service
            .update_task_node(
                &task.id,
                task.version,
                TaskNodeUpdate {
                    priority: Some(Some(Priority::User("someday".to_string()))),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Invalid value 'someday'"), "{err}");
    }

    #[tokio::test]
    async fn task_update_on_a_stale_version_conflicts() {
        let (service, _t) = create_test_service().await;
        let task = create(&service, "task", json!({})).await;

        let err = service
            .update_task_node(
                &task.id,
                task.version + 5,
                TaskNodeUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(
            matches!(err, NodeServiceError::VersionConflict { .. }),
            "{err:?}"
        );
    }

    /// The typed task shape is `task`'s own: another type's node, and an empty
    /// update, are both refused.
    #[tokio::test]
    async fn task_update_rejects_another_type_and_an_empty_update() {
        let (service, _t) = create_test_service().await;
        let project = create(&service, "project", json!({})).await;
        let err = service
            .update_task_node(
                &project.id,
                project.version,
                TaskNodeUpdate {
                    status: Some(TaskStatus::Done),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a task node"), "{err}");

        let task = create(&service, "task", json!({})).await;
        assert!(service
            .update_task_node(&task.id, task.version, TaskNodeUpdate::default())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn project_update_writes_fields_and_leaves_others() {
        let (service, _t) = create_test_service().await;
        let project = create(
            &service,
            "project",
            json!({ "status": "planning", "custom:budget": 1200 }),
        )
        .await;

        let updated = service
            .update_project_node(
                &project.id,
                project.version,
                ProjectNodeUpdate {
                    status: Some(ProjectStatus::Active),
                    start_date: set("2026-03-01"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["status"], "active");
        assert_eq!(typed["startDate"], "2026-03-01");
        assert_eq!(typed["properties"], json!({ "custom:budget": 1200 }));
    }

    #[tokio::test]
    async fn project_update_rejects_status_outside_the_schema_vocabulary() {
        let (service, _t) = create_test_service().await;
        let project = create(&service, "project", json!({})).await;

        let err = service
            .update_project_node(
                &project.id,
                project.version,
                ProjectNodeUpdate {
                    status: Some(ProjectStatus::from_value("someday")),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Invalid value 'someday'"), "{err}");
    }

    /// A status a user added to the schema is written and read back as
    /// itself: the typed path accepts it, and the wire conversion carries it
    /// through the enum's user variant.
    #[tokio::test]
    async fn project_update_accepts_a_status_added_to_the_schema() {
        let (service, _t) = create_test_service().await;
        let service = std::sync::Arc::new(service);
        crate::schema::handle_update_schema(
            &service,
            json!({
                "schema_id": "project",
                "add_field_values": [{
                    "field": "status",
                    "values": [{"value": "on_hold", "label": "On hold"}]
                }]
            }),
        )
        .await
        .expect("add_field_values should succeed");
        let project = create(&service, "project", json!({})).await;

        let updated = service
            .update_project_node(
                &project.id,
                project.version,
                ProjectNodeUpdate {
                    status: Some(ProjectStatus::User("on_hold".to_string())),
                    ..Default::default()
                },
            )
            .await
            .expect("a status added via add_field_values must be accepted");

        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["status"], "on_hold");
    }

    /// `archived` is governance vocabulary, not a project status (ADR-087):
    /// the typed update, the generic update and creation all reject it.
    #[tokio::test]
    async fn project_status_archived_is_rejected() {
        let (service, _t) = create_test_service().await;
        let project = create(&service, "project", json!({})).await;

        let err = service
            .update_project_node(
                &project.id,
                project.version,
                ProjectNodeUpdate {
                    status: Some(ProjectStatus::from_value("archived")),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Invalid value 'archived'"),
            "{err}"
        );

        let err = service
            .update_node(
                &project.id,
                project.version,
                NodeUpdate {
                    properties: Some(json!({ "status": "archived" })),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Invalid value 'archived'"),
            "{err}"
        );

        let err = service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "project".to_string(),
                content: "Retired".to_string(),
                parent_id: None,
                position: InsertPositionOwned::End,
                properties: json!({ "status": "archived" }),
                lifecycle_status: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Invalid value 'archived'"),
            "{err}"
        );
    }

    fn saved_query_properties() -> serde_json::Value {
        json!({
            "target_type": "task",
            "filters": [
                { "type": "property", "operator": "equals", "property": "status", "value": "open" }
            ],
            "sorting": [{ "field": "due_date", "direction": "asc" }],
            "generated_by": "user",
            "view_config": { "lastView": "table" },
            "custom:pinned": true
        })
    }

    #[tokio::test]
    async fn query_update_writes_fields_and_leaves_others() {
        let (service, _t) = create_test_service().await;
        let query = create(&service, "query", saved_query_properties()).await;

        let update: QueryNodeUpdate = serde_json::from_value(json!({
            "viewConfig": { "lastView": "kanban", "kanban": { "groupBy": "status" } },
            "limit": 25
        }))
        .unwrap();
        let updated = service
            .update_query_node(&query.id, query.version, update)
            .await
            .expect("typed query update succeeds");

        assert_eq!(updated.version, query.version + 1);
        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["viewConfig"]["kanban"]["groupBy"], "status");
        assert_eq!(typed["limit"], 25);
        assert_eq!(typed["targetType"], "task", "an untouched field survives");
        assert_eq!(typed["filters"][0]["property"], "status");
        assert_eq!(typed["sorting"][0]["field"], "due_date");
        assert_eq!(typed["properties"], json!({ "custom:pinned": true }));
    }

    #[tokio::test]
    async fn query_update_null_clears_a_field() {
        let (service, _t) = create_test_service().await;
        let query = create(&service, "query", saved_query_properties()).await;

        let update: QueryNodeUpdate =
            serde_json::from_value(json!({ "sorting": null, "viewConfig": null })).unwrap();
        let updated = service
            .update_query_node(&query.id, query.version, update)
            .await
            .unwrap();

        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert!(typed.get("sorting").is_none(), "{typed}");
        assert!(typed.get("viewConfig").is_none(), "{typed}");
        assert_eq!(typed["targetType"], "task");
    }

    #[tokio::test]
    async fn query_update_on_a_stale_version_conflicts() {
        let (service, _t) = create_test_service().await;
        let query = create(&service, "query", saved_query_properties()).await;

        let err = service
            .update_query_node(
                &query.id,
                query.version + 5,
                QueryNodeUpdate {
                    limit: Some(Some(10)),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(
            matches!(err, NodeServiceError::VersionConflict { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn query_update_rejects_a_node_of_another_type() {
        let (service, _t) = create_test_service().await;
        let task = create(&service, "task", json!({})).await;

        let err = service
            .update_query_node(
                &task.id,
                task.version,
                QueryNodeUpdate {
                    limit: Some(Some(10)),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("not a query node"), "{err}");
    }

    #[tokio::test]
    async fn empty_query_update_is_rejected() {
        let (service, _t) = create_test_service().await;
        let query = create(&service, "query", saved_query_properties()).await;

        assert!(service
            .update_query_node(&query.id, query.version, QueryNodeUpdate::default())
            .await
            .is_err());
    }

    /// A saved query's relationship paths are resolved against the schemas
    /// when it is saved: a name the target type does not declare is refused
    /// then, rather than stored as a query that quietly matches nothing.
    #[tokio::test]
    async fn a_query_with_an_undeclared_path_is_rejected_on_write() {
        let (service, _t) = create_test_service().await;

        let relationship_filter = |path: serde_json::Value| {
            json!({
                "target_type": "task",
                "filters": [{
                    "type": "relationship", "operator": "equals",
                    "path": path, "node_id": "some-node"
                }]
            })
        };

        // Declared and built-in names save, by forward or reverse name.
        for path in [
            json!(["project"]),
            json!(["child_of"]),
            json!(["blocked_by"]),
        ] {
            create(&service, "query", relationship_filter(path)).await;
        }

        let err = service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "query".to_string(),
                content: "Bad path".to_string(),
                parent_id: None,
                position: InsertPositionOwned::End,
                properties: relationship_filter(json!(["no_such_relationship"])),
                lifecycle_status: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no_such_relationship"), "{err}");

        // The same gate on update.
        let query = create(&service, "query", saved_query_properties()).await;
        let update: QueryNodeUpdate = serde_json::from_value(json!({
            "filters": [{
                "type": "related", "operator": "equals",
                "path": ["no_such_relationship"],
                "filter": { "type": "property", "operator": "equals", "property": "status", "value": "open" }
            }]
        }))
        .unwrap();
        let err = service
            .update_query_node(&query.id, query.version, update)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no_such_relationship"), "{err}");
    }

    fn play_properties() -> serde_json::Value {
        json!({
            "description": "Close a task's parent",
            "rules": [{
                "name": "close-parent",
                "description": "Test rule",
                "trigger": {
                    "type": "graph_event",
                    "on": "property_changed",
                    "select": { "target_type": "task" },
                    "property_key": "task.status"
                },
                "conditions": [{ "expr": "node.status == 'done'", "description": "Test condition" }],
                "actions": [{
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.child_of.id}", "properties": { "status": "done" } }
                }]
            }],
            "custom:owner": "ada"
        })
    }

    #[tokio::test]
    async fn play_update_writes_fields_and_leaves_others() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        let update: PlayNodeUpdate =
            serde_json::from_value(json!({ "description": "Roll completion up" })).unwrap();
        let updated = service
            .update_play_node(&play.id, play.version, update)
            .await
            .expect("typed play update succeeds");

        assert_eq!(updated.version, play.version + 1);
        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert_eq!(typed["description"], "Roll completion up");
        assert_eq!(
            typed["rules"][0]["name"], "close-parent",
            "an untouched field survives"
        );
        assert_eq!(
            typed["rules"][0]["trigger"]["select"]["target_type"],
            "task"
        );
        assert_eq!(typed["lifecycleStatus"], "active");
        assert_eq!(typed["properties"], json!({ "custom:owner": "ada" }));
    }

    #[tokio::test]
    async fn play_update_replaces_rules_whole_and_null_clears_the_description() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        let update: PlayNodeUpdate = serde_json::from_value(json!({
            "description": null,
            "rules": [{
                "name": "greet",
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } },
                "actions": [{
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "content": "hello" }
                }]
            }]
        }))
        .unwrap();
        let updated = service
            .update_play_node(&play.id, play.version, update)
            .await
            .unwrap();

        let typed = crate::models::node_to_typed_value(updated).unwrap();
        assert!(typed.get("description").is_none(), "{typed}");
        let rules = typed["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1, "rules are replaced, not merged: {typed}");
        assert_eq!(rules[0]["name"], "greet");
    }

    #[tokio::test]
    async fn play_update_on_a_stale_version_conflicts() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        let err = service
            .update_play_node(
                &play.id,
                play.version + 5,
                PlayNodeUpdate {
                    description: Some(Some("stale".to_string())),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(
            matches!(err, NodeServiceError::VersionConflict { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn play_update_rejects_a_node_of_another_type() {
        let (service, _t) = create_test_service().await;
        let task = create(&service, "task", json!({})).await;

        let err = service
            .update_play_node(
                &task.id,
                task.version,
                PlayNodeUpdate {
                    description: Some(Some("x".to_string())),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("not a play node"), "{err}");
    }

    #[tokio::test]
    async fn empty_play_update_is_rejected() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        assert!(service
            .update_play_node(&play.id, play.version, PlayNodeUpdate::default())
            .await
            .is_err());
    }

    /// The typed update lowers into the shared pipeline, so new rules pass
    /// the same schema-aware gate a created play does: a rule that decodes
    /// but names a type that does not exist is refused.
    #[tokio::test]
    async fn play_update_validates_new_rules_against_the_schemas() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        let update: PlayNodeUpdate = serde_json::from_value(json!({
            "rules": [{
                "name": "ghost",
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "no_such_type" } }
            }]
        }))
        .unwrap();
        let err = service
            .update_play_node(&play.id, play.version, update)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no_such_type"), "{err}");
    }

    /// The rules of [`play_properties`], with one change made to the rule.
    fn changed_rules(change: impl FnOnce(&mut serde_json::Value)) -> serde_json::Value {
        let mut rules = play_properties()["rules"].clone();
        change(&mut rules[0]);
        rules
    }

    #[tokio::test]
    async fn a_blank_description_is_rejected_on_create_and_on_update() {
        let (service, _t) = create_test_service().await;
        let blank = changed_rules(|rule| rule["actions"][0]["description"] = json!("  "));

        let err = service
            .create_node(Node::new(
                "play".to_string(),
                "Blank".to_string(),
                json!({ "rules": blank }),
            ))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("rule `close-parent`, action 1: its description is blank"),
            "{err}"
        );

        let play = create(&service, "play", play_properties()).await;
        let update: PlayNodeUpdate = serde_json::from_value(json!({ "rules": blank })).unwrap();
        let err = service
            .update_play_node(&play.id, play.version, update)
            .await
            .unwrap_err();
        assert!(
            matches!(err, NodeServiceError::PlayValidationFailed { .. }),
            "{err:?}"
        );
    }

    /// A component whose content changed must not keep the description it was
    /// stored with (ADR-090 §1), through the typed update and the flat one.
    #[tokio::test]
    async fn a_stale_description_is_rejected_until_it_is_rewritten() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        let stale = changed_rules(|rule| {
            rule["conditions"][0]["expr"] = json!("node.status == 'cancelled'");
        });
        let expected = "rule `close-parent`, condition 1: its expression changed and its \
                        description didn't";

        let typed: PlayNodeUpdate = serde_json::from_value(json!({ "rules": stale })).unwrap();
        let err = service
            .update_play_node(&play.id, play.version, typed)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");

        let flat = crate::models::NodeUpdate::default().with_properties(json!({ "rules": stale }));
        let err = service
            .update_node(&play.id, play.version, flat)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(expected), "{err}");

        let rewritten = changed_rules(|rule| {
            rule["conditions"][0] = json!({
                "expr": "node.status == 'cancelled'",
                "description": "The task is cancelled"
            });
        });
        let update: PlayNodeUpdate = serde_json::from_value(json!({ "rules": rewritten })).unwrap();
        service
            .update_play_node(&play.id, play.version, update)
            .await
            .expect("a changed condition with a new description saves");
    }

    /// The description checks run only on a write that changes the rules, so
    /// a play whose stored rules would not pass them can still be switched
    /// off or renamed.
    #[tokio::test]
    async fn a_write_that_leaves_the_rules_alone_skips_the_description_checks() {
        let (service, _t) = create_test_service().await;
        let play = create(&service, "play", play_properties()).await;

        // Stored without the save-time gate, as a synced or imported play is.
        let blank = changed_rules(|rule| rule["description"] = json!(""));
        service
            .update_node_unchecked(
                &play.id,
                crate::models::NodeUpdate::default().with_properties(json!({ "rules": blank })),
            )
            .await
            .unwrap();
        let play = service.get_node(&play.id).await.unwrap().unwrap();

        let update: PlayNodeUpdate = serde_json::from_value(json!({
            "enabled": false,
            "description": "Switched off"
        }))
        .unwrap();
        let updated = service
            .update_play_node(&play.id, play.version, update)
            .await
            .expect("a write that leaves the rules alone succeeds");
        assert!(!crate::models::PlayFields::enabled_in(&updated.properties));
    }

    /// One case per action: a play whose action names a param the engine
    /// never reads does not save, through any write path. Before the rules
    /// were typed such a play saved and silently did nothing.
    #[tokio::test]
    async fn a_play_with_an_unknown_action_param_is_rejected_on_save() {
        let (service, _t) = create_test_service().await;
        let relationship = json!({
            "source_id": "{trigger.node.id}", "relationship_type": "blocks",
            "target_id": "{trigger.node.id}"
        });
        let with = |mut params: serde_json::Value, key: &str| {
            params[key] = json!("x");
            params
        };
        let cases = [
            (
                "create_node",
                with(json!({ "node_type": "task" }), "parent_id"),
                "parent_id",
            ),
            (
                "update_node",
                with(
                    json!({ "node_id": "{trigger.node.id}" }),
                    "lifecycle_status",
                ),
                "lifecycle_status",
            ),
            (
                "add_relationship",
                with(relationship.clone(), "weight"),
                "weight",
            ),
            (
                "remove_relationship",
                with(relationship, "edge_data"),
                "edge_data",
            ),
            ("reject", with(json!({ "message": "no" }), "code"), "code"),
        ];

        for (action_type, params, unknown) in cases {
            let rules = json!([{
                "name": "typo",
                "class": if action_type == "reject" { "invariant" } else { "reactive" },
                "description": "Test rule",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } },
                "actions": [{ "description": "Test action", "action_type": action_type, "params": params }]
            }]);

            let err = service
                .create_node_with_parent(CreateNodeParams {
                    id: None,
                    node_type: "play".to_string(),
                    content: "Typo".to_string(),
                    parent_id: None,
                    position: InsertPositionOwned::End,
                    properties: json!({ "rules": rules }),
                    lifecycle_status: None,
                })
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains(unknown),
                "{action_type}: the error must name `{unknown}`: {err}"
            );

            // The typed update refuses it before it reaches the service at
            // all: the update itself does not decode.
            assert!(
                serde_json::from_value::<PlayNodeUpdate>(json!({ "rules": rules })).is_err(),
                "{action_type}: a PlayNodeUpdate carrying `{unknown}` must not decode"
            );
        }
    }

    /// A query whose filters the query service could not execute is refused
    /// on write, through any path — here the generic update — rather than
    /// stored and discovered when the view is opened.
    #[tokio::test]
    async fn a_query_with_an_unexecutable_filter_is_rejected_on_write() {
        let (service, _t) = create_test_service().await;
        let query = create(&service, "query", saved_query_properties()).await;

        let err = service
            .update_node(
                &query.id,
                query.version,
                NodeUpdate {
                    properties: Some(json!({ "filters": [{ "type": "nonsense" }] })),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("'filters'"), "{err}");
    }
}
