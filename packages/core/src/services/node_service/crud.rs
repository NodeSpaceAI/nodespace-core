//! CRUD operations for NodeService.

use super::*;

/// Result of [`NodeService::update_with_version_check_returning_node_in_tx`]:
/// either the update landed (carrying what the caller's post-commit side
/// effects need — `content_changed` and the pre-update content, so
/// `sync_mentions` never has to re-fetch), or the OCC check failed. Not an
/// `Err` for the conflict case — see that method's own doc.
pub(crate) enum VersionCheckedUpdateOutcome {
    VersionConflict,
    Updated {
        // Boxed: `Node` is large enough that an unboxed field here would make
        // every `VersionCheckedUpdateOutcome` (including the zero-data
        // `VersionConflict` variant) pay for the biggest variant's size.
        node: Box<Node>,
        content_changed: bool,
        existing_content: String,
    },
}

impl NodeService {
    /// Create a new node
    ///
    /// Validates the node using the appropriate behavior (Text, Task, or Date),
    /// then inserts it into the database.
    ///
    /// # Arguments
    ///
    /// * `node` - The node to create
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node validation fails
    /// - Parent node doesn't exist (if parent_id is set)
    /// - Root node doesn't exist (if root_id is set)
    /// - Database insertion fails
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
    /// let node = Node::new(
    ///     "text".to_string(),
    ///     "My note".to_string(),
    ///     json!({}),
    /// );
    /// let id = service.create_node(node).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_node(&self, mut node: Node) -> Result<String, NodeServiceError> {
        let start = std::time::Instant::now();
        tracing::debug!(node_type = %node.node_type, node_id = %node.id, "create_node: START");

        // Auto-detect date nodes by ID format (YYYY-MM-DD) to ensure correct node_type.
        // This maintains data integrity regardless of caller mistakes.
        // NOTE: Date nodes can have custom content (not required to match ID).
        // We only enforce the node_type, not the content.
        if is_date_node_id(&node.id) {
            node.node_type = "date".to_string();
            // Content is preserved - date nodes can have custom content like "Custom Date Content"
        }

        // DatabaseSettingsNode is a singleton (ADR-037). The fixed reserved ID makes
        // creation idempotent: if one already exists, treat a second create as a no-op
        // and return the existing id rather than erroring. Mirrors the collection-name
        // uniqueness guard in SqliteStore::create_node, but non-fatal.
        if node.node_type == "database-settings" {
            if let Some(existing) = self
                .query_nodes_by_type("database-settings", None)
                .await?
                .into_iter()
                .next()
            {
                tracing::debug!(
                    node_id = %existing.id,
                    "create_node: database-settings singleton already exists, no-op"
                );
                return Ok(existing.id);
            }
        }

        // Collection-name-collision pre-check, mirrored here because the insert
        // below now runs through `create_node_in_tx` (ADR-069/ADR-060 §1 — see
        // that method's doc for why: it needs an open transaction so an
        // invariant rule's actions can join it). `SqliteStore::create_node_in_tx`
        // deliberately does NOT do collision detection/marking itself — the
        // marker write is a second, OCC-bypassing write kept outside the
        // transaction boundary by design (mirrors `mark_collection_name_collision`'s
        // own doc) — so this caller detects before the transaction (read-only,
        // safe before commit) and marks after it (once the node is durably
        // committed), exactly preserving `SqliteStore::create_node`'s prior
        // before/after timing for a plain top-level collection create.
        let colliding_collection = if node.node_type == "collection" {
            self.store
                .get_collection_by_name(&node.content)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to check collection name collision: {}",
                        e
                    ))
                })?
        } else {
            None
        };

        // All schema/behavior validation, normalization, title computation,
        // play-rule validation, the actual insert, and — new here —
        // synchronous invariant-rule dispatch (ADR-060 §1) happen inside
        // `create_node_in_tx`'s pipeline, run inside one transaction via
        // `with_transaction`. An invariant action failure fails this whole
        // call: the node is not created.
        let service = self.clone();
        let service_for_tx = service.clone();
        let node_for_tx = node.clone();
        let db_start = std::time::Instant::now();
        let created_id = service
            .with_transaction(move |tx| {
                Box::pin(async move {
                    service_for_tx
                        .create_node_in_tx(tx, node_for_tx, true)
                        .await
                })
            })
            .await?;
        tracing::debug!(
            "create_node: database insert completed in {}ms",
            db_start.elapsed().as_millis()
        );

        if let Some(existing) = colliding_collection {
            // Best-effort and non-blocking, same posture as
            // `SqliteStore::create_node`'s own call site: the node above is
            // already durably committed, so a marker-write failure must
            // never undo it.
            self.store
                .mark_collection_name_collision(&created_id, &existing.id)
                .await;
        }

        // Post-commit, best-effort `UniqueFieldCollision` detection (ADR-068):
        // the node above is already durably written, so a detection failure
        // must never fail or undo this create. This is the real caller the
        // old `mark_possible_duplicates` never had — see
        // `conflicts::detect_unique_field_collisions`.
        if let Err(e) = self.detect_unique_field_collisions(&created_id).await {
            tracing::warn!(
                node_id = %created_id,
                error = %e,
                "failed to detect unique-field collisions after create_node (create unaffected)"
            );
        }

        tracing::debug!(
            node_id = %created_id,
            "create_node: COMPLETE at {}ms",
            start.elapsed().as_millis()
        );
        Ok(created_id)
    }

    /// `_in_tx` twin of [`Self::create_node`] (ADR-069 §1b/S2). Delegates the
    /// validation/normalization/title/insert pipeline to
    /// [`Self::insert_node_in_tx_no_invariant_dispatch`] (the insert lands on
    /// `tx.store_tx()` instead of opening its own transaction, and the
    /// `NodeCreated` event is buffered via `self.emit_event` — routed to the
    /// transaction buffer by `BatchState::Transactional`, see
    /// `NodeService::with_transaction` — instead of relying on the store
    /// notifier, since `create_node_in_tx` the store method deliberately does
    /// not call `notify`), then additionally runs invariant-rule dispatch
    /// (ADR-060 §1) — the one thing that method does NOT do, by design.
    ///
    /// Does not handle the `database-settings` singleton short-circuit or
    /// collection-name-collision marking that `create_node` does — no
    /// composed caller (`create_node_with_parent`) creates either of those
    /// node shapes through this path today; if one ever does, add the
    /// missing behavior here rather than silently diverging.
    ///
    /// `is_root` is whether the node will have no parent once the caller's
    /// transaction commits — see
    /// [`Self::insert_node_in_tx_no_invariant_dispatch`] for why it cannot be
    /// derived here.
    pub(crate) async fn create_node_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node: Node,
        is_root: bool,
    ) -> Result<String, NodeServiceError> {
        // Validation/normalization/title/play-rule-gate pipeline and the
        // actual insert all live in `insert_node_in_tx_no_invariant_dispatch`
        // now — kept in exactly one place rather than duplicated here, since
        // this method and the invariant action executor
        // (`playbook::actions::execute_create_node_in_tx`) both need it.
        let node = self
            .insert_node_in_tx_no_invariant_dispatch(tx, node, is_root)
            .await?;

        // ADR-060 §1: invariant-rule dispatch runs HERE — pre-commit, inside
        // this same transaction, after the row lands but before
        // `with_transaction` commits it. An action failure returns `Err`
        // from this whole function, which propagates out through
        // `with_transaction`'s `?` and rolls back the insert above (and its
        // buffered `NodeCreated` event, discarded per ADR-069 §2) along with
        // everything the invariant action(s) wrote: fail-closed, no partial
        // state. See `invariants::dispatch_invariant_rules_in_tx`.
        //
        // Deliberately NOT reached when THIS insert is itself running inside
        // an invariant action's own `execute_create_node_in_tx` (which calls
        // `insert_node_in_tx_no_invariant_dispatch` directly, skipping this
        // method) — ADR-060 §2 requires invariant rules to be non-chaining,
        // depth 1: an invariant action must not itself trigger further rule
        // evaluation, invariant or reactive. Save-time validation only
        // proves the statically-decidable self-chaining case; this call
        // structure is what makes the general case (a DIFFERENT invariant
        // rule's trigger) impossible at the type level rather than relying
        // on a runtime depth counter.
        self.dispatch_invariant_rules_in_tx(tx, &node).await?;

        Ok(node.id)
    }

    /// Insert-only half of [`Self::create_node_in_tx`]: identical
    /// validation/normalization/title pipeline and the store insert, but
    /// WITHOUT invariant-rule dispatch — returns the fully-resolved `Node`
    /// (normalized properties/title/etc. applied) rather than just its id,
    /// so a caller needing it (both callers do) never has to re-read it back
    /// through the transaction a second time. The only callers are
    /// `create_node_in_tx` itself and the invariant-rule action executor
    /// (`playbook::actions::execute_create_node_in_tx`) — see
    /// `create_node_in_tx`'s doc for why an invariant action's own
    /// `create_node` must not recurse into dispatch.
    ///
    /// `is_root` must come from the caller: a node being inserted has no
    /// parent edge yet — a composed parent-edge write runs after this, in
    /// the same transaction — so deriving rootness from the store would
    /// classify every child as a root and title it with its own body text.
    pub(crate) async fn insert_node_in_tx_no_invariant_dispatch(
        &self,
        tx: &NodeServiceTx<'_>,
        mut node: Node,
        is_root: bool,
    ) -> Result<Node, NodeServiceError> {
        if is_date_node_id(&node.id) {
            node.node_type = "date".to_string();
        }

        self.behaviors.validate_node(&node)?;

        if node.node_type != "schema" {
            node.properties =
                Self::normalize_flat_properties_to_namespace(&node.node_type, &node.properties);
            // Resolve the full `extends` chain rather than this type's own
            // schema (ADR-078): an extending type's inherited fields are
            // declared by an ancestor, so a single-schema fetch would neither
            // default nor validate them, and would have no ownership map to
            // bucket them by.
            let (fields, owners, chain) = self.resolve_field_owners(&node.node_type).await?;
            if !fields.is_empty() {
                self.apply_schema_defaults_with_fields(&mut node, &fields, Some(&chain))?;
                node.properties =
                    Self::bucket_properties_by_owner(&node.node_type, &node.properties, &owners);
                self.validate_node_with_fields(&node, &fields, Some(&chain))?;
            }
        }

        if node.title.is_none() {
            node.title = self.compute_title(&node, Some(is_root)).await?;
        }

        if node.node_type == "play" {
            self.validate_play_rules(&node.properties).await?;
        }

        crate::db::SqliteStore::create_node_in_tx(tx.store_tx(), &node)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;

        self.emit_event(DomainEvent::NodeCreated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
        });

        Ok(node)
    }

    /// Create a node with parent relationship in a single operation
    ///
    /// This is the primary node creation API that enforces all business rules:
    /// 1. Auto-creates date containers (YYYY-MM-DD) if parent is a date ID
    /// 2. Validates parent exists (if provided)
    /// 3. Creates the node with proper validation
    /// 4. Establishes parent-child edge with correct sibling ordering
    ///
    /// # Arguments
    ///
    /// * `params` - CreateNodeParams containing all node creation parameters
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Parent doesn't exist (and isn't a valid date format)
    /// - Node validation fails
    /// - ID format is invalid (non-UUID for production nodes)
    ///
    /// Note: If `position` is `InsertPositionOwned::After(sibling_id)` and that
    /// sibling no longer exists or has moved to a different parent (stale hint from
    /// a race condition), the operation falls back to `End` rather than failing.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Create a child node under a date container
    /// let id = service.create_node_with_parent(CreateNodeParams {
    ///     id: None,
    ///     node_type: "text".to_string(),
    ///     content: "My note".to_string(),
    ///     parent_id: Some("2025-01-15".to_string()),
    ///     position: InsertPositionOwned::Beginning,
    ///     properties: json!({}),
    ///     lifecycle_status: None,
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_node_with_parent(
        &self,
        params: CreateNodeParams,
    ) -> Result<String, NodeServiceError> {
        let (node, parent, node_type) = self.prepare_create_node_with_parent(params).await?;
        let has_parent = parent.is_some();

        let created_id = if let Some((parent_id, position)) = parent {
            let service = self.clone();
            let service_for_tx = service.clone();
            service
                .with_transaction(move |tx| {
                    Box::pin(async move {
                        let created_id = service_for_tx.create_node_in_tx(tx, node, false).await?;
                        service_for_tx
                            .create_parent_edge_in_tx(
                                tx,
                                &created_id,
                                &parent_id,
                                position.as_ref(),
                            )
                            .await?;
                        Ok(created_id)
                    })
                })
                .await?
        } else {
            self.create_node(node).await?
        };

        self.queue_created_root_for_embedding(&created_id, &node_type, has_parent)
            .await;

        Ok(created_id)
    }

    /// `_in_tx` twin of [`Self::create_node_with_parent`] (ADR-069 §1b/S3),
    /// composable into a caller's own transaction — e.g. `handle_create_schema`
    /// composing the schema node create with its relationship declarations and
    /// description subtree. Shares the same validation/preparation pipeline;
    /// the node insert and parent edge (when there is a parent) land on the
    /// same `tx` the caller is already holding. Embedding-marker queueing is
    /// intentionally NOT reproduced here — it is derived state outside the
    /// boundary by design (ADR-069 §5); a caller needing it should queue
    /// after its own `with_transaction` commits, the way `handle_create_schema`
    /// does not need to (schema nodes are not embedded root content).
    pub(crate) async fn create_node_with_parent_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        params: CreateNodeParams,
    ) -> Result<String, NodeServiceError> {
        let (node, parent, _node_type) = self.prepare_create_node_with_parent(params).await?;

        let created_id = self.create_node_in_tx(tx, node, parent.is_none()).await?;
        if let Some((parent_id, position)) = parent {
            self.create_parent_edge_in_tx(tx, &created_id, &parent_id, position.as_ref())
                .await?;
        }

        Ok(created_id)
    }

    /// Shared validation/preparation pipeline for
    /// [`Self::create_node_with_parent`] and
    /// [`Self::create_node_with_parent_in_tx`] (steps 1-6 of the original
    /// method): date-container bootstrap, node_type/parent/sibling
    /// validation, ID generation, and title computation. Returns the fully
    /// constructed `Node` plus, when a parent was requested, its resolved
    /// `(parent_id, position)` — everything the caller needs to perform
    /// steps 6+7 either standalone or composed into an outer transaction.
    async fn prepare_create_node_with_parent(
        &self,
        params: CreateNodeParams,
    ) -> Result<
        (
            Node,
            Option<(String, crate::services::InsertPositionOwned)>,
            String,
        ),
        NodeServiceError,
    > {
        // Make params mutable so we can resolve InsertPositionOwned::End
        let mut params = params;
        let start = std::time::Instant::now();
        tracing::debug!(
            node_type = %params.node_type,
            has_parent = params.parent_id.is_some(),
            "create_node_with_parent: START"
        );

        // Step 1: Auto-create date container if parent is a date ID
        if let Some(ref parent_id) = params.parent_id {
            self.ensure_date_exists(parent_id).await?;
        }

        // Step 2: Reject a node_type that is neither a registered core type
        // nor an existing schema id. Without this, an invented id (a display
        // name, a paraphrase) falls through to CustomNodeBehavior and the node
        // is stored as a bare shell: no schema means nothing to validate
        // supplied properties against, so every one of them is silently
        // dropped and the caller is told the write succeeded.
        if self.behaviors.get(&params.node_type).is_none() {
            let schema_exists = self
                .store
                .get_schema(&params.node_type)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                .is_some();
            if !schema_exists {
                return Err(NodeServiceError::unknown_node_type(&params.node_type));
            }
        }

        // Step 3: Validate parent exists and is a container (if provided)
        if let Some(ref parent_id) = params.parent_id {
            let parent_node = self
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeServiceError::invalid_parent(parent_id.as_str()))?;

            if !self
                .behavior_for(&parent_node.node_type)
                .can_have_children()
            {
                return Err(NodeServiceError::not_a_container(
                    parent_id.as_str(),
                    &parent_node.node_type,
                ));
            }
        }

        // Step 4: Validate sibling (if After) - treat as best-effort hint.
        // If the sibling doesn't exist or has moved to a different parent, fall
        // back to End so new nodes land at the bottom rather than the top.
        // SQLite is synchronous/ACID: a node written by a prior awaited call is
        // immediately visible; a single check is sufficient.
        if let crate::services::InsertPositionOwned::After(ref sibling_id) = params.position.clone()
        {
            let sibling_valid = match self.get_node(sibling_id).await {
                Ok(Some(_)) => match self.get_parent(sibling_id).await {
                    Ok(sibling_parent) => {
                        let sibling_parent_id = sibling_parent.as_ref().map(|p| p.id.as_str());
                        sibling_parent_id == params.parent_id.as_deref()
                    }
                    Err(_) => false,
                },
                _ => false,
            };

            if !sibling_valid {
                tracing::warn!(
                    sibling_id = %sibling_id,
                    parent_id = ?params.parent_id,
                    "position sibling is stale (moved or deleted), falling back to End"
                );
                params.position = crate::services::InsertPositionOwned::End;
            }
        }

        // Step 5: Generate or validate node ID
        let node_id = if let Some(provided_id) = params.id {
            // Validate ID format based on node type
            if params.node_type == "date"
                || params.node_type == "schema"
                || provided_id.starts_with("test-")
            {
                // Date, schema, and test nodes can use their own ID format
                provided_id
            } else {
                // Production nodes must use UUID format
                uuid::Uuid::parse_str(&provided_id).map_err(|_| {
                    NodeServiceError::invalid_update(format!(
                        "Provided ID '{}' is not a valid UUID format (required for non-date/non-schema nodes)",
                        provided_id
                    ))
                })?;
                provided_id
            }
        } else if params.node_type == "date" {
            params.content.clone()
        } else if params.node_type == "schema" {
            let id = normalize_schema_id(&params.content);
            if id.is_empty() {
                return Err(NodeServiceError::invalid_update(
                    "Schema content must not be empty or contain only special characters"
                        .to_string(),
                ));
            }
            id
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        // Step 6: Create the node
        // Save node_type before moving into Node (needed for embedding check)
        let node_type = params.node_type.clone();

        // Determine title for @mention search
        // Schema-driven title_template support
        // Normalize properties to namespaced format so compute_title can find fields correctly.
        // (create_node will normalize again, but the result is idempotent)
        let title = {
            let normalized_props = if params.node_type != "schema" {
                Self::normalize_flat_properties_to_namespace(&params.node_type, &params.properties)
            } else {
                params.properties.clone()
            };
            let temp_node = Node {
                id: node_id.clone(),
                node_type: params.node_type.clone(),
                content: params.content.clone(),
                version: 1,
                properties: normalized_props,
                mentions: vec![],
                mentioned_in: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None,
                lifecycle_status: "active".to_string(),
            };
            // is_root = parent_id.is_none() — avoids a DB lookup at create time
            self.compute_title(&temp_node, Some(params.parent_id.is_none()))
                .await?
        };

        let parent = params
            .parent_id
            .clone()
            .map(|parent_id| (parent_id, params.position.clone()));

        let lifecycle_status = match params.lifecycle_status {
            Some(status) => {
                if !crate::models::is_valid_lifecycle_status(&status) {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Invalid lifecycle_status '{}'. Valid values: {:?}",
                        status,
                        crate::models::LIFECYCLE_STATUSES
                    )));
                }
                status
            }
            None => "active".to_string(),
        };

        let node = Node {
            id: node_id,
            node_type: params.node_type,
            content: params.content,
            version: 1,
            properties: params.properties,
            mentions: vec![],
            mentioned_in: vec![],
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            title,
            lifecycle_status,
        };

        tracing::debug!(
            "create_node_with_parent: prepared node{} at {}ms",
            if parent.is_some() {
                " + parent edge"
            } else {
                ""
            },
            start.elapsed().as_millis()
        );

        Ok((node, parent, node_type))
    }

    /// Post-commit follow-up for [`Self::create_node_with_parent`]: queue the
    /// created node's aggregate root for embedding regeneration. Deliberately
    /// outside the transaction boundary (ADR-069 §5) — embedding markers are
    /// derived state with their own reconciliation loop, and a queueing
    /// failure must never fail or roll back a create that already committed.
    async fn queue_created_root_for_embedding(
        &self,
        created_id: &str,
        node_type: &str,
        has_parent: bool,
    ) {
        if has_parent {
            // Child node created - queue root for embedding regeneration.
            // The new child's content should be included in the root's
            // aggregate embedding (root-aggregate model).
            #[cfg(feature = "nlp")]
            self.queue_root_for_embedding(created_id).await;
        } else {
            // Root node created - queue for embedding if embeddable type
            // (root-aggregate model). Stale markers are written
            // unconditionally (even without the `nlp` feature) so a build
            // re-enabled with NLP picks up existing roots without a manual
            // resync.
            if self.is_embeddable_type(node_type) {
                if let Err(e) = self.store.create_stale_embedding_marker(created_id).await {
                    tracing::warn!(
                        "Failed to create embedding marker for new root {}: {}",
                        created_id,
                        e
                    );
                } else {
                    tracing::debug!(
                        "Queued new root {} for embedding (direct creation)",
                        created_id
                    );
                    #[cfg(feature = "nlp")]
                    if let Some(waker) = self.embedding_waker.get() {
                        waker.wake();
                    }
                }
            }
        }
    }

    /// Auto-create date container if it doesn't exist in the database
    ///
    /// Date nodes (YYYY-MM-DD format) are lazily created when children reference them.
    /// This ensures date containers exist before child nodes are created under them.
    ///
    /// # Arguments
    ///
    /// * `node_id` - Potential date node ID to check/create
    ///
    /// # Returns
    ///
    /// `Ok(())` if not a date or date container exists/was created
    pub async fn ensure_date_exists(&self, node_id: &str) -> Result<(), NodeServiceError> {
        // Check if this is a date format (YYYY-MM-DD)
        if !is_date_node_id(node_id) {
            return Ok(()); // Not a date, nothing to do
        }

        // Check if date container already exists IN THE DATABASE
        // IMPORTANT: Call store.get_node() directly to bypass virtual date node logic
        // in get_node(). The virtual date nodes are only for read operations,
        // we need to check actual database state for auto-creation.
        let exists = self
            .store
            .get_node(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Database error: {}", e)))?
            .is_some();

        if exists {
            return Ok(()); // Already exists in database
        }

        // Auto-create the date container
        let date_node = Node::new_with_id(
            node_id.to_string(),
            "date".to_string(),
            node_id.to_string(), // Default content to date
            serde_json::json!({}),
        );

        self.create_node(date_node).await?;

        Ok(())
    }

    /// Get a node by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to fetch
    ///
    /// # Returns
    ///
    /// `Some(Node)` if found, `None` if not found
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
    /// if let Some(node) = service.get_node("node-id-123").await? {
    ///     println!("Found: {}", node.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_node(&self, id: &str) -> Result<Option<Node>, NodeServiceError> {
        // Delegate to SqliteStore
        if let Some(mut node) = self.store.get_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Database operation failed: {}", e),
            })
        })? {
            self.populate_mentions(&mut node).await?;
            Ok(Some(node))
        } else {
            // NOT in database - check if it's a virtual date node
            // Date nodes (YYYY-MM-DD format) are virtual until they have children
            if is_date_node_id(id) {
                // Return virtual date node (will auto-persist when children are added)
                // Date nodes are root-level containers (no parent/container relationships)
                let virtual_date = Node {
                    id: id.to_string(),
                    node_type: "date".to_string(),
                    content: id.to_string(), // Content MUST match ID for validation
                    version: 1,
                    created_at: chrono::Utc::now(),
                    modified_at: chrono::Utc::now(),
                    properties: serde_json::json!({}),
                    mentions: vec![],
                    mentioned_in: vec![],
                    title: None, // Date nodes don't have indexed titles
                    lifecycle_status: "active".to_string(),
                };
                return Ok(Some(virtual_date));
            }

            Ok(None)
        }
    }

    /// Update a node without version checking (no OCC).
    ///
    /// **Prefer `update_node()`** which enforces optimistic concurrency control.
    /// This unchecked variant is for internal operations (migrations, schema
    /// updates) where version conflicts are not a concern.
    ///
    /// Performs a partial update using the NodeUpdate struct. Only provided fields
    /// will be updated. Handles the double-Option pattern for nullable fields.
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to update
    /// * `update` - The fields to update
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - Validation fails after update
    /// - Database update fails
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
    /// let update = NodeUpdate::new()
    ///     .with_content("Updated content".to_string());
    /// service.update_node_unchecked("node-id", update).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node_unchecked(
        &self,
        id: &str,
        update: NodeUpdate,
    ) -> Result<(), NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // Get existing node to validate update
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // For simplicity with libsql, we'll fetch the node, apply updates, and replace entirely
        let mut updated = existing.clone();
        let mut content_changed = false;
        let mut node_type_changed = false;
        let mut properties_changed = false;

        if let Some(node_type) = update.node_type {
            node_type_changed = updated.node_type != node_type;
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        // NOTE: Sibling ordering is now handled via has_child relationship order field.
        // Use reorder_siblings() or move_node() for ordering changes.

        if let Some(properties) = update.properties {
            properties_changed = true;
            // Normalize flat client properties to namespaced format before merging
            // Skip for schema nodes - they use a special non-namespaced format
            if updated.node_type == "schema" {
                // Schema nodes use flat properties format (relationships, fields, etc.)
                Self::deep_merge_namespaced_properties(&mut updated.properties, properties);
            } else {
                // Client sends: { "status": "done" }
                // We convert to: { "task": { "status": "done" } } before merging with existing namespaced properties
                let normalized_properties =
                    Self::normalize_flat_properties_to_namespace(&updated.node_type, &properties);
                // Deep-merge namespaced properties
                Self::deep_merge_namespaced_properties(
                    &mut updated.properties,
                    normalized_properties,
                );
            }
        }

        // Step 1: Core behavior validation (PROTECTED)
        self.behaviors.validate_node(&updated)?;

        // Step 1.5: Apply schema defaults and validate (if node type changed)
        // Apply default values for missing fields when node type changes
        // Skip for schema nodes to avoid circular dependency
        if updated.node_type != "schema" {
            // On a type change, default the new type's fields first. Either
            // way the properties are re-bucketed before validation: an update
            // naming an inherited field arrives flat, normalizes into the
            // node's OWN bucket, and would sit there duplicating the
            // authoritative value in the declaring ancestor's bucket. The
            // readers then disagree — a scope-chain flattener takes the nearer
            // bucket, a base-scoped filter never looks in it, and the
            // validation merge resolves by map ordering.
            self.rebucket_and_validate(&mut updated, node_type_changed)
                .await?;
        }

        // Sync title when content, node_type, or properties change
        // Schema-driven title_template — also trigger on properties_changed
        let title_update = if content_changed || node_type_changed || properties_changed {
            let new_title = self.compute_title(&updated, None).await?;
            Some(new_title)
        } else {
            None // No title update needed
        };

        // Update node via store
        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            title: title_update,
            lifecycle_status: None, // Schema update doesn't change lifecycle_status
        };

        // Schema nodes go through the normal update path
        self.store
            .update_node(id, node_update, self.client_id.clone())
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // NOTE: NodeUpdated event is now automatically emitted by store notifier

        // Sync mentions if content changed
        if content_changed {
            if let Err(e) = self
                .sync_mentions(id, &existing.content, &updated.content)
                .await
            {
                // Log warning but don't fail the update - mention sync failures should not block content updates
                tracing::warn!("Failed to sync mentions for node {}: {}", id, e);
            }
        }

        Ok(())
    }

    /// `_in_tx` twin of [`Self::update_node_unchecked`] (ADR-069 §1b/S3).
    /// Identical validation/merge pipeline; the write lands on
    /// `tx.store_tx()` and the `NodeUpdated` event is buffered via
    /// `self.emit_event` instead of relying on the store notifier. Mention
    /// sync is intentionally NOT reproduced here — it stays outside the
    /// transaction boundary by design (ADR-069 §5, derived state that
    /// self-heals); the only current caller (`rename_schema_field_in_tx`)
    /// updates a schema node's `fields` JSON, whose content mention sync is
    /// a no-op in practice, and a future caller updating real content
    /// through this path should call `sync_mentions` itself after `commit`.
    pub(crate) async fn update_node_unchecked_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        id: &str,
        update: NodeUpdate,
    ) -> Result<(), NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        let mut updated = existing.clone();
        let mut node_type_changed = false;
        let mut content_changed = false;
        let mut properties_changed = false;

        if let Some(node_type) = update.node_type {
            node_type_changed = updated.node_type != node_type;
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        if let Some(properties) = update.properties {
            properties_changed = true;
            if updated.node_type == "schema" {
                Self::deep_merge_namespaced_properties(&mut updated.properties, properties);
            } else {
                let normalized_properties =
                    Self::normalize_flat_properties_to_namespace(&updated.node_type, &properties);
                Self::deep_merge_namespaced_properties(
                    &mut updated.properties,
                    normalized_properties,
                );
            }
        }

        self.behaviors.validate_node(&updated)?;

        if updated.node_type != "schema" {
            // Chain-resolved, per ADR-078 — see `rebucket_and_validate`.
            self.rebucket_and_validate(&mut updated, node_type_changed)
                .await?;
        }

        let title_update = if content_changed || node_type_changed || properties_changed {
            Some(self.compute_title(&updated, None).await?)
        } else {
            None
        };
        if let Some(ref new_title) = title_update {
            updated.title = new_title.clone();
        }

        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            title: title_update,
            lifecycle_status: None,
        };

        crate::db::SqliteStore::update_node_in_tx(tx.store_tx(), id, node_update)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Reflects the store's unconditional version bump — see
        // `SqliteStore::update_node_in_tx` (mirrors `update_node`'s own
        // "exactly one version bump per call" statement).
        updated.version += 1;

        self.emit_event(DomainEvent::NodeUpdated {
            node_id: id.to_string(),
            node_type: updated.node_type.clone(),
            node: updated,
            changed_properties: vec![],
        });

        Ok(())
    }

    /// General tx-scoped node update, for invariant-rule `update_node`
    /// actions (ADR-060 §1) and any other caller composing an update into an
    /// existing `with_transaction` unit of work.
    ///
    /// Unlike [`Self::update_node_unchecked_in_tx`], reads `existing` via
    /// [`crate::db::SqliteStore::get_node_in_tx`] rather than
    /// `self.get_node` — this sees a node inserted **earlier in the same
    /// transaction**, which the ordinary pooled-reader `get_node` cannot see
    /// until commit. That case is the common one here: the canonical
    /// invariant rule shape stamps a property onto the very node whose
    /// creation triggered it, which exists only inside this transaction
    /// until commit. `update_node_unchecked_in_tx` is left as-is for its one
    /// existing caller (`rename_schema_field_in_tx`, which always targets an
    /// already-committed schema node) rather than changed underneath it.
    ///
    /// No optimistic-concurrency check: within one transaction, nothing else
    /// can observe or mutate `id` mid-transaction (SQLite serializes writers
    /// to one connection), so there is no concurrent writer to race against —
    /// the same reasoning `update_node_with_version_check_in_tx`'s doc gives
    /// for why its OCC check is sound inside a transaction applies here too,
    /// just without a caller-supplied `expected_version` to check against.
    pub(crate) async fn update_node_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        id: &str,
        update: NodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        let existing = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        let mut updated = existing.clone();
        let mut node_type_changed = false;
        let mut content_changed = false;
        let mut properties_changed = false;

        if let Some(node_type) = update.node_type {
            node_type_changed = updated.node_type != node_type;
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        if let Some(properties) = update.properties {
            properties_changed = true;
            if updated.node_type == "schema" {
                Self::deep_merge_namespaced_properties(&mut updated.properties, properties);
            } else {
                let normalized_properties =
                    Self::normalize_flat_properties_to_namespace(&updated.node_type, &properties);
                Self::deep_merge_namespaced_properties(
                    &mut updated.properties,
                    normalized_properties,
                );
            }
        }

        if let Some(status) = update.lifecycle_status {
            updated.lifecycle_status = status;
        }

        self.behaviors.validate_node(&updated)?;

        if updated.node_type != "schema" {
            // Chain-resolved, per ADR-078 — see `rebucket_and_validate`.
            self.rebucket_and_validate(&mut updated, node_type_changed)
                .await?;
        }

        let title_update = if content_changed || node_type_changed || properties_changed {
            Some(self.compute_title(&updated, None).await?)
        } else {
            None
        };
        if let Some(ref new_title) = title_update {
            updated.title = new_title.clone();
        }

        // update_node_with_version_check_in_tx re-reads `id` via `tx` itself
        // to gate on `expected_version`, then writes the fully-resolved
        // fields computed above. Passing `existing.version` (read above, also
        // via `tx`) can never mismatch: nothing else can have mutated this
        // row between that read and this call inside one transaction.
        let result = crate::db::SqliteStore::update_node_with_version_check_in_tx(
            tx.store_tx(),
            id,
            existing.version,
            crate::models::NodeUpdate {
                node_type: Some(updated.node_type.clone()),
                content: Some(updated.content.clone()),
                properties: Some(updated.properties.clone()),
                title: title_update,
                lifecycle_status: Some(updated.lifecycle_status.clone()),
            },
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let node = result.map_err(|actual_version| {
            NodeServiceError::version_conflict(id, existing.version, actual_version)
        })?;

        self.emit_event(DomainEvent::NodeUpdated {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            node: node.clone(),
            changed_properties: vec![],
        });

        Ok(node)
    }

    /// Update node with optimistic concurrency control (version check)
    ///
    /// Internal method that returns the updated node directly to avoid
    /// redundant fetches. Delegates the validation/normalization/title/write
    /// pipeline to [`Self::update_with_version_check_returning_node_in_tx`]
    /// (runs inside one transaction via `with_transaction`, mirroring
    /// `create_node`'s own wrapping of `create_node_in_tx` — see that
    /// method's doc), then performs the same post-commit, best-effort side
    /// effects this method always has: embedding-queue-on-content-change and
    /// mentions sync. Both stay outside the transaction, unchanged from
    /// before — a failure in either must never undo an already-committed
    /// update.
    pub(crate) async fn update_with_version_check_returning_node(
        &self,
        id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<Option<Node>, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // Collection-name-collision pre-check, mirrored here for the same
        // reason `create_node`'s own pre-check exists (see that method's
        // doc): the write below now runs through
        // `update_with_version_check_returning_node_in_tx`, and
        // `SqliteStore::update_node_with_version_check_in_tx` deliberately
        // does NOT do collision detection/marking itself — same posture as
        // `create_node_in_tx`, the marker write is a second,
        // OCC-bypassing write kept outside the transaction boundary by
        // design. Read-only and safe before the transaction opens; if the
        // node doesn't exist or changes underneath this read before the real
        // write lands, the OCC check inside the transaction is the actual
        // correctness guard — this pre-check only decides whether a
        // best-effort marker write happens afterward, never whether the
        // update itself succeeds.
        let previous = self.get_node(id).await?;
        let colliding_collection = match &previous {
            Some(previous) => {
                let updated_content = update
                    .content
                    .clone()
                    .unwrap_or_else(|| previous.content.clone());
                let updated_node_type = update
                    .node_type
                    .clone()
                    .unwrap_or_else(|| previous.node_type.clone());
                if updated_node_type == "collection" && updated_content != previous.content {
                    self.store
                        .get_collection_by_name(&updated_content)
                        .await
                        .map_err(|e| {
                            NodeServiceError::query_failed(format!(
                                "Failed to check collection name collision: {}",
                                e
                            ))
                        })?
                        .filter(|existing| existing.id != id)
                } else {
                    None
                }
            }
            None => None,
        };

        let service = self.clone();
        let service_for_tx = service.clone();
        let id_for_tx = id.to_string();
        let outcome = service
            .with_transaction(move |tx| {
                Box::pin(async move {
                    service_for_tx
                        .update_with_version_check_returning_node_in_tx(
                            tx,
                            &id_for_tx,
                            expected_version,
                            update,
                        )
                        .await
                })
            })
            .await?;

        let (updated_node, content_changed, existing_content) = match outcome {
            VersionCheckedUpdateOutcome::VersionConflict => return Ok(None),
            VersionCheckedUpdateOutcome::Updated {
                node,
                content_changed,
                existing_content,
            } => (node, content_changed, existing_content),
        };

        if let Some(existing) = colliding_collection {
            // Best-effort and non-blocking, same posture as `create_node`'s
            // own call site: the node above is already durably committed
            // (we're past the `VersionConflict` early-return, so the OCC
            // check genuinely passed and the write landed), so a
            // marker-write failure must never undo it.
            self.store
                .mark_collection_name_collision(id, &existing.id)
                .await;
        }

        // Queue root for embedding regeneration if content changed (root-aggregate model)
        // Fire-and-forget: don't block the update response on embedding queue operations
        #[cfg(feature = "nlp")]
        if content_changed {
            let store = self.store.clone();
            let behaviors = self.behaviors.clone();
            let node_id = id.to_string();
            let embedding_waker = self.embedding_waker.clone();
            tokio::spawn(async move {
                Self::queue_root_for_embedding_async(
                    &store,
                    &behaviors,
                    &node_id,
                    embedding_waker.get(),
                )
                .await;
            });
        }

        // Sync mentions if content changed
        if content_changed {
            if let Err(e) = self
                .sync_mentions(id, &existing_content, &updated_node.content)
                .await
            {
                // Log warning but don't fail the update
                tracing::warn!("Failed to sync mentions for node {}: {}", id, e);
            }
        }

        Ok(Some(*updated_node))
    }

    /// Tx-scoped twin of [`Self::update_with_version_check_returning_node`]
    /// (ADR-060 §2). Same validation/normalization/title pipeline as before,
    /// but reading `existing` and writing the version-checked update through
    /// `tx` instead of the pooled reader / the store's own transaction —
    /// which is what makes it possible to run synchronous invariant-rule
    /// dispatch (`dispatch_invariant_rules_for_update_in_tx`) inside the
    /// SAME transaction as the write, after it lands but before
    /// `with_transaction` commits: a rejecting invariant rule returns `Err`
    /// here, which rolls back the version-checked update above it AND
    /// discards the `NodeUpdated` event buffered by `emit_event` below
    /// (never flushed on rollback — see `NodeService::with_transaction`'s
    /// own doc) — no partial write, no broadcast, for a rejected update.
    ///
    /// Returns [`VersionCheckedUpdateOutcome::VersionConflict`] on an OCC
    /// mismatch rather than an `Err` — an expected, common outcome the
    /// caller maps to `NodeServiceError::VersionConflict` itself (mirrors
    /// `update_node_with_version_check_in_tx`'s own `Ok(Err(actual_version))`
    /// convention, ADR-069 §2a: a version mismatch is not a transaction
    /// failure).
    pub(crate) async fn update_with_version_check_returning_node_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<VersionCheckedUpdateOutcome, NodeServiceError> {
        let existing = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // Build updated node state
        let mut updated = existing.clone();
        let mut content_changed = false;
        let mut node_type_changed = false;
        let mut properties_changed = false;

        if let Some(node_type) = update.node_type {
            node_type_changed = updated.node_type != node_type;
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        // NOTE: Sibling ordering is now handled via has_child relationship order field.
        // Use reorder_siblings() or move_node() for ordering changes.

        if let Some(properties) = update.properties {
            properties_changed = true;
            // Normalize flat client properties to namespaced format before merging
            // Skip for schema nodes - they use a special non-namespaced format
            if updated.node_type == "schema" {
                // Schema nodes use flat properties format (relationships, fields, etc.)
                Self::deep_merge_namespaced_properties(&mut updated.properties, properties);
            } else {
                let normalized_properties =
                    Self::normalize_flat_properties_to_namespace(&updated.node_type, &properties);
                // Deep-merge namespaced properties
                Self::deep_merge_namespaced_properties(
                    &mut updated.properties,
                    normalized_properties,
                );
            }
        }

        // Step 1: Core behavior validation (PROTECTED)
        self.behaviors.validate_node(&updated)?;

        // Step 2: Schema validation (USER-EXTENSIBLE)
        // Every type that declares a schema is validated, user-defined types
        // included; `validate_node_against_schema` no-ops for types without one.
        // The lookup behind it is a single primary-key read. Non-tx reads
        // like this one are safe from inside a write transaction — same
        // precedent as `insert_node_in_tx_no_invariant_dispatch`'s own
        // schema/title/play-validation calls.
        if updated.node_type != "schema" {
            // Re-bucket before validating, same reasoning as the update paths
            // above: an inherited field arrives flat and must be moved to its
            // declaring ancestor's bucket, or it exists in two places. No
            // defaulting here — this path does not change the node's type.
            self.rebucket_and_validate(&mut updated, false).await?;
        }

        // Synchronous play validation gate — reject invalid rule changes before persist
        if updated.node_type == "play" && properties_changed {
            self.validate_play_rules(&updated.properties).await?;
        }

        // Sync title when content, node_type, or properties change
        // Schema-driven title_template — also trigger on properties_changed
        let title_update = if content_changed || node_type_changed || properties_changed {
            let new_title = self.compute_title(&updated, None).await?;
            Some(new_title)
        } else {
            None
        };

        // Create node update
        // Pass through lifecycle_status if provided
        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            title: title_update,
            lifecycle_status: update.lifecycle_status,
        };

        // Perform atomic update with version check, tx-scoped.
        let result = crate::db::SqliteStore::update_node_with_version_check_in_tx(
            tx.store_tx(),
            id,
            expected_version,
            node_update,
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let updated_node = match result {
            Ok(node) => node,
            Err(_actual_version) => return Ok(VersionCheckedUpdateOutcome::VersionConflict),
        };

        // Real changed_properties — the same diff the store's own notifier
        // closure computes for the non-tx path (`compute_property_changes`,
        // this module's own helper); `_in_tx` store methods bypass that
        // notifier entirely (see its doc), so this is required here to keep
        // property_changed-triggered plays (reactive or invariant) and
        // WatchNodes consumers seeing accurate diffs, not an empty vec.
        let changed_properties =
            compute_property_changes(&existing.properties, &updated_node.properties);

        // Buffered, not broadcast yet (`BatchState::Transactional`) — only
        // flushed if this whole transaction commits. See this method's own
        // doc for why emitting before dispatch below is safe.
        self.emit_event(DomainEvent::NodeUpdated {
            node_id: updated_node.id.clone(),
            node_type: updated_node.node_type.clone(),
            node: updated_node.clone(),
            changed_properties: changed_properties.clone(),
        });

        // ADR-060 §2: synchronous invariant-rule dispatch for
        // property_changed triggers, inside this same transaction. A
        // rejecting rule's `Err` propagates out through the `?` below,
        // through this whole function, and through the caller's
        // `with_transaction`, rolling back everything above — including the
        // buffered event.
        self.dispatch_invariant_rules_for_update_in_tx(tx, &updated_node, &changed_properties)
            .await?;

        // Re-read the trigger node's final state, tx-consistent, rather than
        // returning the `updated_node` snapshot captured above: an invariant
        // rule's own action can be a self-referential `update_node` on the
        // SAME node (the canonical shape — stamp a derived property on the
        // node whose change just triggered the rule), which writes a second
        // time inside this same transaction. Returning the pre-dispatch
        // snapshot would silently omit that second write from what the
        // caller (and ultimately the RPC response) sees, even though it is
        // genuinely, durably part of what this transaction committed.
        let final_node = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        Ok(VersionCheckedUpdateOutcome::Updated {
            node: Box::new(final_node),
            content_changed,
            existing_content: existing.content,
        })
    }

    /// Update a node with OCC and return the updated node
    ///
    /// This is the primary update API that:
    /// 1. Validates update has changes
    /// 2. Applies update with version check
    /// 3. Returns detailed error on version conflict
    /// 4. Returns the updated node on success
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to update
    /// * `expected_version` - Version for optimistic concurrency control
    /// * `update` - Fields to update
    ///
    /// # Returns
    ///
    /// The updated Node with new version number
    ///
    /// # Errors
    ///
    /// Returns error on:
    /// - Empty update (no changes)
    /// - Node not found
    /// - Version conflict (with expected/actual versions)
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
    /// let update = NodeUpdate::new().with_content("Updated content".to_string());
    /// let updated = service.update_node("node-id", 5, update).await?;
    /// println!("New version: {}", updated.version);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node(
        &self,
        node_id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        // Validate update has changes
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // A seeded node's config or guidance aspect becomes user-owned the
        // moment its content or properties are edited through the normal
        // update path — reseed's replace path goes through delete_node +
        // create_node_with_parent (or, for a config-only replace, a direct
        // property merge), not here, so it never trips this. Checked before
        // the update so a version-conflict below doesn't leave a partial flag
        // write behind.
        //
        // Which flag gets set depends on which node is being edited, not
        // which field: `_seed` lives only on a seeded node's root (see
        // `prepare_nodes_from_template`), so editing the root itself is a
        // config edit (`config_modified`), while editing one of its markdown
        // children — which carry no `_seed` of their own — is a guidance
        // edit (`guidance_modified`), stamped on the child's root. This is
        // tier-independent: `Starter` and `System` seeded nodes are guarded
        // identically.
        let touches_content = update.content.is_some() || update.properties.is_some();
        let stamp_target: Option<(String, &'static str)> = if touches_content {
            match self.store.get_node(node_id).await {
                Ok(Some(existing)) => {
                    if let Some(seed) = existing.properties.get("_seed") {
                        // Editing the seeded root itself: a config edit.
                        let already_modified = seed
                            .get("config_modified")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        (!already_modified).then_some((node_id.to_string(), "config_modified"))
                    } else {
                        // Not itself a seeded root — if some ancestor is,
                        // this is an edit to that seed's guidance children.
                        match self.get_root_id(node_id).await {
                            Ok(root_id) if root_id != node_id => {
                                match self.store.get_node(&root_id).await {
                                    Ok(Some(root)) if root.properties.get("_seed").is_some() => {
                                        let already_modified = root
                                            .properties
                                            .get("_seed")
                                            .and_then(|s| s.get("guidance_modified"))
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        (!already_modified)
                                            .then_some((root_id, "guidance_modified"))
                                    }
                                    _ => None,
                                }
                            }
                            _ => None,
                        }
                    }
                }
                _ => None,
            }
        } else {
            None
        };

        // NOTE: Removed redundant get_node() call here - update_with_version_check_returning_node
        // already fetches the node and handles not-found case

        // Apply update with version check - returns the updated node directly
        match self
            .update_with_version_check_returning_node(node_id, expected_version, update)
            .await?
        {
            Some(updated_node) => {
                if let Some((stamp_node_id, flag)) = stamp_target {
                    // Best-effort, OCC-bypassing second write (see set_property_bool's
                    // doc comment). A concurrent writer landing between the update
                    // above and this stamp could have its own version bump masked
                    // by this call's WHERE-less json_set — the node's `version`
                    // column would then undercount real mutations by one. Blast
                    // radius is limited to that bookkeeping counter: `_seed.config_modified`
                    // / `_seed.guidance_modified` itself is idempotent (setting it to
                    // `true` twice is a no-op), so no seeded content or user edit can
                    // be lost this way.
                    let json_path = format!("$._seed.{flag}");
                    if let Err(e) = self
                        .store
                        .set_property_bool(&stamp_node_id, &json_path, true)
                        .await
                    {
                        tracing::warn!(
                            node_id = %stamp_node_id,
                            flag,
                            error = %e,
                            "Failed to stamp seed modification flag after edit"
                        );
                    }
                }

                // Post-commit, best-effort `UniqueFieldCollision` detection
                // (ADR-068) — same posture as `create_node`'s call: the
                // update above already succeeded and must never be undone by
                // a detection failure. Skipped when this update touched
                // NEITHER content nor properties (a title- or
                // lifecycle_status-only change): unique-field values live
                // under `properties`, so such an update cannot introduce —
                // or need to re-detect against — a collision, avoiding a
                // get_node + get_schema_node round-trip. A content-only
                // update still runs this: `detect_unique_field_collisions`
                // re-derives from the node's CURRENT properties regardless
                // of what changed, and re-detection of an already-open
                // collision is what bumps `occurrences`/`last_seen_at` (see
                // `redetecting_the_same_collision_bumps_occurrences_not_a_new_record`).
                if touches_content {
                    if let Err(e) = self.detect_unique_field_collisions(node_id).await {
                        tracing::warn!(
                            node_id,
                            error = %e,
                            "failed to detect unique-field collisions after update_node (update unaffected)"
                        );
                    }
                }

                Ok(updated_node)
            }
            None => {
                // The version-gated UPDATE matched no row for one of two
                // reasons — the node was concurrently DELETED, or its version
                // moved. Disambiguate against the REAL persisted row: `get_node`
                // virtualizes a date page, so it would report a phantom version 1
                // for a deleted date node → a false `version_conflict{actual:1}`
                // instead of `NotFound` (and an absent regular node → `actual:0`).
                match self
                    .store
                    .persisted_version(node_id)
                    .await
                    .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                {
                    None => Err(NodeServiceError::node_not_found(node_id)),
                    Some(actual) => Err(NodeServiceError::version_conflict(
                        node_id,
                        expected_version,
                        actual,
                    )),
                }
            }
        }
    }

    /// Sync mention relationships when node content changes
    pub(crate) async fn sync_mentions(
        &self,
        node_id: &str,
        old_content: &str,
        new_content: &str,
    ) -> Result<(), NodeServiceError> {
        let old_mentions: HashSet<String> = extract_mentions(old_content).into_iter().collect();
        let new_mentions: HashSet<String> = extract_mentions(new_content).into_iter().collect();

        // Calculate diff
        let to_add: Vec<&String> = new_mentions.difference(&old_mentions).collect();
        let to_remove: Vec<&String> = old_mentions.difference(&new_mentions).collect();

        // Get parent ID once for all mention checks (optimized: use get_parent_id instead of get_parent)
        let parent_id = self
            .store
            .get_parent_id(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Add new mentions (filter out self-references and root-level self-references)
        for mentioned_id in to_add {
            // Skip direct self-references
            if mentioned_id.as_str() == node_id {
                tracing::debug!("Skipping self-reference: {} -> {}", node_id, mentioned_id);
                continue;
            }

            // Skip root-level self-references (child mentioning its own parent)
            if let Some(ref pid) = parent_id {
                if mentioned_id.as_str() == pid.as_str() {
                    tracing::debug!(
                        "Skipping root-level self-reference: {} -> {} (parent: {})",
                        node_id,
                        mentioned_id,
                        pid
                    );
                    continue;
                }
            }

            // Auto-create date nodes when mentioned.
            // Date nodes are lazily created, but we need them to exist for the
            // "Mentioned by" panel to work. This ensures the relationship can be created.
            if is_date_node_id(mentioned_id) {
                if let Err(e) = self.ensure_date_exists(mentioned_id).await {
                    tracing::warn!(
                        "Failed to ensure date node exists for mention: {} -> {}: {}",
                        node_id,
                        mentioned_id,
                        e
                    );
                    // Continue anyway - the mention creation will fail if node doesn't exist
                }
            }

            if let Err(e) = self.create_mention(node_id, mentioned_id).await {
                tracing::warn!(
                    "Failed to create mention: {} -> {}: {}",
                    node_id,
                    mentioned_id,
                    e
                );
            }
        }

        // Remove old mentions
        for mentioned_id in to_remove {
            // Skip direct self-references (shouldn't exist, but be safe)
            if mentioned_id.as_str() == node_id {
                continue;
            }

            if let Err(e) = self.delete_mention(node_id, mentioned_id).await {
                tracing::warn!(
                    "Failed to delete mention: {} -> {}: {}",
                    node_id,
                    mentioned_id,
                    e
                );
            }
        }

        Ok(())
    }

    /// Delete a node without version checking (no OCC).
    ///
    /// **Prefer `delete_node()`** which enforces optimistic concurrency control.
    /// This unchecked variant is for internal operations (diagnostics cleanup)
    /// where version conflicts are not a concern.
    ///
    /// Deletes a node and all its children (cascade delete).
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to delete
    ///
    /// # Errors
    ///
    /// Returns error if node doesn't exist or database deletion fails
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
    /// service.delete_node_unchecked("node-id-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node_unchecked(
        &self,
        id: &str,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        // Delegate to SqliteStore
        let result = self
            .store
            .delete_node(id, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                    context: format!("Database operation failed: {}", e),
                })
            })?;

        // NOTE: NodeDeleted event is now automatically emitted by store notifier

        // Idempotent delete: return success even if node doesn't exist
        Ok(result)
    }

    /// Delete node with optimistic concurrency control (version check)
    ///
    /// This method performs an atomic delete with version checking to prevent
    /// race conditions when multiple clients attempt to delete or modify the same node.
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to delete
    /// * `expected_version` - Version the client expects (from their last read)
    ///
    /// # Returns
    ///
    /// * `Ok(rows_affected)` - Number of rows deleted (0 = version mismatch or not found, 1 = success)
    /// * `Err(NodeServiceError)` - Database errors
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
    /// let rows = service.delete_with_version_check("node-123", 5).await?;
    ///
    /// if rows == 0 {
    ///     // Either version conflict or node doesn't exist
    ///     // Caller should check if node still exists to distinguish
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<usize, NodeServiceError> {
        let rows_affected = self
            .store
            .delete_with_version_check(id, expected_version, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to delete node with version check: {}",
                    e
                ))
            })?;

        // NOTE: NodeDeleted event is now automatically emitted by store notifier

        Ok(rows_affected)
    }

    /// Delete a node and its entire `has_child` subtree atomically with OCC.
    ///
    /// The target node's version is checked at `expected_version`. Descendants are removed
    /// unconditionally inside the same transaction — a conflict or failure leaves the subtree
    /// fully intact. One `NodeDeleted` event is emitted per deleted node after commit.
    ///
    /// **Access gate (ADR-041):** before anything is deleted, the subtree (target + all
    /// descendants) is checked against `subtree_access_gate()`. If any node is unreadable by
    /// the actor, the delete is refused in full — no node is removed — and a
    /// [`NodeServiceError::SubtreeAccessDenied`] error is returned (distinct from a hierarchy
    /// violation, so the daemon can map it to its own wire status and the UI can show a
    /// dedicated refusal modal). Community installs never see a refusal here:
    /// `AlwaysAllowGate` is the default and only a synced Pro daemon injects a gate that can
    /// deny. This check runs before the transaction opens, not inside it — a rollback-based
    /// check would do wasted work every time.
    ///
    /// Returns `DeleteResult` with `existed=true` and `deleted_count` (target + all descendants)
    /// on success, or `existed=false` when the target node was already gone.
    pub async fn delete_node(
        &self,
        node_id: &str,
        expected_version: i64,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        // Capture root before deletion for embedding queue.
        let root_id_for_embedding = self.get_root_id(node_id).await.ok();

        // Nothing to check or delete if the target is already gone — matches the idempotent
        // absent-target behavior `delete_subtree_atomic` has always had.
        let target_exists = self
            .store
            .get_node(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to read target: {}", e)))?
            .is_some();
        if !target_exists {
            return Ok(crate::models::DeleteResult {
                existed: false,
                deleted_count: 0,
            });
        }

        // Compute the subtree once; both the access gate and the delete itself use this exact
        // set so they can never see different subtrees.
        let subtree_ids = self.store.collect_subtree_ids(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to collect subtree: {}", e))
        })?;

        if let access_gate::SubtreeAccessDecision::Denied { inaccessible_count } = self
            .subtree_access_gate()
            .check_subtree_access(&subtree_ids)
            .await
        {
            debug_assert!(
                inaccessible_count > 0,
                "a Denied decision must report at least one inaccessible node — \
                 a gate reporting 0 would surface a confusing refusal message"
            );
            return Err(NodeServiceError::subtree_access_denied(inaccessible_count));
        }

        let (existed, deleted_nodes) = self
            .store
            .delete_subtree_atomic(
                node_id,
                expected_version,
                &subtree_ids,
                self.client_id.clone(),
            )
            .await
            .map_err(|e| {
                let msg = e.to_string();
                // Parse version_conflict sentinel: "version_conflict:<id>:<expected>:<actual>"
                if let Some(rest) = msg.strip_prefix("version_conflict:") {
                    let parts: Vec<&str> = rest.splitn(3, ':').collect();
                    if parts.len() == 3 {
                        let exp: i64 = parts[1].parse().unwrap_or(expected_version);
                        let act: i64 = parts[2].parse().unwrap_or(0);
                        return NodeServiceError::version_conflict(node_id, exp, act);
                    }
                }
                NodeServiceError::query_failed(msg)
            })?;

        if !existed {
            return Ok(crate::models::DeleteResult {
                existed: false,
                deleted_count: 0,
            });
        }

        // Queue root for embedding regeneration when a non-root node was deleted.
        #[cfg(feature = "nlp")]
        if let Some(root_id) = root_id_for_embedding {
            if root_id != node_id {
                self.queue_root_for_embedding(&root_id).await;
            }
        }

        Ok(crate::models::DeleteResult {
            existed: true,
            deleted_count: deleted_nodes.len() as u64,
        })
    }

    /// Bump a node's version without changing any content.
    ///
    /// Used by operations like reorder that need OCC (optimistic concurrency control)
    /// even though they don't modify the node's content directly.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to update
    /// * `expected_version` - The version the caller expects (for OCC)
    ///
    /// # Returns
    ///
    /// Ok(Node) with updated version if bump succeeds, Err if version mismatch or node not found
    pub async fn update_node_with_version_bump(
        &self,
        node_id: &str,
        expected_version: i64,
    ) -> Result<Node, NodeServiceError> {
        // Get current node to preserve its values
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Create update with current values (no actual changes, just version bump)
        let node_update = crate::models::NodeUpdate {
            node_type: Some(node.node_type.clone()),
            content: Some(node.content.clone()),
            properties: Some(node.properties.clone()),
            title: None,            // Don't update title on version bump
            lifecycle_status: None, // Don't update lifecycle_status on version bump
        };

        // Perform atomic update with version check
        let result = self
            .store
            .update_node_with_version_check(
                node_id,
                expected_version,
                node_update,
                self.client_id.clone(),
                self.execution_context.clone(),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Check if update succeeded (version matched)
        let updated_node = result.ok_or_else(|| {
            NodeServiceError::query_failed(format!(
                "Version conflict: expected version {} for node {}",
                expected_version, node_id
            ))
        })?;

        // NOTE: NodeUpdated event is now automatically emitted by store notifier

        Ok(updated_node)
    }

    /// `_in_tx` twin of [`Self::update_node_with_version_bump`] (ADR-069
    /// §1b/S4). Same no-op content/properties, version-checked bump; the
    /// read of current values and the checked write both land on
    /// `tx.store_tx()` (ADR-069 §3: the OCC re-check is sound inside the
    /// transaction, closing the TOCTOU window the standalone method's
    /// pre-read leaves open). A version mismatch surfaces as
    /// `NodeServiceError::VersionConflict` — an expected outcome of
    /// concurrent writes, never `TransactionFailed` (ADR-069 §2a) — which
    /// rolls back the whole unit of work via the caller's `?`.
    pub(crate) async fn update_node_with_version_bump_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        expected_version: i64,
    ) -> Result<Node, NodeServiceError> {
        let result = crate::db::SqliteStore::update_node_with_version_check_in_tx(
            tx.store_tx(),
            node_id,
            expected_version,
            NodeUpdate::default(),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let updated_node = result.map_err(|actual_version| {
            NodeServiceError::version_conflict(node_id, expected_version, actual_version)
        })?;

        self.emit_event(DomainEvent::NodeUpdated {
            node_id: updated_node.id.clone(),
            node_type: updated_node.node_type.clone(),
            node: updated_node.clone(),
            changed_properties: vec![],
        });

        Ok(updated_node)
    }

    /// Upsert a node with automatic parent creation - single transaction
    ///
    /// Creates parent node if it doesn't exist, then upserts the child node.
    /// All operations happen in a single transaction to prevent database locking.
    ///
    /// # Arguments
    /// * `node_id` - ID of the node to upsert
    /// * `content` - Node content
    /// * `node_type` - Type of node (text, task, date)
    /// * `parent_id` - Parent node ID (will be created as date node if missing)
    ///
    /// # Returns
    /// * `Ok(())` - Operation successful
    /// * `Err(NodeServiceError)` - If transaction fails
    pub async fn upsert_node_with_parent(
        &self,
        node_id: &str,
        content: &str,
        node_type: &str,
        parent_id: &str,
        _root_id: &str, // Deprecated: hierarchy now managed via relationships
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Ensure parent exists (create if missing)
        if self
            .store
            .get_node(parent_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to check parent existence: {}", e))
            })?
            .is_none()
        {
            // Create parent as date node
            let parent_node = Node::new(
                "date".to_string(),
                parent_id.to_string(),
                serde_json::json!({}),
            );
            self.store
                .create_node(
                    parent_node,
                    self.client_id.clone(),
                    self.execution_context.clone(),
                )
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to create parent node: {}", e))
                })?;

            // NOTE: NodeCreated event is now automatically emitted by store notifier
        }

        // Enforce container rule: the (now-existent) parent must accept children
        {
            let parent_node = self
                .store
                .get_node(parent_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to fetch parent: {}", e))
                })?
                .ok_or_else(|| NodeServiceError::invalid_parent(parent_id))?;
            if !self
                .behavior_for(&parent_node.node_type)
                .can_have_children()
            {
                return Err(NodeServiceError::not_a_container(
                    parent_id,
                    &parent_node.node_type,
                ));
            }
        }

        // Upsert the node (update if exists, create if not)
        if let Some(existing) = self.store.get_node(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check node existence: {}", e))
        })? {
            // Update existing node
            // Recompute title from the new content — this node is always
            // attached to a parent by the end of this call, so it's never
            // root (mirrors `create_node`'s `Some(params.parent_id.is_none())`).
            let mut node_for_title = existing.clone();
            node_for_title.content = content.to_string();
            let new_title = self.compute_title(&node_for_title, Some(false)).await?;

            let update = NodeUpdate {
                content: Some(content.to_string()),
                title: Some(new_title),
                // NOTE: Sibling ordering now handled via has_child relationship order field
                ..Default::default()
            };
            self.store
                .update_node(&existing.id, update, self.client_id.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to update node: {}", e))
                })?;

            // NOTE: NodeUpdated event is now automatically emitted by store notifier

            // Update parent relationship via edge (handles sibling ordering)
            let actual_order = self
                .store
                .move_node(node_id, Some(parent_id), before_sibling_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to update parent: {}", e))
                })?;

            // Emit RelationshipUpdated event (unified relationship events)
            self.emit_event(DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, node_id),
                    parent_id,
                    node_id,
                    "has_child",
                    serde_json::json!({"order": actual_order}),
                ),
            });
        } else {
            // Create new node
            let node = Node {
                id: node_id.to_string(),
                node_type: node_type.to_string(),
                content: content.to_string(),
                version: 1,
                properties: serde_json::json!({}),
                mentions: vec![],
                mentioned_in: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None, // Title managed by NodeService for root/task nodes
                lifecycle_status: "active".to_string(),
            };
            self.store
                .create_node(node, self.client_id.clone(), self.execution_context.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to create node: {}", e))
                })?;

            // NOTE: NodeCreated event is now automatically emitted by store notifier

            // Create parent relationship via edge (handles sibling ordering)
            let actual_order = self
                .store
                .move_node(node_id, Some(parent_id), before_sibling_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to set parent: {}", e))
                })?;

            // Emit RelationshipCreated event (unified relationship events)
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, node_id),
                    parent_id,
                    node_id,
                    "has_child",
                    serde_json::json!({"order": actual_order}),
                ),
            });
        }

        Ok(())
    }

    // =========================================================================
    // Private CRUD helpers
    // =========================================================================

    /// Validate a node's properties against its schema definition
    pub(crate) async fn validate_node_against_schema(
        &self,
        node: &Node,
    ) -> Result<(), NodeServiceError> {
        // Resolve the full `extends` chain (ADR-078), so an extending type's
        // inherited fields are validated rather than skipped. An empty result
        // means no schema anywhere in the chain — not all types have one, and
        // validation passes for those.
        let (fields, _owners, chain) = self.resolve_field_owners(&node.node_type).await?;
        if fields.is_empty() {
            return Ok(());
        }

        self.validate_node_with_fields(node, &fields, Some(&chain))
    }

    /// Validate play rules before persisting.
    pub(crate) async fn validate_play_rules(
        &self,
        properties: &serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        use crate::playbook::types::{parse_rule, parse_rules_from_properties};

        // Step 1: Parse rules from properties
        let rule_defs = match parse_rules_from_properties(properties) {
            Ok(defs) => defs,
            Err(e) => {
                return Err(NodeServiceError::PlayValidationFailed {
                    errors: format!("Failed to parse play rules: {}", e),
                });
            }
        };

        // Step 2: Parse each rule definition into a ParsedRule
        let mut parsed_rules = Vec::with_capacity(rule_defs.len());
        for def in &rule_defs {
            match parse_rule(def) {
                Ok(rule) => parsed_rules.push(std::sync::Arc::new(rule)),
                Err(e) => {
                    return Err(NodeServiceError::PlayValidationFailed {
                        errors: format!("Failed to parse rule '{}': {}", def.name, e),
                    });
                }
            }
        }

        // Step 3: Run the full validation pipeline (schema checks, CEL compile, paths)
        if let Err(errors) = crate::playbook::validation::validate_play(&parsed_rules, self).await {
            return Err(NodeServiceError::play_validation_failed(&errors));
        }

        Ok(())
    }

    /// Apply schema default values to missing fields using pre-loaded fields
    ///
    /// "Missing" is judged against **every** bucket on the node, not just the
    /// node's own type's. Under `extends` (ADR-078) an inherited field's value
    /// lives in its declaring ancestor's bucket, so checking only
    /// `properties[node_type]` would read a populated inherited field as
    /// missing and overwrite the user's value with the schema default — and
    /// because `bucket_properties_by_owner` then moves that default into the
    /// ancestor's bucket unconditionally, the original value is destroyed
    /// rather than merely shadowed.
    pub(crate) fn apply_schema_defaults_with_fields(
        &self,
        node: &mut Node,
        fields: &[crate::models::SchemaField],
        scope_chain: Option<&[String]>,
    ) -> Result<(), NodeServiceError> {
        // Ensure properties is an object
        if !node.properties.is_object() {
            node.properties = serde_json::json!({});
        }

        // Which fields already hold a value a reader will actually see.
        //
        // Scoped to the node's `extends` chain, matching
        // `validate_node_with_fields` exactly. The two must agree on what
        // "present" means: if defaulting counted a dormant bucket (left by an
        // earlier `node_type` change) but validation did not, a defaulted
        // field would be suppressed as already-present and then read as
        // absent — leaving the node with no value and no error, since a field
        // with a default never trips the required check.
        //
        // Computed before the mutable borrow below.
        let own_chain = std::slice::from_ref(&node.node_type);
        let chain = scope_chain.unwrap_or(own_chain);
        let already_present: std::collections::HashSet<String> = node
            .properties
            .as_object()
            .map(|obj| {
                chain
                    .iter()
                    .filter_map(|scope| obj.get(scope.as_str()))
                    .filter_map(|v| v.as_object())
                    .flat_map(|bucket| bucket.keys().cloned())
                    .collect()
            })
            .unwrap_or_default();

        // Get mutable reference to properties object
        let props_obj = node.properties.as_object_mut().unwrap();

        // Get or create the type namespace
        // Properties are stored under properties[node_type][field_name]
        let type_namespace = props_obj
            .entry(&node.node_type)
            .or_insert_with(|| serde_json::json!({}));

        let type_props = type_namespace.as_object_mut().ok_or_else(|| {
            NodeServiceError::invalid_update(format!(
                "Type namespace for '{}' is not an object",
                node.node_type
            ))
        })?;

        // Apply defaults for fields absent from every bucket. Defaults land in
        // the node's own namespace; `bucket_properties_by_owner` then moves an
        // inherited one into its declaring ancestor's bucket, which is why
        // that call must follow this one.
        for field in fields {
            if !already_present.contains(&field.name) {
                // Apply default value if one is defined
                if let Some(default_value) = &field.default {
                    type_props.insert(field.name.clone(), default_value.clone());
                }
            }
        }

        Ok(())
    }

    /// Deep-merge namespaced properties
    pub(crate) fn deep_merge_namespaced_properties(
        existing: &mut serde_json::Value,
        new: serde_json::Value,
    ) {
        if let (Some(existing_obj), Some(new_obj)) = (existing.as_object_mut(), new.as_object()) {
            for (key, value) in new_obj {
                // If both existing and new have the same key as objects, deep merge
                if let (Some(existing_ns), Some(new_ns)) = (
                    existing_obj.get_mut(key).and_then(|v| v.as_object_mut()),
                    value.as_object(),
                ) {
                    // Deep merge: update fields within the namespace
                    for (field_key, field_value) in new_ns {
                        existing_ns.insert(field_key.clone(), field_value.clone());
                    }
                } else {
                    // Otherwise replace the key (for new namespaces or non-object values)
                    existing_obj.insert(key.clone(), value.clone());
                }
            }
        } else {
            // If either is not an object, just replace (shouldn't happen normally)
            *existing = new;
        }
    }

    /// Normalize flat properties input into namespaced storage format.
    ///
    /// Callers supply properties in the flat, bare-name shape the write surfaces
    /// document (`--property status=done`, `NodeUpdate.properties`); this moves
    /// them under the type's own namespace (`{ "task": { "status": "done" } }`),
    /// which is the shape storage and `flatten_namespaced_properties` agree on.
    ///
    /// Field vs. sibling-namespace is decided by the key's `_` prefix alone, not
    /// by the value's JSON type. `_`-prefixed keys (`_seed`, `_schema_version`)
    /// are internal bookkeeping that must land at a fixed, type-independent path,
    /// so they stay at the top level; every other key is a field of this type and
    /// is namespaced whatever its value — object-valued fields included.
    ///
    /// The `_` prefix is reserved for that bookkeeping on both sides of the
    /// round-trip: `flatten_namespaced_properties` already drops `_`-prefixed
    /// keys from every read surface, so treating them as non-fields here makes
    /// the write path agree with the read path rather than diverging from it.
    ///
    /// Classifying on the value's type instead would be ambiguous in exactly the
    /// case that matters: an object-valued field of a user-defined type is
    /// indistinguishable from a namespace by shape, and treating it as a
    /// namespace hoists it out of the type key where every read path looks,
    /// dropping the value with no error. Deciding on the key prefix removes the
    /// ambiguity without consulting the schema, so no read is needed here.
    ///
    /// Dormant namespaces (an old type's key left behind by a type change) are
    /// not this function's concern — they only ever exist in *stored* properties,
    /// which are merged with this function's output rather than passed through
    /// it, and the flattener already hides them from read output.
    pub(crate) fn normalize_flat_properties_to_namespace(
        node_type: &str,
        properties: &serde_json::Value,
    ) -> serde_json::Value {
        let Some(props_obj) = properties.as_object() else {
            return properties.clone();
        };

        // Already in storage shape - return as-is. This is what makes the
        // function idempotent, which `create_node_with_parent` relies on: it
        // normalizes once to compute the title, then hands the result to
        // `create_node`, which normalizes again. Without this a second pass
        // would nest the type key inside itself.
        if let Some(type_namespace) = props_obj.get(node_type) {
            if type_namespace.is_object() {
                return properties.clone();
            }
        }

        // Separate internal bookkeeping keys from the type's own fields
        let mut namespaced = serde_json::Map::new();
        let mut flat_props = serde_json::Map::new();

        for (key, value) in props_obj {
            if key.starts_with('_') {
                namespaced.insert(key.clone(), value.clone());
            } else {
                flat_props.insert(key.clone(), value.clone());
            }
        }

        // Move flat properties into the current type's namespace
        if !flat_props.is_empty() {
            let type_ns = namespaced
                .entry(node_type.to_string())
                .or_insert_with(|| serde_json::json!({}));
            if let Some(type_obj) = type_ns.as_object_mut() {
                for (key, value) in flat_props {
                    type_obj.insert(key, value);
                }
            }
        } else if !namespaced.contains_key(node_type) {
            // Ensure the current type namespace exists even if empty
            namespaced.insert(node_type.to_string(), serde_json::json!({}));
        }

        serde_json::Value::Object(namespaced)
    }

    /// Resolve the node's `extends` chain, re-bucket its properties by
    /// declaring owner, and validate — the sequence every write path owes a
    /// node before persisting it (ADR-078).
    ///
    /// Extracted because the same ~12 lines appeared at four call sites, and
    /// **one of them was originally missed** — the wrong-bucket bug this
    /// sequence exists to prevent was itself caused by the duplication. A
    /// fifth write path now gets it right by calling this rather than by
    /// copying it correctly.
    ///
    /// Three orderings are load-bearing and are the reason this is one
    /// function rather than three calls at each site:
    ///
    /// 1. **Defaults before bucketing.** `apply_schema_defaults_with_fields`
    ///    puts defaults in the node's *own* bucket; bucketing then moves any
    ///    inherited one into its declaring ancestor's. Reversing them strands
    ///    an inherited default in the wrong bucket.
    /// 2. **Bucketing before validation.** Validation merges the buckets in
    ///    the chain; a field sitting in two of them resolves by map ordering.
    /// 3. **The empty-fields branch.** With no resolved fields there is
    ///    nothing to bucket by, and validation falls back to the single-schema
    ///    path — but only when defaults were not requested, since a type
    ///    change into a type with no schema has nothing to default either.
    ///
    /// `apply_defaults` is true only on a node-type change, where the node may
    /// be missing fields its new type declares. An update that leaves the type
    /// alone must not re-default: the node already has its values, and
    /// defaulting again would resurrect a field the caller deliberately
    /// cleared.
    async fn rebucket_and_validate(
        &self,
        node: &mut Node,
        apply_defaults: bool,
    ) -> Result<(), NodeServiceError> {
        // Resolve the whole `extends` chain once, not just this type's own
        // schema: an extending type must default and validate its ancestors'
        // fields too, and needs the ownership map to re-bucket them.
        let (fields, owners, chain) = self.resolve_field_owners(&node.node_type).await?;

        if fields.is_empty() {
            if !apply_defaults {
                self.validate_node_against_schema(node).await?;
            }
            return Ok(());
        }

        if apply_defaults {
            self.apply_schema_defaults_with_fields(node, &fields, Some(&chain))?;
        }
        node.properties =
            Self::bucket_properties_by_owner(&node.node_type, &node.properties, &owners);
        self.validate_node_with_fields(node, &fields, Some(&chain))?;

        Ok(())
    }

    /// Re-bucket already-normalized properties by which schema declares each
    /// field, for a node whose type participates in an `extends` chain
    /// (ADR-078).
    ///
    /// Runs *after* [`Self::normalize_flat_properties_to_namespace`], which
    /// has already put every field under the node's own type. This moves each
    /// inherited field into its declaring ancestor's bucket, so an issue node
    /// ends up as `{"issue": {"severity": …}, "task": {"status": …}}`.
    ///
    /// Splitting the two apart keeps the flat-input normalization schema-free
    /// (it decides field-vs-bookkeeping on the `_` prefix alone, with no read),
    /// and confines the schema-dependent step to the write paths that have
    /// already resolved field ownership anyway.
    ///
    /// `owners` maps field name → declaring schema id. A field with no entry
    /// stays in the node's own bucket: it is either undeclared (ad-hoc, no
    /// ancestor can claim it) or declared by the node's own type.
    pub(crate) fn bucket_properties_by_owner(
        node_type: &str,
        properties: &serde_json::Value,
        owners: &std::collections::HashMap<String, String>,
    ) -> serde_json::Value {
        let Some(props_obj) = properties.as_object() else {
            return properties.clone();
        };

        // Nothing to move when no field is owned by an ancestor. The common
        // case by far — every node type is unextended until something
        // declares `extends` — so this keeps the unextended write path at one
        // map scan and no allocation.
        let has_inherited = owners.values().any(|owner| owner != node_type);
        if !has_inherited {
            return properties.clone();
        }

        let mut out = serde_json::Map::new();
        // Preserve non-own buckets and bookkeeping keys as they stand; only
        // the node's own bucket is redistributed.
        for (key, value) in props_obj {
            if key != node_type {
                out.insert(key.clone(), value.clone());
            }
        }

        let own_bucket = props_obj.get(node_type).and_then(|v| v.as_object());
        let mut own_remaining = serde_json::Map::new();

        if let Some(own_bucket) = own_bucket {
            for (field, value) in own_bucket {
                match owners.get(field) {
                    Some(owner) if owner != node_type => {
                        let bucket = out
                            .entry(owner.clone())
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(bucket_obj) = bucket.as_object_mut() {
                            bucket_obj.insert(field.clone(), value.clone());
                        }
                    }
                    _ => {
                        own_remaining.insert(field.clone(), value.clone());
                    }
                }
            }
        }

        // The own bucket always exists, even when empty — every read surface
        // keys on it, and an absent bucket would read as "no properties"
        // rather than "no own properties".
        out.insert(
            node_type.to_string(),
            serde_json::Value::Object(own_remaining),
        );

        serde_json::Value::Object(out)
    }

    /// Validate a node against pre-loaded schema fields
    ///
    /// Reads across **every** bucket present on the node, not just its own
    /// type's. Under `extends` (ADR-078) an inherited field is stored in the
    /// declaring ancestor's bucket, so scanning only `properties[node_type]`
    /// would report every inherited required field as missing and skip enum
    /// validation on it entirely. Unextended nodes have exactly one bucket, so
    /// this is identical to the previous behavior for them.
    pub(crate) fn validate_node_with_fields(
        &self,
        node: &Node,
        fields: &[crate::models::SchemaField],
        scope_chain: Option<&[String]>,
    ) -> Result<(), NodeServiceError> {
        // Merge the buckets in this node's `extends` chain into one lookup.
        //
        // Scoped to `scope_chain` rather than "every bucket present": a node
        // retyped from something else keeps a dormant bucket from its previous
        // type, and merging that indiscriminately would let a stale value
        // satisfy a required field, or be enum-validated in place of the real
        // one. Nearest scope wins, matching every other read surface.
        let own_chain = std::slice::from_ref(&node.node_type);
        let chain = scope_chain.unwrap_or(own_chain);
        let node_props: Option<serde_json::Map<String, serde_json::Value>> =
            node.properties.as_object().map(|obj| {
                let mut merged = serde_json::Map::new();
                for scope in chain {
                    let Some(bucket) = obj.get(scope.as_str()).and_then(|v| v.as_object()) else {
                        continue;
                    };
                    for (field, field_value) in bucket {
                        if field.starts_with('_') {
                            continue;
                        }
                        merged
                            .entry(field.clone())
                            .or_insert_with(|| field_value.clone());
                    }
                }
                merged
            });
        let node_props = node_props.as_ref();

        // Validate each field in the schema
        for field in fields {
            let field_value = node_props.and_then(|props| props.get(&field.name));

            // Check required fields
            // Allow missing required fields if they have a default value defined
            if field.required.unwrap_or(false) && field_value.is_none() && field.default.is_none() {
                return Err(NodeServiceError::invalid_update(format!(
                    "Required field '{}' is missing from {} node",
                    field.name, node.node_type
                )));
            }

            // Validate enum fields
            if field.field_type == "enum" {
                if let Some(value) = field_value {
                    if let Some(value_str) = value.as_str() {
                        // Get all valid enum values (core + user)
                        let mut valid_values = Vec::new();
                        if let Some(core_vals) = &field.core_values {
                            valid_values.extend(core_vals.clone());
                        }
                        if let Some(user_vals) = &field.user_values {
                            valid_values.extend(user_vals.clone());
                        }

                        // Check if the value matches any EnumValue.value
                        let is_valid = valid_values.iter().any(|ev| ev.value == value_str);
                        if !is_valid {
                            let valid_labels: Vec<_> = valid_values
                                .iter()
                                .map(|ev| format!("{} ({})", ev.label, ev.value))
                                .collect();
                            return Err(NodeServiceError::invalid_update(format!(
                                "Invalid value '{}' for enum field '{}'. Valid values: {}",
                                value_str,
                                field.name,
                                valid_labels.join(", ")
                            )));
                        }
                    } else if !value.is_null() {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Enum field '{}' must be a string or null",
                            field.name
                        )));
                    }
                }
            }

            // Validate object-shaped fields structurally: a field declared
            // `object` must hold a JSON object, and a field declared `array`
            // with `item_type: "object"` must hold an array whose every
            // element is a JSON object. This is deliberately scoped to the
            // `object` shape only — not every declared `field_type` (string,
            // number, boolean, date) — because that is the specific,
            // concretely-declared gap (core_schemas.rs declares `object`
            // fields and object-item arrays with no enforcement at all), and
            // a survey of every writer against every declared type is a much
            // larger, separately-scoped effort. Widening further risks
            // repeating the `ai-chat.status` incident, where enabling
            // enum validation broke 16 daemon tests because the schema and
            // the writers had already drifted apart.
            //
            // Deliberately NOT recursive: a nested `object` field declared via
            // `fields`/`item_fields` (e.g. `ai-chat.messages[].args`, which
            // core_schemas.rs leaves without declared sub-fields on purpose,
            // since tool-call arguments are freeform) is not walked into.
            // `validate_node_with_fields` itself only ever sees the type's
            // top-level `fields` list, never nested ones, so recursing would
            // be a larger structural change than this fix's scope.
            if field.field_type == "object" {
                if let Some(value) = field_value {
                    if !value.is_object() && !value.is_null() {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Field '{}' is declared as type 'object' but received {}",
                            field.name,
                            crate::schema::json_type_name(value)
                        )));
                    }
                }
            } else if field.field_type == "array" && field.item_type.as_deref() == Some("object") {
                if let Some(value) = field_value {
                    if !value.is_null() {
                        match value.as_array() {
                            None => {
                                return Err(NodeServiceError::invalid_update(format!(
                                    "Field '{}' is declared as type 'array' (item type 'object') \
                                     but received {}",
                                    field.name,
                                    crate::schema::json_type_name(value)
                                )));
                            }
                            Some(items) => {
                                for (index, item) in items.iter().enumerate() {
                                    if !item.is_object() {
                                        return Err(NodeServiceError::invalid_update(format!(
                                            "Field '{}' is declared as type 'array' with item type \
                                             'object', but item {} is {}",
                                            field.name,
                                            index,
                                            crate::schema::json_type_name(item)
                                        )));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Future: Add more type validation (number ranges, string formats, etc.)
        }

        Ok(())
    }

    /// Compute the indexed title for a node.
    ///
    /// Every root carries a title — it is what general search's keyword half
    /// matches. A `date` page or `schema` has no template, so it falls through
    /// to its content (`2026-09-23`, `Task`), like any other root.
    pub(crate) async fn compute_title(
        &self,
        node: &Node,
        is_root: Option<bool>,
    ) -> Result<Option<String>, NodeServiceError> {
        // Check for title_template in the schema for this node type
        match self.get_schema_node(&node.node_type).await {
            Ok(Some(schema)) => {
                if let Some(template) = &schema.title_template {
                    // Properties are stored namespaced: { "node_type": { "field": value } }
                    // Unwrap to the inner namespace object for template interpolation
                    let flat_props = node
                        .properties
                        .get(&node.node_type)
                        .unwrap_or(&node.properties);
                    return Ok(Some(crate::utils::interpolate_title_template_with_schema(
                        template,
                        flat_props,
                        &schema.fields,
                    )));
                }
            }
            Ok(None) => {} // No schema for this type — fall through to content-based logic
            Err(e) => {
                // Schema lookup failed; fall through to content-based title rather than
                // blocking the create/update operation
                tracing::warn!(
                    node_type = %node.node_type,
                    error = %e,
                    "compute_title: schema lookup failed, falling back to content-based title"
                );
            }
        }

        // Fall back to content-based title
        let title = match node.node_type.as_str() {
            "task" | "collection" => Some(crate::utils::strip_markdown(&node.content)),
            _ => {
                let root = match is_root {
                    Some(v) => v,
                    None => self
                        .store
                        .get_parent_id(&node.id)
                        .await
                        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                        .is_none(),
                };
                if root {
                    Some(crate::utils::strip_markdown(&node.content))
                } else {
                    None
                }
            }
        };
        Ok(title)
    }

    /// Re-derive `node_id`'s title after an edge write set whether it is a root.
    ///
    /// A title follows rootness for every type without a template — a root's
    /// is its content, a child's is none — so every path that gains or drops a
    /// `has_child` edge calls this. Without it an indented root keeps its title
    /// and title search returns it as a document, and an outdented child stays
    /// unfindable by name.
    pub(crate) async fn refresh_title_for_rootness(
        &self,
        node_id: &str,
        is_root: bool,
    ) -> Result<(), NodeServiceError> {
        let Some(node) = self
            .store
            .get_node(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        else {
            return Ok(());
        };
        let title = self.compute_title(&node, Some(is_root)).await?;
        if title != node.title {
            self.store
                .set_title(node_id, title.as_deref())
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        }
        Ok(())
    }

    /// `_in_tx` twin of [`Self::refresh_title_for_rootness`].
    pub(crate) async fn refresh_title_for_rootness_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        is_root: bool,
    ) -> Result<(), NodeServiceError> {
        let Some(node) = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        else {
            return Ok(());
        };
        let title = self.compute_title(&node, Some(is_root)).await?;
        if title != node.title {
            crate::db::SqliteStore::set_title_in_tx(tx.store_tx(), node_id, title.as_deref())
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        }
        Ok(())
    }

    /// Check if a node exists
    pub(crate) async fn node_exists(&self, id: &str) -> Result<bool, NodeServiceError> {
        let node = self.store.get_node(id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check node existence: {}", e))
        })?;
        Ok(node.is_some())
    }
}
