//! Pending seed updates for NodeService (ADR-094 §8): the shipped changes
//! reconciliation held back because the user had edited that aspect, and the
//! two choices that settle one. See [`NodeService::seed_nodes_from_templates`]
//! for where they are recorded.
//!
//! A core schema's context paths are a seeded aspect too
//! ([`SeedAspect::ContextPaths`]). A schema is not built from a template, so
//! that aspect has its own reconciliation
//! ([`NodeService::reconcile_core_context_paths`]) and its own compare and
//! take; listing, reading and keeping are the same calls as for any seed.

use super::*;
use crate::markdown::PreparedNode;
use crate::models::schema_node::{is_core_schema, CONTEXT_PATHS_KEY};
use crate::models::seed_update::{PendingSeedUpdate, PendingSeedUpdateRow};
use crate::models::SchemaNode;
use nodespace_types::RelationshipPath;

/// A pending update with both versions of the aspect, as text a person can
/// read side by side: Markdown for guidance, the name and fields for config,
/// one path per line for context paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedUpdateComparison {
    pub update: PendingSeedUpdate,
    /// The version that ships now.
    pub shipped: String,
    /// The version in this database.
    pub yours: String,
}

impl NodeService {
    /// Every pending seed update, oldest first, each with what names its node.
    pub async fn list_pending_seed_updates(
        &self,
    ) -> Result<Vec<PendingSeedUpdate>, NodeServiceError> {
        let rows = self
            .store
            .list_pending_seed_updates()
            .await
            .map_err(NodeServiceError::from_store)?;

        let mut updates = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(update) = self.describe_pending_seed_update(row).await? {
                updates.push(update);
            }
        }
        Ok(updates)
    }

    /// The pending update for `aspect` of the seeded node `node_id`, if there
    /// is one.
    pub async fn get_pending_seed_update(
        &self,
        node_id: &str,
        aspect: SeedAspect,
    ) -> Result<Option<PendingSeedUpdate>, NodeServiceError> {
        let row = self
            .store
            .get_pending_seed_update(node_id, aspect)
            .await
            .map_err(NodeServiceError::from_store)?;
        match row {
            Some(row) => self.describe_pending_seed_update(row).await,
            None => Ok(None),
        }
    }

    /// The pending update for `aspect` of the seed `template_group` expands,
    /// with the shipped version beside the one in this database. `None` when
    /// nothing is pending for it.
    ///
    /// Takes the seed's prepared template for the reason
    /// [`Self::reset_seed_node`] does: the seed tables live in the crates
    /// that own them, and the caller resolves the node to its template.
    pub async fn compare_pending_seed_update(
        &self,
        template_group: &[PreparedNode],
        aspect: SeedAspect,
    ) -> Result<Option<SeedUpdateComparison>, NodeServiceError> {
        let Some(template_root) = template_group.first() else {
            return Ok(None);
        };
        // A template ships no context paths: those are a core schema's, and
        // are compared by `compare_pending_context_paths_update`.
        if aspect == SeedAspect::ContextPaths {
            return Ok(None);
        }
        let Some(update) = self
            .get_pending_seed_update(&template_root.id, aspect)
            .await?
        else {
            return Ok(None);
        };

        let yours = match aspect {
            SeedAspect::ContextPaths => String::new(),
            SeedAspect::Config => {
                let node = self
                    .store
                    .get_node(&update.node_id)
                    .await
                    .map_err(NodeServiceError::from_store)?
                    .ok_or_else(|| NodeServiceError::node_not_found(&update.node_id))?;
                let fields = node
                    .properties
                    .get(node.node_type.as_str())
                    .filter(|bucket| bucket.is_object())
                    .unwrap_or(&node.properties);
                config_text(&node.content, fields)
            }
            SeedAspect::Guidance => {
                let (_, node_map, adjacency_list) = self.get_subtree_data(&update.node_id).await?;
                render_subtree_markdown(&update.node_id, &node_map, &adjacency_list)
            }
        };

        Ok(Some(SeedUpdateComparison {
            update,
            shipped: shipped_seed_aspect_text(template_group, aspect),
            yours,
        }))
    }

    /// Keep the user's version of `aspect`: the shipped version on record is
    /// marked as seen, so the aspect is no longer pending and stays that way
    /// across restarts until what ships changes again. The user's content and
    /// the aspect's modified flag are untouched. Returns `false` when nothing
    /// was pending.
    pub async fn keep_seed_update(
        &self,
        node_id: &str,
        aspect: SeedAspect,
    ) -> Result<bool, NodeServiceError> {
        let Some(row) = self
            .store
            .get_pending_seed_update(node_id, aspect)
            .await
            .map_err(NodeServiceError::from_store)?
        else {
            return Ok(false);
        };

        // Same best-effort, OCC-bypassing stamp reconciliation uses for
        // `_seed` bookkeeping: it carries no content, and writing the same
        // fingerprint twice changes nothing.
        self.store
            .set_property_string(
                node_id,
                &format!("$._seed.{}", aspect.version_key()),
                &row.shipped_version,
            )
            .await
            .map_err(NodeServiceError::from_store)?;
        self.settle_pending_seed_update(node_id, aspect, true)
            .await?;
        Ok(true)
    }

    /// Take the shipped version of `aspect` for the seed `template_group`
    /// expands: it replaces the user's, and the aspect's modified flag and
    /// its pending record are cleared. Returns `false`, changing nothing,
    /// when nothing was pending: this is a choice about a pending update, and
    /// [`Self::reset_seed_node`] is the path that discards an edit
    /// unconditionally.
    pub async fn take_seed_update(
        &self,
        template_group: &[PreparedNode],
        aspect: SeedAspect,
    ) -> Result<bool, NodeServiceError> {
        let Some(template_root) = template_group.first() else {
            return Ok(false);
        };
        // Taken by `take_context_paths_update`: a template ships none.
        if aspect == SeedAspect::ContextPaths {
            return Ok(false);
        }
        if self
            .store
            .get_pending_seed_update(&template_root.id, aspect)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_none()
        {
            return Ok(false);
        }

        self.reset_seed_node(
            template_group,
            aspect == SeedAspect::Config,
            aspect == SeedAspect::Guidance,
        )
        .await?;
        Ok(true)
    }

    /// Bring every core schema's context paths up to date with the ones
    /// `shipped` declares, by the rule every other seeded aspect follows
    /// (ADR-072, [`Self::seed_nodes_from_templates`]):
    ///
    /// | state                                                 | action                       |
    /// |-------------------------------------------------------|------------------------------|
    /// | schema absent                                         | nothing (seeding creates it) |
    /// | fingerprint matches                                   | skip                         |
    /// | fingerprint differs, `context_paths_modified` not set | replace the paths            |
    /// | fingerprint differs, `context_paths_modified` set     | keep, record as pending      |
    ///
    /// The fingerprint ([`context_paths_version`]) of the shipped list a
    /// schema's paths were last brought up to date with, or kept against, is
    /// `_seed.context_paths_version` on the schema node. The flag is set by
    /// `update_schema` when a call adds or removes a path. Nothing else of a
    /// core schema is reconciled: its fields and rules are this build's or
    /// the database is refused (`db::core_type_shape`).
    ///
    /// `shipped` is [`crate::models::core_schemas::get_core_schemas`] on
    /// every open.
    pub async fn reconcile_core_context_paths(
        &self,
        shipped: &[SchemaNode],
    ) -> Result<(), NodeServiceError> {
        let ids: Vec<String> = shipped.iter().map(|s| s.envelope.id.clone()).collect();
        let stored = self
            .store
            .get_nodes_by_ids(&ids)
            .await
            .map_err(NodeServiceError::from_store)?;
        let pending: HashSet<String> = self
            .store
            .list_pending_seed_updates()
            .await
            .map_err(NodeServiceError::from_store)?
            .into_iter()
            .filter(|row| row.aspect == SeedAspect::ContextPaths)
            .map(|row| row.node_id)
            .collect();

        for schema in shipped {
            let schema_id = schema.envelope.id.as_str();
            let Some(node) = stored.get(schema_id).filter(|node| is_core_schema(node)) else {
                continue;
            };
            let seed = node.properties.get("_seed");
            let stored_version = seed
                .and_then(|s| s.get(SeedAspect::ContextPaths.version_key()))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let modified = seed
                .and_then(|s| s.get(SeedAspect::ContextPaths.modified_key()))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let version = context_paths_version(&schema.context_paths);
            let was_pending = pending.contains(schema_id);

            if stored_version == version {
                self.settle_pending_seed_update(schema_id, SeedAspect::ContextPaths, was_pending)
                    .await?;
            } else if modified {
                tracing::info!(
                    schema_id,
                    "Shipped context paths changed but the schema's were user-modified; kept, \
                     recorded as pending"
                );
                self.store
                    .record_pending_seed_update(schema_id, SeedAspect::ContextPaths, &version)
                    .await
                    .map_err(NodeServiceError::from_store)?;
            } else {
                self.write_shipped_context_paths(schema_id, &schema.context_paths, false)
                    .await?;
                self.settle_pending_seed_update(schema_id, SeedAspect::ContextPaths, was_pending)
                    .await?;
            }
        }
        Ok(())
    }

    /// The pending update to the context paths of the core schema `shipped`
    /// is this build's definition of, with the shipped paths beside the ones
    /// in this database, one per line. `None` when nothing is pending for it.
    ///
    /// Takes the shipped schema, as [`Self::compare_pending_seed_update`]
    /// takes a seed's template; [`shipped_core_schema`] resolves an id to it.
    pub async fn compare_pending_context_paths_update(
        &self,
        shipped: &SchemaNode,
    ) -> Result<Option<SeedUpdateComparison>, NodeServiceError> {
        let schema_id = &shipped.envelope.id;
        let Some(update) = self
            .get_pending_seed_update(schema_id, SeedAspect::ContextPaths)
            .await?
        else {
            return Ok(None);
        };
        let yours = self
            .get_schema_node(schema_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(schema_id))?
            .context_paths;

        Ok(Some(SeedUpdateComparison {
            update,
            shipped: context_paths_text(&shipped.context_paths),
            yours: context_paths_text(&yours),
        }))
    }

    /// Take the shipped context paths of the core schema `shipped` is this
    /// build's definition of: they replace the user's, and the aspect's
    /// modified flag and its pending record are cleared. Returns `false`,
    /// changing nothing, when nothing was pending.
    pub async fn take_context_paths_update(
        &self,
        shipped: &SchemaNode,
    ) -> Result<bool, NodeServiceError> {
        let schema_id = &shipped.envelope.id;
        if self
            .store
            .get_pending_seed_update(schema_id, SeedAspect::ContextPaths)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_none()
        {
            return Ok(false);
        }
        self.write_shipped_context_paths(schema_id, &shipped.context_paths, true)
            .await?;
        self.settle_pending_seed_update(schema_id, SeedAspect::ContextPaths, true)
            .await?;
        Ok(true)
    }

    /// Store `paths` as the schema's context paths and stamp their
    /// fingerprint, in one write. `clear_modified` also clears the modified
    /// flag: the paths are the shipped ones again, by the user's choice.
    ///
    /// The schema-definition write `update_schema` uses, so no paths removes
    /// the stored key rather than leaving the old list. Not `update_schema`
    /// itself: that marks the paths as the user's.
    async fn write_shipped_context_paths(
        &self,
        schema_id: &str,
        paths: &[RelationshipPath],
        clear_modified: bool,
    ) -> Result<(), NodeServiceError> {
        let mut seed = serde_json::json!({
            SeedAspect::ContextPaths.version_key(): context_paths_version(paths),
        });
        if clear_modified {
            seed[SeedAspect::ContextPaths.modified_key()] = Value::Bool(false);
        }
        let stored_paths = if paths.is_empty() {
            Value::Null
        } else {
            serde_json::json!(paths)
        };
        let properties = serde_json::json!({ CONTEXT_PATHS_KEY: stored_paths, "_seed": seed });
        let service = self.clone();
        let schema_id = schema_id.to_string();
        self.with_transaction(move |tx| {
            Box::pin(async move {
                let update = NodeUpdate {
                    properties: Some(properties),
                    ..Default::default()
                };
                service
                    .update_node_unchecked_in_tx(tx, &schema_id, update)
                    .await
            })
        })
        .await
    }

    /// Clear the pending record for an aspect that is current with what
    /// ships. `pending` is whether a record is known to exist, so the common
    /// case of nothing pending costs no write.
    pub(super) async fn settle_pending_seed_update(
        &self,
        node_id: &str,
        aspect: SeedAspect,
        pending: bool,
    ) -> Result<(), NodeServiceError> {
        if pending {
            self.store
                .clear_pending_seed_update(node_id, aspect)
                .await
                .map_err(NodeServiceError::from_store)?;
        }
        Ok(())
    }

    /// A pending row with its node's type, title and last edit. `None` when
    /// the node is gone.
    async fn describe_pending_seed_update(
        &self,
        row: PendingSeedUpdateRow,
    ) -> Result<Option<PendingSeedUpdate>, NodeServiceError> {
        // By id, so an archived seed is described too: archiving turns a
        // seed off, and its pending update is still the user's to settle.
        let Some(node) = self
            .store
            .get_node(&row.node_id)
            .await
            .map_err(NodeServiceError::from_store)?
        else {
            return Ok(None);
        };

        let last_edited_at = match row.aspect {
            SeedAspect::Config | SeedAspect::ContextPaths => node.modified_at,
            SeedAspect::Guidance => {
                let (_, node_map, _) = self.get_subtree_data(&row.node_id).await?;
                node_map
                    .values()
                    .filter(|child| child.id != row.node_id)
                    .map(|child| child.modified_at)
                    .max()
                    .unwrap_or(node.modified_at)
            }
        };

        Ok(Some(PendingSeedUpdate {
            node_id: row.node_id,
            node_type: node.node_type,
            title: node.content,
            aspect: row.aspect,
            shipped_version: row.shipped_version,
            recorded_at: row.recorded_at,
            last_edited_at,
        }))
    }
}

/// The shipped version of `aspect` of the seed `template_group` expands, as
/// the text [`NodeService::compare_pending_seed_update`] shows beside the
/// user's. Guidance is rendered by the renderer that renders the stored body,
/// so the two differ only where their content does.
pub fn shipped_seed_aspect_text(template_group: &[PreparedNode], aspect: SeedAspect) -> String {
    let Some(root) = template_group.first() else {
        return String::new();
    };
    match aspect {
        // A template ships no context paths.
        SeedAspect::ContextPaths => String::new(),
        SeedAspect::Config => config_text(&root.content, &root.properties),
        SeedAspect::Guidance => {
            let mut children: Vec<&PreparedNode> = template_group[1..].iter().collect();
            children.sort_by(|a, b| a.order.total_cmp(&b.order));

            let mut node_map = HashMap::new();
            let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
            for child in children {
                if let Some(parent_id) = &child.parent_id {
                    adjacency_list
                        .entry(parent_id.clone())
                        .or_default()
                        .push(child.id.clone());
                }
                node_map.insert(
                    child.id.clone(),
                    Node::new_with_id(
                        child.id.clone(),
                        child.node_type.clone(),
                        child.content.clone(),
                        child.properties.clone(),
                    ),
                );
            }
            render_subtree_markdown(&root.id, &node_map, &adjacency_list)
        }
    }
}

/// The fingerprint of a shipped list of context paths: what
/// `_seed.context_paths_version` holds on a core schema, and what a pending
/// update to them records. Order is part of it, since a context read follows
/// the paths in the order declared.
pub fn context_paths_version(paths: &[RelationshipPath]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for path in paths {
        hasher.update(path.to_string().as_bytes());
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

/// This build's definition of the core schema `schema_id`, whose context
/// paths are the shipped ones.
pub fn shipped_core_schema(schema_id: &str) -> Option<SchemaNode> {
    crate::models::core_schemas::get_core_schemas()
        .into_iter()
        .find(|schema| schema.envelope.id == schema_id)
}

/// Context paths as text: one dotted path per line, in the order declared.
fn context_paths_text(paths: &[RelationshipPath]) -> String {
    paths
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A seed's config aspect as text: its name, then its fields. Bookkeeping
/// keys (`_seed`) and cleared fields are left out; neither is something the
/// user wrote or the seed ships.
fn config_text(name: &str, fields: &Value) -> String {
    let fields: serde_json::Map<String, Value> = fields
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, value)| !key.starts_with('_') && !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let fields = serde_json::to_string_pretty(&Value::Object(fields)).unwrap_or_default();
    format!("name: {name}\n{fields}")
}
