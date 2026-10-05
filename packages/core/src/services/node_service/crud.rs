//! CRUD operations for NodeService.

use super::*;

/// `(effective_fields, field_name -> owning_schema_id, chain)`, as
/// [`NodeService::resolve_field_owners`] returns it: what a write needs of a
/// node type to default, bucket and validate a node of it. A bulk write
/// resolves it once per distinct type, not once per row
/// ([`NodeService::field_ownership_in_batch`]).
pub(crate) type FieldOwnershipInfo = (
    Vec<crate::models::SchemaField>,
    std::collections::HashMap<String, String>,
    Vec<String>,
);

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
    /// Refuse an update that changes whether a node is a core schema.
    ///
    /// The core-schema delete refusal (`assert_schema_deletable`) reads
    /// `isCore` from the stored row, so it is only as strong as the guarantee
    /// that nothing rewrites it: an update clearing `isCore` — or retyping the
    /// row away from `schema` — would make a core type deletable. Whether a
    /// schema is core is fixed when it is created. The `schema_core_status_fixed`
    /// trigger backs this up on every write path; checking here gives the
    /// update paths a readable error.
    pub(crate) fn ensure_schema_core_status_unchanged(
        existing: &Node,
        updated: &Node,
    ) -> Result<(), NodeServiceError> {
        if crate::models::schema_node::is_core_schema(existing)
            != crate::models::schema_node::is_core_schema(updated)
        {
            return Err(NodeServiceError::invalid_update(format!(
                "schema_is_core: whether schema '{}' is core is fixed when it is created",
                existing.id
            )));
        }
        Ok(())
    }

    /// Refuse a type change the type system does not allow.
    ///
    /// **Into an abstract type.** No node has an abstract type as its
    /// `node_type` (ADR-086 §6), so a retype into one is refused exactly as a
    /// create is.
    ///
    /// **Into an `ai-chat`** (or a subtype of one). No node may reference an
    /// ai-chat node (ADR-061 §8), and that holds only because it is enforced
    /// when an edge is *created*: edges onto an ai-chat are refused there (see
    /// `refuse_ai_chat_target`). A retype would carry the node's existing
    /// inbound mentions and relationships into the chat, so a chat can only
    /// come into being by being created as one. Retyping *out of* ai-chat, or
    /// between two chat types, is allowed: the node gains no inbound
    /// references, so the invariant still holds.
    ///
    /// **Into a type whose structural rules the node's place breaks**
    /// (ADR-089): the new type's `parent` and `children` rules are checked
    /// against the parent and children the node has, and theirs against the
    /// new type.
    ///
    /// `tx` is the transaction the update runs in, when it runs in one: the
    /// structural check then reads the node's parent and children on it, so
    /// it sees a move or a create made earlier in the same transaction. That
    /// matters most for a type that needs a parent, which no database rule
    /// backs up.
    pub(crate) async fn ensure_retype_allowed(
        &self,
        tx: Option<&NodeServiceTx<'_>>,
        existing: &Node,
        updated: &Node,
    ) -> Result<(), NodeServiceError> {
        if existing.node_type == updated.node_type {
            return Ok(());
        }
        self.ensure_instantiable(&updated.node_type).await?;
        // A node that may be referenced never becomes one that may not: its
        // inbound references would outlive the change (ADR-061 §8).
        if !self.accepts_inbound_references(&updated.node_type).await?
            && self.accepts_inbound_references(&existing.node_type).await?
        {
            return Err(NodeServiceError::invalid_update(format!(
                "Node '{}' cannot be converted to an ai-chat node or a chat message; create a \
                 new one instead",
                existing.id
            )));
        }
        match tx {
            Some(tx) => {
                crate::db::SqliteStore::assert_retype_keeps_structure_in_tx(
                    tx.store_tx(),
                    &existing.id,
                    &updated.node_type,
                )
                .await
            }
            None => {
                self.store
                    .assert_retype_keeps_structure(&existing.id, &updated.node_type)
                    .await
            }
        }
        .map_err(NodeServiceError::from_store)
    }

    /// Refuse a generic update that changes a schema's structural rules
    /// (ADR-089).
    ///
    /// `update_schema` is the one path that changes them: it checks that the
    /// types a rule names exist, that the rule only tightens the base type's,
    /// and that no existing node breaks it. The database copies whatever a
    /// schema node declares into the rules it enforces, so a generic update
    /// must not be a way around those checks.
    pub(crate) fn ensure_schema_structure_unchanged(
        existing: &Node,
        updated: &Node,
    ) -> Result<(), NodeServiceError> {
        if !crate::models::CoreNodeType::Schema.is_exactly(&updated.node_type) {
            return Ok(());
        }
        for rule in ["children", "parent"] {
            if existing.properties.get(rule) != updated.properties.get(rule) {
                return Err(NodeServiceError::invalid_update(format!(
                    "The \"{rule}\" rule of schema '{}' can only be changed with update_schema, \
                     which checks it against the type's base and its existing nodes.",
                    existing.id
                )));
            }
        }
        // Context paths are checked against the schemas when `update_schema`
        // saves them (ADR-094 §2), so they have the same one write path.
        let paths = crate::models::schema_node::CONTEXT_PATHS_KEY;
        if existing.properties.get(paths) != updated.properties.get(paths) {
            return Err(NodeServiceError::invalid_update(format!(
                "The context paths of schema '{}' can only be changed with update_schema \
                 (add_context_paths, remove_context_paths), which checks each path against \
                 the schemas.",
                existing.id
            )));
        }
        Ok(())
    }

    /// Refuse a generic properties patch from a typed client when it names a
    /// declared field of a core type that has a typed update (ADR-086 §7).
    ///
    /// A typed client (the desktop app, the dev-proxy) writes a core type's
    /// fields only through that type's typed update, so each field has one
    /// write path from it and one typed shape. The generic update stays for
    /// content, extension fields and types with no typed update. The CLI and
    /// the agent are not typed clients: they name core fields as bare keys
    /// and are validated by the same pipeline the typed update lowers into.
    ///
    /// The rule is the core type's own. A node of a type that *extends* one
    /// travels as a generic node and has no typed update to prefer, so its
    /// inherited fields are written through the generic update.
    ///
    /// `new_node_type` is the type the update retypes the node to, if any;
    /// the patch is read against the type the node will have.
    pub async fn ensure_no_typed_core_fields(
        &self,
        node_id: &str,
        new_node_type: Option<&str>,
        properties: &serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        let Some(patch) = properties.as_object() else {
            return Ok(());
        };
        let node_type = match new_node_type {
            Some(node_type) => node_type.to_string(),
            None => match self.get_node(node_id).await? {
                Some(node) => node.node_type,
                // A missing node is the update's own error to report.
                None => return Ok(()),
            },
        };
        let Some(core) = crate::models::CoreNodeType::from_id(&node_type) else {
            return Ok(());
        };
        let typed = nodespace_types::typed_update_fields(core);
        if typed.is_empty() {
            return Ok(());
        }
        // The patch may be flat (`{"status": ..}`) or already bucketed under
        // the type (`{"task": {"status": ..}}`); both name the same fields.
        let bucket = patch.get(core.as_str()).and_then(|v| v.as_object());
        let named = patch
            .keys()
            .chain(bucket.into_iter().flat_map(|b| b.keys()));
        for key in named {
            if typed
                .iter()
                .any(|field| key == field.storage || key == field.wire)
            {
                return Err(NodeServiceError::invalid_update(format!(
                    "'{key}' is a field of the core type '{core}' and is written through the \
                     typed {core} update, not the generic node update"
                )));
            }
        }
        Ok(())
    }

    /// The checks every create owes a node before anything is written,
    /// whichever path creates it (single, with a parent, bulk, hierarchy
    /// import, an invariant action): its type can be instantiated, its id has
    /// a legal form, and it is not a hand-made core schema. Its behaviours
    /// validate it later, once its properties are in the buckets they will be
    /// stored in (see [`Self::rebucket_and_validate`]).
    pub(crate) async fn ensure_creatable(&self, node: &Node) -> Result<(), NodeServiceError> {
        self.ensure_creatable_in_batch(node, &mut std::collections::HashSet::new())
            .await
    }

    /// [`Self::ensure_creatable`] for one node of a batch. Whether a type can
    /// be instantiated is read once per distinct type and remembered in
    /// `instantiable_types`, so a large import does not repeat the read per
    /// row.
    pub(crate) async fn ensure_creatable_in_batch(
        &self,
        node: &Node,
        instantiable_types: &mut std::collections::HashSet<String>,
    ) -> Result<(), NodeServiceError> {
        if !instantiable_types.contains(&node.node_type) {
            self.ensure_instantiable(&node.node_type).await?;
            instantiable_types.insert(node.node_type.clone());
        }
        self.ensure_valid_node_id(node).await?;
        Self::ensure_not_creating_core_schema(node)
    }

    /// Refuse `node_type` as the type of a node when it is abstract
    /// (ADR-086 §6), or another build's subtype whose schema this database
    /// lacks (see [`Self::ensure_extension_type_has_schema`]). An abstract
    /// type is a real type — queryable, and a valid `extends` target — but
    /// only its subtypes are ever instantiated. Checked here, in the service,
    /// so it holds for every surface that creates or retypes a node.
    pub(crate) async fn ensure_instantiable(
        &self,
        node_type: &str,
    ) -> Result<(), NodeServiceError> {
        self.ensure_extension_type_has_schema(node_type).await?;
        let is_abstract = self
            .store
            .is_abstract_type(node_type)
            .await
            .map_err(NodeServiceError::from_store)?;
        if is_abstract {
            return Err(NodeServiceError::abstract_node_type(node_type));
        }
        Ok(())
    }

    /// Refuse a type another build registered a behaviour for while this
    /// database has no schema for it (ADR-082 §2.1). The schema is what makes
    /// the type a subtype: it places the type under the core type it extends.
    /// Without it the type's chain is the type alone, so a node of it would
    /// skip its base's rules and be missing from queries for its base. Checked
    /// on every create and retype, through [`Self::ensure_instantiable`]. A
    /// core type, or a type without such a behaviour, is not read at all.
    async fn ensure_extension_type_has_schema(
        &self,
        node_type: &str,
    ) -> Result<(), NodeServiceError> {
        if crate::models::CoreNodeType::from_id(node_type).is_some()
            || self.behaviors.get(node_type).is_none()
        {
            return Ok(());
        }
        let has_schema = self
            .store
            .get_schema(node_type)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_some();
        if has_schema {
            Ok(())
        } else {
            Err(NodeServiceError::unknown_node_type(node_type))
        }
    }

    /// Refuse a provided id that is not a UUID (ADR-086 §10). Three id forms
    /// are not UUIDs, and each belongs to one type: a `date` node's id is its
    /// date (`YYYY-MM-DD`), a `schema` node's id is the type name, and the
    /// settings singleton has a fixed id. Every other node, seeded or not, has
    /// a UUID.
    pub(crate) async fn ensure_valid_node_id(&self, node: &Node) -> Result<(), NodeServiceError> {
        if uuid::Uuid::parse_str(&node.id).is_ok() {
            return Ok(());
        }
        if crate::models::CoreNodeType::Schema.is_exactly(&node.node_type) && !node.id.is_empty() {
            return Ok(());
        }
        // `create_node` has already forced a date-shaped id to the `date` type.
        if crate::models::CoreNodeType::Date.is_exactly(&node.node_type)
            && is_date_node_id(&node.id)
        {
            return Ok(());
        }
        if node.id == DATABASE_SETTINGS_NODE_ID
            && self
                .type_is_a(
                    &node.node_type,
                    crate::models::CoreNodeType::DatabaseSettings,
                )
                .await?
        {
            return Ok(());
        }
        Err(NodeServiceError::invalid_update(format!(
            "Provided ID '{}' is not a valid UUID. Only a date node (YYYY-MM-DD), a schema \
             node (its type name) and the settings singleton ('{}') take a non-UUID id.",
            node.id, DATABASE_SETTINGS_NODE_ID
        )))
    }

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
            node.node_type = crate::models::CoreNodeType::Date.as_str().to_string();
            // Content is preserved - date nodes can have custom content like "Custom Date Content"
        }

        // DatabaseSettingsNode is a singleton (ADR-037). The fixed reserved ID makes
        // creation idempotent: if one already exists, treat a second create as a no-op
        // and return the existing id rather than erroring. Mirrors the collection-name
        // uniqueness guard in SqliteStore::create_node, but non-fatal.
        if self
            .type_is_a(
                &node.node_type,
                crate::models::CoreNodeType::DatabaseSettings,
            )
            .await?
        {
            if let Some(existing) = self
                .query_nodes_by_type(crate::models::CoreNodeType::DatabaseSettings.as_str(), true)
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
        let colliding_collection = if self
            .type_is_a(&node.node_type, crate::models::CoreNodeType::Collection)
            .await?
        {
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

        self.sync_created_mentions(&created_id, &node.content).await;

        tracing::debug!(
            node_id = %created_id,
            "create_node: COMPLETE at {}ms",
            start.elapsed().as_millis()
        );
        Ok(created_id)
    }

    /// Create the `mentions` edges of a node created with content, as an
    /// update to that content would (see [`Self::sync_mentions`]). A node
    /// written whole in one create (a chat message, a node the CLI or an
    /// agent creates) links what it mentions without waiting for an edit.
    ///
    /// Called by the single-node creates. A markdown import links its
    /// mentions in bulk, and a node a Play's action creates is linked when
    /// its content is next edited.
    ///
    /// Best-effort, after the create has committed: a mention that cannot be
    /// linked never fails or undoes the create.
    pub(super) async fn sync_created_mentions(&self, node_id: &str, content: &str) {
        if extract_mentions(content).is_empty() {
            return;
        }
        // Boxed: linking a mentioned date page creates it, which comes back
        // through `create_node`.
        if let Err(e) = Box::pin(self.sync_mentions(node_id, "", content)).await {
            tracing::warn!(
                "Failed to sync mentions for created node {}: {}",
                node_id,
                e
            );
        }
    }

    /// `_in_tx` twin of [`Self::create_node`] (ADR-069 §1b/S2). Delegates the
    /// validation/normalization/title/insert pipeline to
    /// [`Self::insert_node_in_tx_no_invariant_dispatch`] (the insert lands on
    /// `tx.store_tx()` instead of opening its own transaction, and the
    /// `NodeCreated` event is buffered on the transaction via
    /// `self.emit_event_in_tx` — see `NodeService::with_transaction` —
    /// instead of relying on the store notifier, since `create_node_in_tx` the store method deliberately does
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

    /// Refuse creating a core schema through the service, and creating any
    /// schema with context paths already on it.
    ///
    /// Core schemas are seeded straight into the store at startup; nothing
    /// else may mint one. Since whether a schema is core is fixed at creation
    /// (see [`Self::ensure_schema_core_status_unchanged`]), a user schema
    /// created with `isCore: true` could never be corrected or deleted.
    pub(crate) fn ensure_not_creating_core_schema(node: &Node) -> Result<(), NodeServiceError> {
        if crate::models::schema_node::is_core_schema(node) {
            return Err(NodeServiceError::invalid_update(format!(
                "schema_is_core: schema '{}' cannot be created as a core type; core schemas are built in",
                node.id
            )));
        }
        // A schema is created before its relationships are declared, so a
        // context path given here has nothing to be checked against.
        if crate::models::CoreNodeType::Schema.is_exactly(&node.node_type)
            && node
                .properties
                .get(crate::models::schema_node::CONTEXT_PATHS_KEY)
                .is_some()
        {
            return Err(NodeServiceError::invalid_update(format!(
                "Schema '{}' cannot be created with context paths: add them with update_schema \
                 (add_context_paths) once its relationships are declared, so each path is \
                 checked against the schemas.",
                node.id
            )));
        }
        Ok(())
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
            node.node_type = crate::models::CoreNodeType::Date.as_str().to_string();
        }

        self.ensure_creatable(&node).await?;
        // A type that needs a parent is refused here, before the row is
        // written: the row lands before its parent edge, so no database rule
        // can see that the edge never came (ADR-089). The refusal names no
        // id: the node was never created, so its id leads nowhere.
        if is_root {
            crate::db::SqliteStore::assert_may_be_root_in_tx(tx.store_tx(), &node.node_type, None)
                .await
                .map_err(NodeServiceError::from_store)?;
        }
        self.validate_templated_content(&node).await?;

        if !crate::models::CoreNodeType::Schema.is_exactly(&node.node_type) {
            node.properties =
                Self::normalize_flat_properties_to_namespace(&node.node_type, &node.properties);
        }
        // A create defaults the fields the caller left out.
        self.rebucket_and_validate(&mut node, true).await?;

        if node.title.is_none() {
            node.title = self.compute_title(&node, Some(is_root)).await?;
        }

        self.stamp_task_started(None, &mut node).await?;

        if self
            .type_is_a(&node.node_type, crate::models::CoreNodeType::Play)
            .await?
        {
            Self::ensure_play_created_unsuspended(&node)?;
            self.validate_play_rules(&node.properties, None).await?;
        }
        if self
            .type_is_a(&node.node_type, crate::models::CoreNodeType::Query)
            .await?
        {
            self.validate_query_paths(&node.properties).await?;
        }

        crate::db::SqliteStore::create_node_in_tx(tx.store_tx(), &node)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;

        self.emit_event_in_tx(
            tx,
            DomainEvent::NodeCreated {
                node_id: node.id.clone(),
                node_type: node.node_type.clone(),
            },
        );

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
        self.create_placed_node(params).await.map(|(id, _)| id)
    }

    /// [`Self::create_node_with_parent`], also returning the store's
    /// [`crate::db::ChildPlacement`] for the new node's parent edge (`None`
    /// for a root node).
    ///
    /// A caller that relays the create to a client returns the placement in
    /// its reply: echo suppression keeps this write's own relationship events
    /// from reaching the client that made it, so the reply is the only place
    /// that client learns the authoritative order keys (including any
    /// re-spread of the new node's siblings).
    pub async fn create_placed_node(
        &self,
        params: CreateNodeParams,
    ) -> Result<(String, Option<crate::db::ChildPlacement>), NodeServiceError> {
        let (node, parent, node_type) = self.prepare_create_node_with_parent(params).await?;
        let has_parent = parent.is_some();
        let content = node.content.clone();

        let (created_id, placement) = if let Some((parent_id, position)) = parent {
            let service = self.clone();
            let service_for_tx = service.clone();
            service
                .with_transaction(move |tx| {
                    Box::pin(async move {
                        let created_id = service_for_tx.create_node_in_tx(tx, node, false).await?;
                        let placement = service_for_tx
                            .create_parent_edge_in_tx(
                                tx,
                                &created_id,
                                &parent_id,
                                position.as_ref(),
                            )
                            .await?;
                        Ok((created_id, Some(placement)))
                    })
                })
                .await?
        } else {
            (self.create_node(node).await?, None)
        };

        self.queue_created_root_for_embedding(&created_id, &node_type, has_parent)
            .await;
        // A root went through `create_node`, which linked its mentions.
        if has_parent {
            self.sync_created_mentions(&created_id, &content).await;
        }

        Ok((created_id, placement))
    }

    /// `_in_tx` twin of [`Self::create_node_with_parent`] (ADR-069 §1b/S3),
    /// composable into a caller's own transaction — e.g. `handle_create_schema`
    /// composing the schema node create with its relationship declarations and
    /// description subtree. Shares the same validation/preparation pipeline;
    /// the node insert and parent edge (when there is a parent) land on the
    /// same `tx` the caller is already holding. Embedding-marker queueing is
    /// intentionally NOT reproduced here — it is derived state outside the
    /// boundary by design (ADR-069 §5). A caller creating an embedded root
    /// queues it after its own `with_transaction` commits, with
    /// [`Self::queue_created_root_for_embedding`], as `create_schema` does: a
    /// schema is found by meaning, so it has to be embedded.
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

        // Step 1: Reject a node_type that is neither a registered core type
        // nor an existing schema id — before any write, so a rejected call
        // leaves no auto-created date container behind.
        self.ensure_known_node_type(&params.node_type).await?;

        // Refuse a parent for a type that is always a root — before step 2,
        // so a rejected call leaves no auto-created date container behind. A
        // schema not yet given an id is named by the id it would get.
        if params.parent_id.is_some() {
            let node_id = params.id.clone().or_else(|| {
                crate::models::CoreNodeType::Schema
                    .is_exactly(&params.node_type)
                    .then(|| normalize_schema_id(&params.content))
            });
            self.store
                .assert_may_have_parent(&params.node_type, node_id.as_deref())
                .await
                .map_err(NodeServiceError::from_store)?;
        }

        // Step 2: Auto-create date container if parent is a date ID
        if let Some(ref parent_id) = params.parent_id {
            self.ensure_date_exists(parent_id).await?;
        }

        // Step 3: The parent exists, and both structural rules allow the new
        // node under it (ADR-089).
        if let Some(ref parent_id) = params.parent_id {
            let parent_node = self
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeServiceError::invalid_parent(parent_id.as_str()))?;
            self.store
                .assert_has_child_allowed(
                    crate::db::Placed::existing(parent_id, &parent_node.node_type),
                    crate::db::Placed {
                        id: params.id.as_deref(),
                        node_type: &params.node_type,
                    },
                )
                .await
                .map_err(NodeServiceError::from_store)?;
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
            // The id form is checked on the insert itself
            // (`ensure_valid_node_id`), which every create path reaches.
            provided_id
        } else if crate::models::CoreNodeType::Date.is_exactly(&params.node_type) {
            params.content.clone()
        } else if crate::models::CoreNodeType::Schema.is_exactly(&params.node_type) {
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
            let normalized_props = if !crate::models::CoreNodeType::Schema
                .is_exactly(&params.node_type)
            {
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

    /// Post-commit follow-up for [`Self::create_node_with_parent`] and for a
    /// caller of [`Self::create_node_with_parent_in_tx`]: queue the created
    /// node's aggregate root for embedding regeneration. Deliberately
    /// outside the transaction boundary (ADR-069 §5) — embedding markers are
    /// derived state with their own reconciliation loop, and a queueing
    /// failure must never fail or roll back a create that already committed.
    pub(crate) async fn queue_created_root_for_embedding(
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
            if self.is_embeddable_type(node_type).await {
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

        // Read before the patch is merged: afterwards a patch that sets
        // `enabled` to the value it already has can't be told from one that
        // never named it.
        let enables_play = Self::patch_enables_play(update.properties.as_ref());

        if let Some(properties) = update.properties {
            properties_changed = true;
            // Normalize flat client properties to namespaced format before merging
            // Skip for schema nodes - they use a special non-namespaced format
            if crate::models::CoreNodeType::Schema.is_exactly(&updated.node_type) {
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
        Self::ensure_schema_core_status_unchanged(&existing, &updated)?;
        Self::ensure_schema_structure_unchanged(&existing, &updated)?;
        self.ensure_retype_allowed(None, &existing, &updated)
            .await?;

        // Step 2: Behavior and schema validation, on the properties as they
        // will be stored. On a type change, the new type's fields are
        // defaulted first. Either way the properties are re-bucketed before
        // anything validates them: an update naming an inherited field
        // arrives flat, normalizes into the node's OWN bucket, and would sit
        // there duplicating the authoritative value in the declaring
        // ancestor's bucket, where that ancestor's behaviour never reads it.
        self.rebucket_and_validate(&mut updated, node_type_changed)
            .await?;

        self.settle_play_update(&existing, &mut updated, enables_play)
            .await?;
        self.stamp_task_started(Some(&existing), &mut updated)
            .await?;

        // Sync title when content, node_type, or properties change
        // Schema-driven title_template — also trigger on properties_changed
        if content_changed || node_type_changed {
            self.validate_templated_content(&updated).await?;
        }
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
            .map_err(NodeServiceError::from_store)?;

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
    /// self-heals); every current caller (`rename_schema_field`,
    /// `update_schema_field_friendly_name`, `handle_update_schema`) updates a
    /// schema node's `fields` JSON, whose content mention sync is a no-op in
    /// practice, and a future caller updating real content
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
        // Optional schema-definition keys this write clears.
        let mut cleared_keys: Vec<&'static str> = Vec::new();

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

        // Read before the patch is merged: afterwards a patch that sets
        // `enabled` to the value it already has can't be told from one that
        // never named it.
        let enables_play = Self::patch_enables_play(update.properties.as_ref());

        if let Some(properties) = update.properties {
            properties_changed = true;
            if crate::models::CoreNodeType::Schema.is_exactly(&updated.node_type) {
                cleared_keys = Self::merge_schema_definition(&mut updated.properties, properties);
            } else {
                let normalized_properties =
                    Self::normalize_flat_properties_to_namespace(&updated.node_type, &properties);
                Self::deep_merge_namespaced_properties(
                    &mut updated.properties,
                    normalized_properties,
                );
            }
        }

        Self::ensure_schema_core_status_unchanged(&existing, &updated)?;
        self.ensure_retype_allowed(Some(tx), &existing, &updated)
            .await?;
        // Chain-resolved, per ADR-078 — see `rebucket_and_validate`.
        self.rebucket_and_validate(&mut updated, node_type_changed)
            .await?;

        self.settle_play_update(&existing, &mut updated, enables_play)
            .await?;
        self.stamp_task_started(Some(&existing), &mut updated)
            .await?;

        if content_changed || node_type_changed {
            self.validate_templated_content(&updated).await?;
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
            .map_err(NodeServiceError::from_store)?;
        // The store's update only adds and replaces keys, so the ones this
        // write clears are removed from the row here.
        crate::db::SqliteStore::remove_property_keys_in_tx(tx.store_tx(), id, &cleared_keys)
            .await
            .map_err(NodeServiceError::from_store)?;

        // Reflects the store's unconditional version bump — see
        // `SqliteStore::update_node_in_tx` (mirrors `update_node`'s own
        // "exactly one version bump per call" statement).
        updated.version += 1;

        self.emit_event_in_tx(
            tx,
            DomainEvent::NodeUpdated {
                node_id: id.to_string(),
                node_type: updated.node_type.clone(),
                node: updated,
                changed_properties: vec![],
            },
        );

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
    /// until commit. `update_node_unchecked_in_tx` is left as-is for its
    /// callers (the schema-definition writers, which always target an
    /// already-committed schema node) rather than changed underneath them.
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
            .map_err(NodeServiceError::from_store)?
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

        // Read before the patch is merged: afterwards a patch that sets
        // `enabled` to the value it already has can't be told from one that
        // never named it.
        let enables_play = Self::patch_enables_play(update.properties.as_ref());

        if let Some(properties) = update.properties {
            properties_changed = true;
            if crate::models::CoreNodeType::Schema.is_exactly(&updated.node_type) {
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

        Self::ensure_schema_core_status_unchanged(&existing, &updated)?;
        Self::ensure_schema_structure_unchanged(&existing, &updated)?;
        self.ensure_retype_allowed(Some(tx), &existing, &updated)
            .await?;
        // Chain-resolved, per ADR-078 — see `rebucket_and_validate`.
        self.rebucket_and_validate(&mut updated, node_type_changed)
            .await?;

        self.settle_play_update(&existing, &mut updated, enables_play)
            .await?;
        self.stamp_task_started(Some(&existing), &mut updated)
            .await?;

        if content_changed || node_type_changed {
            self.validate_templated_content(&updated).await?;
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
        .map_err(NodeServiceError::from_store)?;

        let node = result.map_err(|actual_version| {
            NodeServiceError::version_conflict(id, existing.version, actual_version)
        })?;

        self.emit_event_in_tx(
            tx,
            DomainEvent::NodeUpdated {
                node_id: node.id.clone(),
                node_type: node.node_type.clone(),
                node: node.clone(),
                changed_properties: vec![],
            },
        );

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
                if updated_content != previous.content
                    && self
                        .type_is_a(&updated_node_type, crate::models::CoreNodeType::Collection)
                        .await?
                {
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

        // Queue root for embedding regeneration if content changed (root-aggregate model),
        // or if the node was archived or unarchived: an unarchived node is
        // embedded again, and an archived child leaves its root's aggregate
        // (ADR-087 §2). An archived root's own vectors went with the write.
        // Fire-and-forget: don't block the update response on embedding queue operations
        #[cfg(feature = "nlp")]
        let participation_changed = previous.as_ref().is_some_and(|previous| {
            crate::governance::participation_changed(previous, &updated_node)
        });
        #[cfg(feature = "nlp")]
        if content_changed || participation_changed {
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
            .map_err(NodeServiceError::from_store)?
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

        // Read before the patch is merged: afterwards a patch that sets
        // `enabled` to the value it already has can't be told from one that
        // never named it.
        let enables_play = Self::patch_enables_play(update.properties.as_ref());

        if let Some(properties) = update.properties {
            properties_changed = true;
            // Normalize flat client properties to namespaced format before merging
            // Skip for schema nodes - they use a special non-namespaced format
            if crate::models::CoreNodeType::Schema.is_exactly(&updated.node_type) {
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
        Self::ensure_schema_core_status_unchanged(&existing, &updated)?;
        Self::ensure_schema_structure_unchanged(&existing, &updated)?;
        self.ensure_retype_allowed(Some(tx), &existing, &updated)
            .await?;

        // Step 2: Behavior and schema validation (USER-EXTENSIBLE)
        // Every type that declares a schema is validated, user-defined types
        // included. Non-tx reads like the chain lookup behind it are safe
        // from inside a write transaction — same precedent as
        // `insert_node_in_tx_no_invariant_dispatch`'s own
        // schema/title/play-validation calls. Re-bucketed before validating,
        // same reasoning as the update paths above: an inherited field
        // arrives flat and must be moved to its declaring ancestor's bucket,
        // or it exists in two places. No defaulting here — this path does not
        // change the node's type.
        self.rebucket_and_validate(&mut updated, false).await?;

        let play_rules_changed = self
            .settle_play_update(&existing, &mut updated, enables_play)
            .await?;
        self.stamp_task_started(Some(&existing), &mut updated)
            .await?;

        // Synchronous play validation gate — reject invalid rule changes
        // before persist. Only a write that changes the rules runs it: a
        // write of the switch, the description or an extension field says
        // nothing about the rules, so it succeeds for a play whose rules a
        // schema change has since broken (ADR-087 §5).
        if play_rules_changed {
            self.validate_play_rules(&updated.properties, Some(&existing.properties))
                .await?;
        }
        // The same gate for a saved query's relationship paths.
        if properties_changed
            && self
                .type_is_a(&updated.node_type, crate::models::CoreNodeType::Query)
                .await?
        {
            self.validate_query_paths(&updated.properties).await?;
        }

        // Sync title when content, node_type, or properties change
        // Schema-driven title_template — also trigger on properties_changed
        if content_changed || node_type_changed {
            self.validate_templated_content(&updated).await?;
        }
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
        .map_err(NodeServiceError::from_store)?;

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

        // Buffered on `tx`, not broadcast yet — only
        // flushed if this whole transaction commits. See this method's own
        // doc for why emitting before dispatch below is safe.
        self.emit_event_in_tx(
            tx,
            DomainEvent::NodeUpdated {
                node_id: updated_node.id.clone(),
                node_type: updated_node.node_type.clone(),
                node: updated_node.clone(),
                changed_properties: changed_properties.clone(),
            },
        );

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
            .map_err(NodeServiceError::from_store)?
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
                    .map_err(NodeServiceError::from_store)?
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
            .map_err(NodeServiceError::from_store)?;

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
    /// dedicated refusal modal). With core's default gate (`AlwaysAllowGate`) there is never a
    /// refusal here; only an injected gate can deny. This check runs before the transaction
    /// opens, not inside it — a rollback-based check would do wasted work every time.
    ///
    /// Returns `DeleteResult` with `existed=true` and `deleted_count` (target + all descendants)
    /// on success, or `existed=false` when the target node was already gone.
    pub async fn delete_node(
        &self,
        node_id: &str,
        expected_version: i64,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        // Capture the embedding root before deletion for the embedding queue.
        let root_id_for_embedding = self.get_embedding_root_id(node_id).await.ok();

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
            .map_err(NodeServiceError::from_store)??;

        if !existed {
            return Ok(crate::models::DeleteResult {
                existed: false,
                deleted_count: 0,
            });
        }

        // Queue the embedding root for regeneration when a node inside it was deleted.
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
            .map_err(NodeServiceError::from_store)?;

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
        .map_err(NodeServiceError::from_store)?;

        let updated_node = result.map_err(|actual_version| {
            NodeServiceError::version_conflict(node_id, expected_version, actual_version)
        })?;

        self.emit_event_in_tx(
            tx,
            DomainEvent::NodeUpdated {
                node_id: updated_node.id.clone(),
                node_type: updated_node.node_type.clone(),
                node: updated_node.clone(),
                changed_properties: vec![],
            },
        );

        Ok(updated_node)
    }

    // =========================================================================
    // Private CRUD helpers
    // =========================================================================

    /// Reject a node_type that is neither a registered core type nor an
    /// existing schema id. Without this, an invented id (a display name, a
    /// paraphrase) falls through to CustomNodeBehavior and the node is stored
    /// as a bare shell: no schema means nothing to validate supplied
    /// properties against, so every one of them is silently dropped and the
    /// caller is told the write succeeded.
    ///
    /// A behaviour another build registered does not make its type known: a
    /// subtype is defined by its schema, which places it under the core type
    /// it extends. Until that schema exists in this database, a node of the
    /// type would be neither validated as its base nor found by a query for
    /// it.
    async fn ensure_known_node_type(&self, node_type: &str) -> Result<(), NodeServiceError> {
        if crate::models::CoreNodeType::from_id(node_type).is_some() {
            return Ok(());
        }
        let schema_exists = self
            .store
            .get_schema(node_type)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_some();
        if schema_exists {
            Ok(())
        } else {
            Err(NodeServiceError::unknown_node_type(node_type))
        }
    }

    /// Whether a properties patch sets a play's `enabled` to `true`, in
    /// either the flat or the bucketed (`{"play": {..}}`) shape.
    pub(crate) fn patch_enables_play(patch: Option<&serde_json::Value>) -> bool {
        let Some(patch) = patch else {
            return false;
        };
        let enabled = patch
            .get(crate::models::PLAY_NODE_TYPE)
            .and_then(|bucket| bucket.get(nodespace_types::PLAY_ENABLED_FIELD))
            .or_else(|| patch.get(nodespace_types::PLAY_ENABLED_FIELD));
        enabled == Some(&serde_json::Value::Bool(true))
    }

    /// Settle what an update means for a play's suspension, and report
    /// whether it changes the play's rules (ADR-087 §5). Every update
    /// pipeline calls this once the merged properties are re-bucketed, so a
    /// type extending `play` is read in the bucket its fields live in. A node
    /// that is not a play is left alone.
    ///
    /// - The suspension fields are the engine's: an update that would change
    ///   one is refused. One that carries a field's stored value back
    ///   unchanged is not a change.
    /// - Setting `enabled` to `true` (even when it already is) or saving
    ///   different rules clears a suspension. The engine then re-validates
    ///   the play and suspends it again if the problem remains.
    pub(crate) async fn settle_play_update(
        &self,
        existing: &Node,
        updated: &mut Node,
        enables_play: bool,
    ) -> Result<bool, NodeServiceError> {
        use crate::models::PlayFields;
        use nodespace_types::{PLAY_RULES_FIELD, PLAY_SUSPENSION_FIELDS};

        if !self
            .type_is_a(&updated.node_type, crate::models::CoreNodeType::Play)
            .await?
        {
            return Ok(false);
        }

        let rules_changed = PlayFields::stored_field(&existing.properties, PLAY_RULES_FIELD)
            != PlayFields::stored_field(&updated.properties, PLAY_RULES_FIELD);
        let clears =
            (rules_changed || enables_play) && PlayFields::suspended_in(&existing.properties);

        for field in PLAY_SUSPENSION_FIELDS {
            let before = PlayFields::stored_field(&existing.properties, field);
            let after = PlayFields::stored_field(&updated.properties, field);
            // A write that clears the suspension anyway may also carry the
            // fields as cleared: it says the same thing twice.
            if before != after && !(clears && after.is_none()) {
                return Err(NodeServiceError::invalid_update(format!(
                    "'{field}' is recorded by the play engine and can't be written. Set \
                     'enabled' to true to clear a suspension"
                )));
            }
        }

        if clears {
            // A cleared field is stored as `null`, like any other clear.
            if let Some(bucket) = updated
                .properties
                .get_mut(crate::models::PLAY_NODE_TYPE)
                .and_then(|b| b.as_object_mut())
            {
                for field in PLAY_SUSPENSION_FIELDS {
                    bucket.insert(field.to_string(), serde_json::Value::Null);
                }
            }
        }
        Ok(rules_changed)
    }

    /// Record when work on a task began: a write that moves a task into a
    /// started status (`in_progress`, or `in_review`, which is work already
    /// done) sets `started_at` to today when the task has none (ADR-092 §5).
    /// Every write pipeline that creates or updates a node calls this once
    /// the properties are re-bucketed. `existing` is `None` on a create.
    ///
    /// Only the first start is recorded: a task that already has a
    /// `started_at`, or whose write sets one, keeps it. A node that is not a
    /// task is left alone.
    ///
    /// The fields are read in the `task` bucket, the one storage shape a
    /// task's fields have. A batch create handed properties outside it stores
    /// them as given, and such a node has no status here to read.
    pub(crate) async fn stamp_task_started(
        &self,
        existing: Option<&Node>,
        updated: &mut Node,
    ) -> Result<(), NodeServiceError> {
        use crate::models::{CoreNodeType, TaskStatus};
        const STATUS: &str = "status";
        const STARTED_AT: &str = "started_at";

        if !self
            .type_is_a(&updated.node_type, CoreNodeType::Task)
            .await?
        {
            return Ok(());
        }
        let bucket = CoreNodeType::Task.as_str();
        let stored = |node: &Node, field: &str| -> Option<serde_json::Value> {
            node.properties
                .get(bucket)
                .and_then(|b| b.get(field))
                .filter(|v| !v.is_null())
                .cloned()
        };
        let status_of = |node: &Node| {
            stored(node, STATUS)
                .as_ref()
                .and_then(|v| v.as_str())
                .map(TaskStatus::from_value)
        };

        let Some(status) = status_of(updated) else {
            return Ok(());
        };
        let moved = existing.and_then(status_of).as_ref() != Some(&status);
        if !status.is_started() || !moved || stored(updated, STARTED_AT).is_some() {
            return Ok(());
        }
        if let Some(fields) = updated
            .properties
            .get_mut(bucket)
            .and_then(|b| b.as_object_mut())
        {
            let today = chrono::Local::now().date_naive().format("%Y-%m-%d");
            fields.insert(STARTED_AT.to_string(), serde_json::json!(today.to_string()));
        }
        Ok(())
    }

    /// A new play carries no suspension: only the engine records one.
    pub(crate) fn ensure_play_created_unsuspended(node: &Node) -> Result<(), NodeServiceError> {
        match nodespace_types::PLAY_SUSPENSION_FIELDS
            .into_iter()
            .find(|field| {
                crate::models::PlayFields::stored_field(&node.properties, field).is_some()
            }) {
            Some(field) => Err(NodeServiceError::invalid_update(format!(
                "'{field}' is recorded by the play engine and can't be set on a new play"
            ))),
            None => Ok(()),
        }
    }

    /// Validate play rules before persisting.
    ///
    /// `stored` is the play's properties before a write that changes its
    /// rules, and `None` for a new play. The rules' descriptions are checked
    /// against it (ADR-090 §1): a component whose content changed must not
    /// keep the description it was stored with.
    pub(crate) async fn validate_play_rules(
        &self,
        properties: &serde_json::Value,
        stored: Option<&serde_json::Value>,
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

        // Step 2: Check the descriptions, against the stored rules when
        // there are any that decode. Restoring a seeded play's shipped rules
        // is never stale: they were described when they shipped, whatever
        // the play holds now.
        let stored_rules = stored
            .filter(|stored| !Self::restores_shipped_rules(properties, stored))
            .and_then(|stored| parse_rules_from_properties(stored).ok());
        if let Err(errors) =
            crate::playbook::descriptions::check_descriptions(&rule_defs, stored_rules.as_deref())
        {
            return Err(NodeServiceError::PlayValidationFailed {
                errors: errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }

        // Step 3: Parse each rule definition into a ParsedRule
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

        // Step 4: Run the full validation pipeline (schema checks, CEL compile, paths)
        if let Err(errors) = crate::playbook::validation::validate_play(&parsed_rules, self).await {
            return Err(NodeServiceError::play_validation_failed(&errors));
        }

        Ok(())
    }

    /// Whether a write sets a seeded play's rules to the ones it shipped with
    /// (`_seed.default_rules`, ADR-060 §8).
    ///
    /// `_seed` is an ordinary property a client can write, so this relaxes
    /// the stale-description comparison and nothing else: the rules still
    /// decode, carry descriptions that are not blank, and validate in full.
    fn restores_shipped_rules(properties: &serde_json::Value, stored: &serde_json::Value) -> bool {
        let shipped = stored
            .get("_seed")
            .and_then(|seed| seed.get("default_rules"));
        shipped.is_some()
            && shipped
                == crate::models::PlayFields::stored_field(
                    properties,
                    nodespace_types::PLAY_RULES_FIELD,
                )
    }

    /// Resolve a saved query's relationship paths, and check its paths into
    /// object fields' values, before persisting.
    ///
    /// A path names relationships or fields as an author writes them, and
    /// what a name means depends on the schemas. Resolving it here means a
    /// name that resolves to nothing is an error when the query is saved,
    /// rather than a query that quietly matches nothing each time it runs.
    /// The resolved form is not stored: a query's paths are resolved again
    /// whenever it runs, so they always follow the current schemas.
    pub(crate) async fn validate_query_paths(
        &self,
        properties: &serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        let fields = crate::models::QueryFields::from_properties(properties)
            .map_err(|e| NodeServiceError::invalid_update(e.to_string()))?;
        let to_service_error = |e: crate::ops::OpsError| match e {
            crate::ops::OpsError::InvalidParams(message) => {
                NodeServiceError::invalid_update(message)
            }
            other => NodeServiceError::query_failed(other.to_string()),
        };
        // The check every run makes: a name that could not be formatted into
        // a statement is refused here, not stored to fail each time it runs.
        crate::services::QueryDefinition::from_fields(&fields)
            .validate_identifiers()
            .map_err(|e| NodeServiceError::invalid_update(e.to_string()))?;
        crate::ops::query_ops::resolve_filters(self, &fields.target_type, fields.filters)
            .await
            .map_err(to_service_error)?;
        crate::ops::query_ops::resolve_sorting(
            self,
            &fields.target_type,
            fields.sorting.unwrap_or_default(),
        )
        .await
        .map_err(to_service_error)?;
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

    /// The schema properties that are stored only when they say something:
    /// `abstract` when true, a structural rule when it is not `any`, the
    /// context paths when there are any.
    pub(crate) const OPTIONAL_SCHEMA_DEFINITION_KEYS: [&'static str; 4] = [
        "abstract",
        "children",
        "parent",
        crate::models::schema_node::CONTEXT_PATHS_KEY,
    ];

    /// Merge a schema-definition write into a schema node's stored
    /// properties.
    ///
    /// The definition writer owns [`Self::OPTIONAL_SCHEMA_DEFINITION_KEYS`]:
    /// a value replaces the stored one whole, and `null` removes the key.
    /// That is how a rule goes back to `any` and a type stops being
    /// abstract; a plain merge could only ever add them, and would leave a
    /// replaced rule carrying its old `types`. Every other key merges as
    /// usual. Returns the keys the write removed from the stored ones.
    pub(crate) fn merge_schema_definition(
        existing: &mut serde_json::Value,
        mut new: serde_json::Value,
    ) -> Vec<&'static str> {
        let mut cleared = Vec::new();
        if let (Some(stored), Some(written)) = (existing.as_object_mut(), new.as_object_mut()) {
            for key in Self::OPTIONAL_SCHEMA_DEFINITION_KEYS {
                if let Some(value) = written.remove(key) {
                    if value.is_null() {
                        if stored.remove(key).is_some() {
                            cleared.push(key);
                        }
                    } else {
                        stored.insert(key.to_string(), value);
                    }
                }
            }
        }
        Self::deep_merge_namespaced_properties(existing, new);
        cleared
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
    /// node before persisting it (ADR-078), creates and updates alike.
    ///
    /// Extracted because the same lines appeared at several call sites, and
    /// **one of them was originally missed** — the wrong-bucket bug this
    /// sequence exists to prevent was itself caused by the duplication. A
    /// new write path gets it right by calling this rather than by copying it
    /// correctly.
    ///
    /// Five orderings are load-bearing and are the reason this is one
    /// function rather than separate calls at each site:
    ///
    /// 0. **Un-nesting first.** A field the caller named in its declaring
    ///    bucket arrives nested in the node's own
    ///    ([`Self::unnest_ancestor_buckets`]). Every later step reads a field
    ///    from the bucket it is stored in: a default fills only a field with
    ///    no value there, and a behaviour reads its own type's bucket.
    /// 1. **Defaults before bucketing.** `apply_schema_defaults_with_fields`
    ///    puts defaults in the node's *own* bucket; bucketing then moves any
    ///    inherited one into its declaring ancestor's. Reversing them strands
    ///    an inherited default in the wrong bucket.
    /// 2. **Bucketing before behaviours.** A behaviour reads its fields from
    ///    its own type's bucket. A caller names an inherited field flat, which
    ///    normalizes into the *node's* bucket; until it is moved, the
    ///    ancestor's behaviour sees the previously stored value, or none, and
    ///    the new one would be stored unchecked. A subtype would then relax
    ///    its ancestor's rule (ADR-086), and since behaviours validate the
    ///    whole node on every write, the node would refuse every later update.
    /// 3. **Bucketing before schema validation.** Validation merges the
    ///    buckets in the chain; a field sitting in two of them resolves by map
    ///    ordering.
    /// 4. **The empty-fields branch.** With no resolved fields there is
    ///    nothing to default, bucket by or validate against, but the
    ///    behaviours and the closed-core-bucket check still run.
    ///
    /// `apply_defaults` is true on a create and on a node-type change, where
    /// the node may be missing fields its type declares. An update that
    /// leaves the type alone must not re-default: the node already has its
    /// values, and defaulting again would resurrect a field the caller
    /// deliberately cleared.
    ///
    /// A schema node takes this path too. Its definition is flat, not
    /// bucketed, so only its behaviour runs.
    pub(crate) async fn rebucket_and_validate(
        &self,
        node: &mut Node,
        apply_defaults: bool,
    ) -> Result<(), NodeServiceError> {
        let ownership = self.field_ownership_for_write(&node.node_type).await?;
        self.rebucket_and_validate_with(node, &ownership, apply_defaults)
    }

    /// What [`Self::rebucket_and_validate`] resolves for a node of
    /// `node_type`: the whole `extends` chain, not just the type's own
    /// schema, since an extending type defaults and validates its ancestors'
    /// fields too and needs the ownership map to re-bucket them. A batch
    /// resolves it once per distinct type.
    pub(crate) async fn field_ownership_for_write(
        &self,
        node_type: &str,
    ) -> Result<FieldOwnershipInfo, NodeServiceError> {
        if crate::models::CoreNodeType::Schema.is_exactly(node_type) {
            // A schema's definition is not bucketed: no field of it is
            // defaulted or moved.
            return Ok((
                Vec::new(),
                std::collections::HashMap::new(),
                self.type_chain(node_type).await?,
            ));
        }
        self.resolve_field_owners(node_type).await
    }

    /// [`Self::field_ownership_for_write`] for one row of a batch: resolved
    /// on the first row of each type and kept in `resolved`.
    pub(crate) async fn field_ownership_in_batch<'a>(
        &self,
        node_type: &str,
        resolved: &'a mut std::collections::HashMap<String, FieldOwnershipInfo>,
    ) -> Result<&'a FieldOwnershipInfo, NodeServiceError> {
        if !resolved.contains_key(node_type) {
            let ownership = self.field_ownership_for_write(node_type).await?;
            resolved.insert(node_type.to_string(), ownership);
        }
        Ok(&resolved[node_type])
    }

    /// [`Self::rebucket_and_validate`] with the type's ownership already
    /// resolved.
    pub(crate) fn rebucket_and_validate_with(
        &self,
        node: &mut Node,
        ownership: &FieldOwnershipInfo,
        apply_defaults: bool,
    ) -> Result<(), NodeServiceError> {
        self.rebucket_and_validate_behaviors(node, ownership, apply_defaults)?;
        if crate::models::CoreNodeType::Schema.is_exactly(&node.node_type) {
            return Ok(());
        }
        let (fields, owners, chain) = ownership;
        Self::reject_derived_attribute_keys(node, chain)?;
        // A type that declares no fields has nothing to validate against,
        // but a core one is still closed: its bucket takes no undeclared key.
        Self::reject_undeclared_core_keys(node, owners, chain)?;
        if !fields.is_empty() {
            self.validate_node_with_fields(node, fields, Some(chain))?;
        }
        Ok(())
    }

    /// The first half of [`Self::rebucket_and_validate`]: refuse properties
    /// that are not bucketed, un-nest, default, re-bucket, then run every
    /// behaviour in the node's type chain over the result, base first. A
    /// subtype is validated as the type it extends, plus its own rules.
    /// Called on its own only by the trusted import, which skips schema
    /// validation.
    pub(crate) fn rebucket_and_validate_behaviors(
        &self,
        node: &mut Node,
        ownership: &FieldOwnershipInfo,
        apply_defaults: bool,
    ) -> Result<(), NodeServiceError> {
        let (fields, owners, chain) = ownership;
        if !crate::models::CoreNodeType::Schema.is_exactly(&node.node_type) {
            Self::ensure_properties_are_buckets(node, chain)?;
        }
        node.properties =
            Self::unnest_ancestor_buckets(&node.node_type, &node.properties, owners, chain);
        if !fields.is_empty() {
            if apply_defaults {
                self.apply_schema_defaults_with_fields(node, fields, Some(chain))?;
            }
            node.properties =
                Self::bucket_properties_by_owner(&node.node_type, &node.properties, owners);
        }
        self.behaviors.validate_node(node, chain)?;
        Ok(())
    }

    /// Refuse properties the bucketing steps cannot place: properties that
    /// are not an object, and a value that is not an object under the name of
    /// a type in the node's chain, where that type's bucket is stored.
    ///
    /// Every step after this one skips what it cannot read as a bucket, so a
    /// malformed value would otherwise be dropped, or replaced by defaults,
    /// with no error.
    fn ensure_properties_are_buckets(
        node: &Node,
        chain: &[String],
    ) -> Result<(), NodeServiceError> {
        let Some(buckets) = node.properties.as_object() else {
            return Err(NodeServiceError::invalid_update(
                "Properties must be a JSON object",
            ));
        };
        for scope in chain {
            if buckets.get(scope.as_str()).is_some_and(|v| !v.is_object()) {
                return Err(NodeServiceError::invalid_update(format!(
                    "'{scope}' in properties holds the fields of the type '{scope}' and must be \
                     a JSON object"
                )));
            }
        }
        Ok(())
    }

    /// Refuse a property named after a derived attribute of the node's type
    /// (ADR-094 §5).
    ///
    /// A derived attribute is computed from `content` and never stored, and
    /// every reader computes it ahead of any property: a stored value of the
    /// same name would be unreadable, and a write naming one is a caller that
    /// expects it to change the attribute. The name is refused in every
    /// bucket of the chain, a user subtype's open one included.
    pub(crate) fn reject_derived_attribute_keys(
        node: &Node,
        chain: &[String],
    ) -> Result<(), NodeServiceError> {
        let Some(core_type) = crate::models::CoreNodeType::nearest(chain) else {
            return Ok(());
        };
        let derived = core_type.derived_attributes();
        let Some(properties) = node.properties.as_object() else {
            return Ok(());
        };
        if derived.is_empty() {
            return Ok(());
        }
        let buckets = chain
            .iter()
            .filter_map(|scope| properties.get(scope.as_str()).and_then(|v| v.as_object()));
        for bucket in std::iter::once(properties).chain(buckets) {
            if let Some(attribute) = derived.iter().find(|a| bucket.contains_key(a.name())) {
                return Err(NodeServiceError::invalid_update(format!(
                    "'{}' is derived from the content of a '{core_type}' node and cannot be \
                     written. Change the node's content to change it.",
                    attribute.name()
                )));
            }
        }
        Ok(())
    }

    /// Refuse an undeclared key in a core type's bucket (ADR-086 §7).
    ///
    /// A core type's schema is closed: a key in its bucket must be declared by
    /// that type's schema, be a namespaced extension field (`custom:`, `org:`,
    /// `plugin:`, ADR-063), or start with `_` (bookkeeping). A subtype's own
    /// bucket follows its own schema's rules, and a user-defined type stays
    /// open, so only the core scopes of the chain are checked.
    ///
    /// `owners` maps each declared field to the schema that declares it, so a
    /// key is declared for a core bucket exactly when that core type owns it.
    pub(crate) fn reject_undeclared_core_keys(
        node: &Node,
        owners: &std::collections::HashMap<String, String>,
        chain: &[String],
    ) -> Result<(), NodeServiceError> {
        let Some(buckets) = node.properties.as_object() else {
            return Ok(());
        };
        for scope in chain {
            if crate::models::CoreNodeType::from_id(scope).is_none() {
                continue;
            }
            let Some(bucket) = buckets.get(scope.as_str()).and_then(|v| v.as_object()) else {
                continue;
            };
            for key in bucket.keys() {
                let declared = owners.get(key).is_some_and(|owner| owner == scope);
                if declared || key.starts_with('_') || is_extension_field_name(key) {
                    continue;
                }
                return Err(NodeServiceError::invalid_update(format!(
                    "'{key}' is not a field of the core type '{scope}'. A core type takes only \
                     its declared fields; to add your own, use a namespace prefix \
                     (e.g. 'custom:{key}')."
                )));
            }
        }
        Ok(())
    }

    /// Lift an ancestor's bucket out of the node's own bucket, where flat
    /// normalization put it.
    ///
    /// A caller may name a field in its declaring bucket
    /// (`{"task": {"status": …}}` on an issue). Flat normalization reads no
    /// schema, so it files that bucket under the node's own type like any
    /// other key: `{"issue": {"task": {"status": …}}}`. An own-bucket key is
    /// such a bucket, not a field, when it is the name of a type the node
    /// extends, holds an object, and no schema in the chain declares a field
    /// of that name.
    ///
    /// Each entry of it goes to the bucket of the schema that declares it,
    /// the same rule as [`Self::bucket_properties_by_owner`], or to the
    /// bucket the caller named when no schema does. An entry addressed this
    /// way wins over the same inherited field given flat in the same write.
    ///
    /// Runs before defaults are applied: a default fills only a field with
    /// no value, and a value still nested here would read as none.
    pub(crate) fn unnest_ancestor_buckets(
        node_type: &str,
        properties: &serde_json::Value,
        owners: &std::collections::HashMap<String, String>,
        chain: &[String],
    ) -> serde_json::Value {
        let Some(own_bucket) = properties.get(node_type).and_then(|v| v.as_object()) else {
            return properties.clone();
        };
        let is_ancestor_bucket = |key: &str, value: &serde_json::Value| {
            key != node_type
                && value.is_object()
                && !owners.contains_key(key)
                && chain.iter().any(|scope| scope == key)
        };
        if !own_bucket
            .iter()
            .any(|(key, value)| is_ancestor_bucket(key, value))
        {
            return properties.clone();
        }

        let mut out = properties.as_object().cloned().unwrap_or_default();
        let mut own = serde_json::Map::new();
        let mut addressed = Vec::new();
        for (key, value) in own_bucket {
            match value.as_object() {
                Some(entries) if is_ancestor_bucket(key, value) => {
                    addressed.push((key, entries));
                }
                _ => {
                    own.insert(key.clone(), value.clone());
                }
            }
        }

        for (named, entries) in addressed {
            for (field, value) in entries {
                let owner = owners.get(field);
                let target = owner.map_or(named.as_str(), String::as_str);
                if target == node_type {
                    own.insert(field.clone(), value.clone());
                    continue;
                }
                if owner.is_some() {
                    // The same inherited field given flat: the addressed
                    // value is the one kept.
                    own.remove(field);
                }
                let bucket = out
                    .entry(target.to_string())
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(bucket) = bucket.as_object_mut() {
                    bucket.insert(field.clone(), value.clone());
                }
            }
        }

        out.insert(node_type.to_string(), serde_json::Value::Object(own));
        serde_json::Value::Object(out)
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

            if let Some(value) = field_value {
                Self::check_field_value(field, value).map_err(NodeServiceError::invalid_update)?;
            }
        }

        Ok(())
    }

    /// Check one value against one field declaration: enum membership, the
    /// structural `object`/`array` shape, and the scalar types. Null
    /// always passes — it clears a field. Returns the rejection message.
    ///
    /// The single definition of "this value satisfies this declaration":
    /// [`Self::validate_node_with_fields`] runs it on every write, and
    /// `update_schema` runs it against existing instance values before
    /// re-declaring a field, so a schema change can never leave a node
    /// holding a value its next write would be rejected for.
    pub(crate) fn check_field_value(
        field: &crate::models::SchemaField,
        value: &serde_json::Value,
    ) -> Result<(), String> {
        use crate::models::SchemaFieldType;

        if value.is_null() {
            return Ok(());
        }

        if field.field_type == SchemaFieldType::Enum {
            let Some(value_str) = value.as_str() else {
                return Err(format!(
                    "Enum field '{}' must be a string or null",
                    field.name
                ));
            };
            if !Self::is_enum_value(field, value_str) {
                return Err(format!(
                    "Invalid value '{}' for enum field '{}'. Valid values: {}",
                    value_str,
                    field.name,
                    Self::enum_value_labels(field)
                ));
            }
        }

        // Structured fields are validated by shape: a field declared `object`
        // must hold a JSON object, and a field declared `array` must hold a
        // JSON array. Where the array declares an `item_type`, every element
        // must satisfy it: a JSON object for `object`, a JSON array for
        // `array`, one of the field's declared values for `enum`, and the
        // scalar rule below for `number`, `boolean`, `date` and `datetime`.
        // A `text` element, like a `text` field's value, is not type-checked.
        //
        // Nested declarations are validated recursively (ADR-086 §7): where an
        // `object` field declares `fields`, or an array of objects declares
        // `item_fields`, each nested value gets the same type, enum and
        // required checks as a top-level one, at every depth. An object with no
        // nested declaration is an open leaf and is not walked into.
        if field.field_type == SchemaFieldType::Object {
            let Some(object) = value.as_object() else {
                return Err(format!(
                    "Field '{}' is declared as type 'object' but received {}",
                    field.name,
                    crate::schema::json_type_name(value)
                ));
            };
            if let Some(nested) = field.fields.as_deref() {
                Self::check_nested_fields(&field.name, nested, object)?;
            }
        }
        if field.field_type == SchemaFieldType::Array {
            let Some(items) = value.as_array() else {
                return Err(format!(
                    "Field '{}' is declared as type 'array' but received {}",
                    field.name,
                    Self::describe_received(value)
                ));
            };
            match field.item_type {
                Some(SchemaFieldType::Object) => {
                    if let Some((index, item)) =
                        items.iter().enumerate().find(|(_, i)| !i.is_object())
                    {
                        return Err(format!(
                            "Field '{}' is declared as type 'array' with item type 'object', but \
                             item {} is {}",
                            field.name,
                            index,
                            Self::describe_received(item)
                        ));
                    }
                    if let Some(nested) = field.item_fields.as_deref() {
                        for (index, item) in items.iter().enumerate() {
                            if let Some(object) = item.as_object() {
                                Self::check_nested_fields(
                                    &format!("{}[{}]", field.name, index),
                                    nested,
                                    object,
                                )?;
                            }
                        }
                    }
                }
                Some(SchemaFieldType::Array) => {
                    if let Some((index, item)) =
                        items.iter().enumerate().find(|(_, i)| !i.is_array())
                    {
                        return Err(format!(
                            "Field '{}' is declared as type 'array' with item type 'array', but \
                             item {} is {}",
                            field.name,
                            index,
                            Self::describe_received(item)
                        ));
                    }
                }
                // The array field itself declares the values its elements
                // may take.
                Some(SchemaFieldType::Enum) => {
                    if let Some((index, item)) = items
                        .iter()
                        .enumerate()
                        .find(|(_, i)| !i.as_str().is_some_and(|s| Self::is_enum_value(field, s)))
                    {
                        return Err(format!(
                            "Field '{}' is declared as type 'array' with item type 'enum', but \
                             item {} is {}. Valid values: {}",
                            field.name,
                            index,
                            Self::describe_received(item),
                            Self::enum_value_labels(field)
                        ));
                    }
                }
                // Each link in a list is checked as a link field's value is.
                Some(SchemaFieldType::Link) => {
                    for (index, item) in items.iter().enumerate() {
                        crate::models::LinkValue::from_json(item).map_err(|problem| {
                            format!("Link field '{}' item {} {}", field.name, index, problem)
                        })?;
                    }
                }
                // A null element is not a cleared field, so it is checked
                // like any other value.
                Some(item_type) => {
                    for (index, item) in items.iter().enumerate() {
                        if let Some(expected) = Self::scalar_mismatch(item_type, item) {
                            return Err(format!(
                                "Field '{}' is declared as type 'array' with item type '{}'{}, \
                                 but item {} is {}",
                                field.name,
                                item_type,
                                expected,
                                index,
                                Self::describe_received(item)
                            ));
                        }
                    }
                }
                None => {}
            }
        }

        // A `link` holds a title and an absolute URL.
        if field.field_type == SchemaFieldType::Link {
            crate::models::LinkValue::from_json(value)
                .map_err(|problem| format!("Link field '{}' {}", field.name, problem))?;
        }

        if let Some(expected) = Self::scalar_mismatch(field.field_type, value) {
            return Err(format!(
                "Field '{}' is declared as type '{}'{} but received {}",
                field.name,
                field.field_type,
                expected,
                Self::describe_received(value)
            ));
        }

        Ok(())
    }

    /// The values `field` declares for an enum: its core values, then the
    /// user-added ones.
    fn enum_values(
        field: &crate::models::SchemaField,
    ) -> impl Iterator<Item = &crate::models::schema::EnumValue> {
        field
            .core_values
            .iter()
            .flatten()
            .chain(field.user_values.iter().flatten())
    }

    fn is_enum_value(field: &crate::models::SchemaField, value: &str) -> bool {
        Self::enum_values(field).any(|ev| ev.value == value)
    }

    /// The declared enum values as a rejection message lists them.
    fn enum_value_labels(field: &crate::models::SchemaField) -> String {
        Self::enum_values(field)
            .map(|ev| format!("{} ({})", ev.label, ev.value))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether `value` fails the scalar rule for `field_type`, as the
    /// description of what the type expects (empty where the type name says
    /// it all). `number` holds a JSON number, `boolean` a JSON bool, `date`
    /// an ISO-8601 date or RFC 3339 date-time string, and `datetime` an
    /// RFC 3339 date-time string. Sorting, `gt`/`lt` query filters and the CEL
    /// date functions all trust the declared type, so a value that doesn't
    /// match it is rejected on write rather than misread later.
    ///
    /// Enum, object, array and link values have their own checks in
    /// [`Self::check_field_value`], as a field's value and as an array's
    /// element, and a text value is not type-checked, so those never mismatch
    /// here.
    pub(crate) fn scalar_mismatch(
        field_type: crate::models::SchemaFieldType,
        value: &serde_json::Value,
    ) -> Option<&'static str> {
        use crate::models::SchemaFieldType;

        let (matches, expected) = match field_type {
            SchemaFieldType::Number => (value.is_number(), ""),
            SchemaFieldType::Boolean => (value.is_boolean(), ""),
            SchemaFieldType::Date => (
                value
                    .as_str()
                    .is_some_and(crate::schema::is_iso_date_or_datetime),
                " (a YYYY-MM-DD date or RFC 3339 date-time string)",
            ),
            SchemaFieldType::Datetime => (
                value
                    .as_str()
                    .is_some_and(crate::schema::is_rfc3339_datetime),
                " (an RFC 3339 date-time string)",
            ),
            SchemaFieldType::Text
            | SchemaFieldType::Enum
            | SchemaFieldType::Array
            | SchemaFieldType::Object
            | SchemaFieldType::Link => return None,
        };
        (!matches).then_some(expected)
    }

    /// A rejected value as a rejection message names it: a string is quoted,
    /// anything else is named by its JSON type.
    pub(crate) fn describe_received(value: &serde_json::Value) -> String {
        match value.as_str() {
            Some(s) => format!("the string '{}'", s),
            None => crate::schema::json_type_name(value).to_string(),
        }
    }

    /// Check an object value against the nested fields its declaration lists
    /// (`fields` of an object, `item_fields` of an array of objects): a
    /// required nested field must be present, and every present one must
    /// satisfy its declaration, recursively. `path` names the enclosing value
    /// in a rejection, so a nested failure reads `messages[2].role`.
    fn check_nested_fields(
        path: &str,
        nested: &[crate::models::SchemaField],
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        for nested_field in nested {
            match object.get(&nested_field.name) {
                Some(nested_value) => Self::check_field_value(nested_field, nested_value)
                    .map_err(|e| format!("in '{}': {}", path, e))?,
                None => {
                    if nested_field.required.unwrap_or(false) && nested_field.default.is_none() {
                        return Err(format!(
                            "Required field '{}' is missing from '{}'",
                            nested_field.name, path
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The one title rule, shared by every write path (single-node create and
    /// update, moves, bulk hierarchy inserts).
    ///
    /// A type with a `titleTemplate` takes the interpolated template. Otherwise
    /// the title is the node's content for a root and for a `task` or
    /// `collection` at any depth (each is a named thing wherever it sits), and
    /// none for any other child — a child line has no meaning outside its root.
    /// No type is special-cased beyond that: a `date` page or `schema` is a root
    /// titled by its content (`2026-09-23`, `Task`), like any other root.
    ///
    /// The templated branch resolves fields/properties across the node
    /// type's `extends` chain (ADR-078) via [`Self::resolve_field_owners`],
    /// mirroring `node_to_cel_value_at_scope`'s chain-aware projection: a
    /// `titleTemplate` declared on a subtype schema can reference a field an
    /// ancestor schema declares (and this node's bucket therefore doesn't
    /// hold) without interpolating blank.
    ///
    /// `chain_fields` lets a caller looping over many nodes of the same type
    /// (`with_titles`) supply an already-resolved `(fields, chain)` pair and
    /// skip a redundant `resolve_field_owners` call — and its DB reads — per
    /// row, the same cache-per-type shape [`Self::title_schema`]'s callers
    /// already use for the schema lookup. `None` resolves it here instead,
    /// which every caller but `with_titles` takes.
    pub(crate) async fn derive_title(
        &self,
        node: &Node,
        is_root: bool,
        schema: Option<&crate::models::SchemaNode>,
        chain_fields: Option<&(Vec<crate::models::SchemaField>, Vec<String>)>,
    ) -> Result<Option<String>, NodeServiceError> {
        if let Some(schema) = schema {
            if let Some(template) = &schema.title_template {
                let resolved;
                let (fields, chain) = match chain_fields {
                    Some((fields, chain)) => (fields, chain),
                    None => {
                        let (fields, _owners, chain) =
                            self.resolve_field_owners(&node.node_type).await?;
                        resolved = (fields, chain);
                        (&resolved.0, &resolved.1)
                    }
                };
                let flat_props = Self::merge_properties_across_chain(&node.properties, chain);
                return Ok(Some(crate::utils::interpolate_title_template_with_schema(
                    template,
                    &flat_props,
                    fields,
                )));
            }
        }
        if is_root || self.is_always_titled(&node.node_type).await? {
            Ok(Some(crate::utils::strip_markdown(&node.content)))
        } else {
            Ok(None)
        }
    }

    /// Whether a type is titled by its content at any depth, not only as a
    /// root: the registry's always-titled rule (a task, a collection), which
    /// every type extending one inherits.
    pub(crate) async fn is_always_titled(&self, node_type: &str) -> Result<bool, NodeServiceError> {
        Ok(self
            .core_type_of(node_type)
            .await?
            .is_some_and(|core| core.always_titled()))
    }

    /// A type with a `titleTemplate` takes its name from the template's fields,
    /// so its `content` must be empty — content is not the name. Keyed on the
    /// template rather than on type names, so a user type that declares one is
    /// held to it exactly as `person` is.
    ///
    /// A write-validation rule, run next to the other validation on every
    /// create, and on an update only when content or type changes — so a move
    /// or a field-only edit never trips it, even on a node whose type gained a
    /// template after the node was written.
    pub(crate) fn reject_content_on_templated_type(
        node: &Node,
        schema: Option<&crate::models::SchemaNode>,
    ) -> Result<(), NodeServiceError> {
        let Some(template) = schema.and_then(|s| s.title_template.as_ref()) else {
            return Ok(());
        };
        if node.content.is_empty() {
            return Ok(());
        }
        let fields = crate::utils::title_template_fields(template);
        let source = if fields.is_empty() {
            "its title template".to_string()
        } else {
            fields.join("/")
        };
        Err(NodeServiceError::ValidationFailed(
            crate::models::ValidationError::InvalidProperties(format!(
                "{} takes its name from {}; content is not allowed",
                node.node_type, source
            )),
        ))
    }

    /// [`Self::reject_content_on_templated_type`] with the node type's schema
    /// looked up, for the single-node write paths.
    pub(crate) async fn validate_templated_content(
        &self,
        node: &Node,
    ) -> Result<(), NodeServiceError> {
        let schema = self.title_schema(&node.node_type).await;
        Self::reject_content_on_templated_type(node, schema.as_ref())
    }

    /// Merge a node's per-scope property buckets across an `extends` chain
    /// (ADR-078) into one flat map, nearest scope first — the same
    /// nearest-scope-wins merge [`Self::validate_node_with_fields`] uses to
    /// read fields across a chain, specialized for title-template
    /// interpolation's flat-map input. `_`-prefixed bookkeeping keys
    /// (`_seed`, `_schemaVersion`, ...) are excluded, matching
    /// `validate_node_with_fields` and `node_to_cel_value_at_scope`'s own
    /// chain merges.
    fn merge_properties_across_chain(
        properties: &serde_json::Value,
        chain: &[String],
    ) -> serde_json::Value {
        let Some(obj) = properties.as_object() else {
            return properties.clone();
        };
        let mut merged = serde_json::Map::new();
        for scope in chain {
            let Some(bucket) = obj.get(scope.as_str()).and_then(|v| v.as_object()) else {
                continue;
            };
            for (field, value) in bucket {
                if field.starts_with('_') {
                    continue;
                }
                merged.entry(field.clone()).or_insert_with(|| value.clone());
            }
        }
        serde_json::Value::Object(merged)
    }

    /// The schema [`Self::derive_title`] reads for `node_type`. A failed lookup
    /// falls back to no schema (the content rule) rather than blocking the
    /// write.
    pub(crate) async fn title_schema(&self, node_type: &str) -> Option<crate::models::SchemaNode> {
        match self.nearest_title_schema(node_type).await {
            Ok(schema) => schema,
            Err(e) => {
                tracing::warn!(
                    node_type = %node_type,
                    error = %e,
                    "title schema lookup failed, falling back to content-based title"
                );
                None
            }
        }
    }

    /// The schema a type's title comes from: the nearest one in its `extends`
    /// chain that declares a `titleTemplate`, so a subtype is titled as the
    /// type it extends (ADR-086 §5). A chain with no template yields the
    /// type's own schema, and the content rule applies.
    async fn nearest_title_schema(
        &self,
        node_type: &str,
    ) -> Result<Option<crate::models::SchemaNode>, NodeServiceError> {
        let own = self.get_schema_node(node_type).await?;
        if own.as_ref().is_some_and(|s| s.title_template.is_some()) {
            return Ok(own);
        }
        for ancestor in self.type_chain(node_type).await?.iter().skip(1) {
            if let Some(schema) = self.get_schema_node(ancestor).await? {
                if schema.title_template.is_some() {
                    return Ok(Some(schema));
                }
            }
        }
        Ok(own)
    }

    /// Compute the indexed title for a node — [`Self::derive_title`] with the
    /// node's schema, looking up rootness only when the caller doesn't supply it
    /// and the rule depends on it.
    pub(crate) async fn compute_title(
        &self,
        node: &Node,
        is_root: Option<bool>,
    ) -> Result<Option<String>, NodeServiceError> {
        let schema = self.title_schema(&node.node_type).await;
        let rootness_matters = schema
            .as_ref()
            .and_then(|s| s.title_template.as_ref())
            .is_none()
            && !self.is_always_titled(&node.node_type).await?;
        let is_root = match is_root {
            Some(v) => v,
            None if rootness_matters => self
                .store
                .get_parent_id(&node.id)
                .await
                .map_err(NodeServiceError::from_store)?
                .is_none(),
            None => false,
        };
        self.derive_title(node, is_root, schema.as_ref(), None)
            .await
    }

    /// Bring `node_id`'s derived state in line after an edge write set whether
    /// it is a root. Every path that gains or drops a `has_child` edge calls
    /// this, so "only roots are results" holds for both search indexes:
    ///
    /// - **Title.** A title follows rootness for every type without a template
    ///   (a root's is its content, a child's is none). Without the refresh an
    ///   indented root keeps its title and title search returns it as a
    ///   document, and an outdented child stays unfindable by name.
    /// - **Embedding.** See [`Self::refresh_embedding_for_rootness`].
    ///
    /// `former_parent` is the `has_child` parent the write took away: `None`
    /// when the node had no parent, or kept it. A path that detaches or
    /// reparents a node must pass it, or the tree the node left keeps the
    /// node's text in its embedding.
    ///
    /// Best-effort: it runs after the edge write has committed, so a failure is
    /// logged rather than returned — a derived index must not turn a committed
    /// move into a reported failure that skips the caller's version bump and
    /// events.
    pub(crate) async fn refresh_for_rootness(
        &self,
        node_id: &str,
        is_root: bool,
        former_parent: Option<&str>,
    ) {
        if let Err(e) = self.try_refresh_title_for_rootness(node_id, is_root).await {
            tracing::warn!(
                node_id = %node_id,
                error = %e,
                "failed to refresh title after a rootness change"
            );
        }
        self.refresh_embedding_for_rootness(node_id, is_root, former_parent)
            .await;
    }

    /// The embedding half of [`Self::refresh_for_rootness`]. The tx paths run
    /// it after commit (see [`Self::refresh_for_rootness_in_tx`]).
    ///
    /// Only an embedding root carries an embedding: a tree root, or a
    /// descendant re-rooted at an access boundary (ADR-059 §7), which keeps its
    /// own. Any other node that becomes a child drops its own, or vector search
    /// would still return it bare. Its (new) embedding root is queued either
    /// way: a new root needs its first embedding, and a tree that gained a
    /// child needs its aggregate rebuilt. The tree `former_parent` belongs to
    /// lost the node's subtree, so its embedding root is queued too. Otherwise
    /// its vector keeps ranking for text it no longer holds, including text
    /// since filed behind an access boundary.
    pub(crate) async fn refresh_embedding_for_rootness(
        &self,
        node_id: &str,
        is_root: bool,
        former_parent: Option<&str>,
    ) {
        if let Err(e) = self.try_drop_child_embedding(node_id, is_root).await {
            tracing::warn!(
                node_id = %node_id,
                error = %e,
                "failed to drop a child's embedding after a rootness change"
            );
        }
        #[cfg(feature = "nlp")]
        {
            self.queue_root_for_embedding(node_id).await;
            if let Some(former_parent) = former_parent {
                self.queue_former_embedding_root(node_id, former_parent)
                    .await;
            }
        }
    }

    async fn try_refresh_title_for_rootness(
        &self,
        node_id: &str,
        is_root: bool,
    ) -> anyhow::Result<()> {
        let Some(node) = self.store.get_node(node_id).await? else {
            return Ok(());
        };
        let title = self.compute_title(&node, Some(is_root)).await?;
        if title != node.title {
            self.store
                .set_title(node_id, title.as_deref(), node.version)
                .await?;
        }
        Ok(())
    }

    async fn try_drop_child_embedding(&self, node_id: &str, is_root: bool) -> anyhow::Result<()> {
        // A child carries no embedding, unless it is an access-boundary
        // descendant re-rooted by ADR-059 §7.
        if !is_root
            && self.store.has_embeddings(node_id).await?
            && self.store.embedding_root_id(node_id).await? != node_id
        {
            self.store.delete_embeddings(node_id).await?;
        }
        Ok(())
    }

    /// `_in_tx` twin of [`Self::refresh_for_rootness`]. The title is written
    /// inside the transaction. The embedding store has no transaction-scoped
    /// writers, so the embedding half is recorded on `tx`, and
    /// [`Self::with_transaction`] runs it once the transaction commits.
    pub(crate) async fn refresh_for_rootness_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        is_root: bool,
        former_parent: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        tx.defer_embedding_refresh(node_id, is_root, former_parent);
        let Some(node) = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), node_id)
            .await
            .map_err(NodeServiceError::from_store)?
        else {
            return Ok(());
        };
        let title = self.compute_title(&node, Some(is_root)).await?;
        if title != node.title {
            crate::db::SqliteStore::set_title_in_tx(tx.store_tx(), node_id, title.as_deref())
                .await
                .map_err(NodeServiceError::from_store)?;
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
