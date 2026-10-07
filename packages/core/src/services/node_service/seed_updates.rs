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
//!
//! A built-in skill's `attached_to` links are a seeded aspect too
//! ([`SeedAspect::Links`]), reconciled by [`NodeService::reconcile_seeded_links`]
//! and settled by the link calls below: the seed tables live in the agent
//! crate, so the caller passes in what ships ([`ShippedLinks`]).

use super::*;
use crate::markdown::PreparedNode;
use crate::models::schema_node::{is_core_schema, CONTEXT_PATHS_KEY};
use crate::models::seed_update::{PendingSeedUpdate, PendingSeedUpdateRow};
use crate::models::{SchemaNode, SKILL_ATTACHED_TO};
use nodespace_types::RelationshipPath;

/// The reverse name of [`SKILL_ATTACHED_TO`]: an edge written under it is the
/// same link.
const SKILL_ATTACHED_SKILLS: &str = "attached_skills";

/// What a built-in skill ships attached to: the skill's id, and the ids of the
/// nodes its `attached_to` links reach.
#[derive(Debug, Clone, Copy)]
pub struct ShippedLinks<'a> {
    pub skill_id: &'a str,
    pub targets: &'a [&'a str],
}

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
        // A template ships no context paths and no links: a core schema's
        // paths are compared by `compare_pending_context_paths_update`, a
        // skill's links by `compare_pending_links_update`.
        if matches!(aspect, SeedAspect::ContextPaths | SeedAspect::Links) {
            return Ok(None);
        }
        let Some(update) = self
            .get_pending_seed_update(&template_root.id, aspect)
            .await?
        else {
            return Ok(None);
        };

        let yours = match aspect {
            SeedAspect::ContextPaths | SeedAspect::Links => String::new(),
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
        // Taken by `take_context_paths_update` and `take_links_update`: a
        // template ships neither.
        if matches!(aspect, SeedAspect::ContextPaths | SeedAspect::Links) {
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
    /// changing nothing, when nothing was pending: this is a choice about a
    /// pending update, and [`Self::reset_context_paths`] is the path that
    /// discards an edit unconditionally.
    pub async fn take_context_paths_update(
        &self,
        shipped: &SchemaNode,
    ) -> Result<bool, NodeServiceError> {
        if self
            .store
            .get_pending_seed_update(&shipped.envelope.id, SeedAspect::ContextPaths)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_none()
        {
            return Ok(false);
        }
        self.reset_context_paths(shipped).await
    }

    /// Put the context paths of the core schema `shipped` is this build's
    /// definition of back to the shipped list, whether or not a shipped
    /// change is pending: the user's paths are discarded, the modified flag
    /// is cleared, so the aspect follows what ships again, and any pending
    /// record is removed. Returns `false`, changing nothing, when the schema
    /// is not in this database.
    pub async fn reset_context_paths(
        &self,
        shipped: &SchemaNode,
    ) -> Result<bool, NodeServiceError> {
        let schema_id = &shipped.envelope.id;
        let is_core = self
            .store
            .get_node(schema_id)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_some_and(|node| is_core_schema(&node));
        if !is_core {
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

    /// Bring every built-in skill's `attached_to` links up to date with the
    /// ones `shipped` declares, by the rule every other seeded aspect follows
    /// (ADR-072, [`Self::seed_nodes_from_templates`]):
    ///
    /// | state                                         | action                         |
    /// |-----------------------------------------------|--------------------------------|
    /// | skill absent                                  | nothing (seeding creates it)   |
    /// | fingerprint matches                           | skip                           |
    /// | fingerprint differs, `links_modified` not set | make the links the shipped set |
    /// | fingerprint differs, `links_modified` set     | keep, record as pending        |
    ///
    /// The fingerprint ([`links_version`]) of the shipped set a skill's links
    /// were last brought up to date with, or kept against, is
    /// `_seed.links_version` on the skill. The flag is set when anything but
    /// seeding creates or deletes a link on a seeded skill. A link the user
    /// deleted therefore stays deleted: a matching fingerprint skips the
    /// skill whatever its links are.
    ///
    /// A skill whose links cannot be written is logged and left unstamped, so
    /// the next open tries again.
    pub async fn reconcile_seeded_links(
        &self,
        shipped: &[ShippedLinks<'_>],
    ) -> Result<(), NodeServiceError> {
        let ids: Vec<String> = shipped.iter().map(|s| s.skill_id.to_string()).collect();
        let stored = self
            .store
            .get_nodes_by_ids(&ids)
            .await
            .map_err(NodeServiceError::from_store)?;
        let linked = self
            .store
            .get_edge_targets_by_source(&ids, SKILL_ATTACHED_TO)
            .await
            .map_err(NodeServiceError::from_store)?;
        let pending: HashSet<String> = self
            .store
            .list_pending_seed_updates()
            .await
            .map_err(NodeServiceError::from_store)?
            .into_iter()
            .filter(|row| row.aspect == SeedAspect::Links)
            .map(|row| row.node_id)
            .collect();

        for links in shipped {
            let Some(node) = stored.get(links.skill_id) else {
                continue;
            };
            let seed = node.properties.get("_seed");
            let stored_version = seed
                .and_then(|s| s.get(SeedAspect::Links.version_key()))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let modified = seed
                .and_then(|s| s.get(SeedAspect::Links.modified_key()))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let version = links_version(links.targets);
            let was_pending = pending.contains(links.skill_id);

            if stored_version == version {
                self.settle_pending_seed_update(links.skill_id, SeedAspect::Links, was_pending)
                    .await?;
            } else if modified {
                tracing::info!(
                    skill_id = links.skill_id,
                    "Shipped links changed but the skill's were user-modified; kept, recorded \
                     as pending"
                );
                self.store
                    .record_pending_seed_update(links.skill_id, SeedAspect::Links, &version)
                    .await
                    .map_err(NodeServiceError::from_store)?;
            } else {
                let current = linked.get(links.skill_id).map(Vec::as_slice).unwrap_or(&[]);
                match self.write_shipped_links(links, current, false).await {
                    Ok(()) => {
                        self.settle_pending_seed_update(
                            links.skill_id,
                            SeedAspect::Links,
                            was_pending,
                        )
                        .await?;
                    }
                    // One skill that fails does not cost the others theirs.
                    Err(e) => {
                        tracing::warn!(skill_id = links.skill_id, error = %e, "Failed to link a seeded skill");
                    }
                }
            }
        }
        Ok(())
    }

    /// The pending update to the `attached_to` links of the skill `shipped`
    /// is this build's definition of, with the shipped links beside the ones
    /// in this database, one per line. `None` when nothing is pending for it.
    pub async fn compare_pending_links_update(
        &self,
        shipped: ShippedLinks<'_>,
    ) -> Result<Option<SeedUpdateComparison>, NodeServiceError> {
        let Some(update) = self
            .get_pending_seed_update(shipped.skill_id, SeedAspect::Links)
            .await?
        else {
            return Ok(None);
        };
        let yours = self.current_links(shipped.skill_id).await?;
        let shipped_ids: Vec<String> = shipped.targets.iter().map(ToString::to_string).collect();

        Ok(Some(SeedUpdateComparison {
            update,
            shipped: self.links_text(&shipped_ids).await?,
            yours: self.links_text(&yours).await?,
        }))
    }

    /// Take the shipped links of the skill `shipped` is this build's
    /// definition of: they replace the user's, and the aspect's modified flag
    /// and its pending record are cleared. Returns `false`, changing nothing,
    /// when nothing was pending: this is a choice about a pending update, and
    /// [`Self::reset_links`] is the path that discards an edit
    /// unconditionally.
    pub async fn take_links_update(
        &self,
        shipped: ShippedLinks<'_>,
    ) -> Result<bool, NodeServiceError> {
        if self
            .store
            .get_pending_seed_update(shipped.skill_id, SeedAspect::Links)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_none()
        {
            return Ok(false);
        }
        self.reset_links(shipped).await
    }

    /// Make the `attached_to` links of the skill `shipped` is this build's
    /// definition of exactly the shipped ones, whether or not a shipped
    /// change is pending: a link the user deleted comes back, one they added
    /// is removed, the modified flag is cleared so the aspect follows what
    /// ships again, and any pending record is removed. Returns `false`,
    /// changing nothing, when the skill is not in this database.
    pub async fn reset_links(&self, shipped: ShippedLinks<'_>) -> Result<bool, NodeServiceError> {
        if self
            .store
            .get_node(shipped.skill_id)
            .await
            .map_err(NodeServiceError::from_store)?
            .is_none()
        {
            return Ok(false);
        }
        let current = self.current_links(shipped.skill_id).await?;
        self.write_shipped_links(&shipped, &current, true).await?;
        self.settle_pending_seed_update(shipped.skill_id, SeedAspect::Links, true)
            .await?;
        Ok(true)
    }

    /// The targets the skill's `attached_to` links reach now.
    pub async fn current_links(&self, skill_id: &str) -> Result<Vec<String>, NodeServiceError> {
        Ok(self
            .store
            .get_edge_targets_by_source(&[skill_id.to_string()], SKILL_ATTACHED_TO)
            .await
            .map_err(NodeServiceError::from_store)?
            .remove(skill_id)
            .unwrap_or_default())
    }

    /// Links as text, one per line, in the order given: the target's title
    /// and its id.
    pub async fn links_text(&self, target_ids: &[String]) -> Result<String, NodeServiceError> {
        let titles = self
            .store
            .get_nodes_by_ids(target_ids)
            .await
            .map_err(NodeServiceError::from_store)?;
        Ok(target_ids
            .iter()
            .map(|id| match titles.get(id) {
                Some(node) => format!("{} ({id})", node.content),
                None => id.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Make the skill's `attached_to` links the shipped set, in one
    /// transaction, and stamp its fingerprint. `current` is the targets it is
    /// linked to now. `clear_modified` also clears the modified flag: the
    /// links are the shipped ones again, by the user's choice.
    ///
    /// Written through the transaction twins of the relationship calls, which
    /// do not mark a seeded skill's links as the user's.
    async fn write_shipped_links(
        &self,
        shipped: &ShippedLinks<'_>,
        current: &[String],
        clear_modified: bool,
    ) -> Result<(), NodeServiceError> {
        let skill_id = shipped.skill_id.to_string();
        let removed: Vec<String> = current
            .iter()
            .filter(|target| !shipped.targets.contains(&target.as_str()))
            .cloned()
            .collect();
        let added: Vec<String> = shipped
            .targets
            .iter()
            .filter(|target| !current.iter().any(|c| c == *target))
            .map(ToString::to_string)
            .collect();

        let service = self.clone();
        let tx_skill_id = skill_id.clone();
        self.with_transaction(move |tx| {
            Box::pin(async move {
                for target in &removed {
                    service
                        .remove_relationship_in_tx(tx, &tx_skill_id, SKILL_ATTACHED_TO, target)
                        .await?;
                }
                for target in &added {
                    service
                        .create_relationship_in_tx(
                            tx,
                            &tx_skill_id,
                            SKILL_ATTACHED_TO,
                            target,
                            serde_json::json!({}),
                        )
                        .await?;
                }
                Ok(())
            })
        })
        .await?;

        // Same best-effort, OCC-bypassing stamp reconciliation uses for
        // `_seed` bookkeeping.
        self.store
            .set_property_string(
                &skill_id,
                &format!("$._seed.{}", SeedAspect::Links.version_key()),
                &links_version(shipped.targets),
            )
            .await
            .map_err(NodeServiceError::from_store)?;
        if clear_modified {
            self.store
                .set_property_bool(
                    &skill_id,
                    &format!("$._seed.{}", SeedAspect::Links.modified_key()),
                    false,
                )
                .await
                .map_err(NodeServiceError::from_store)?;
        }
        Ok(())
    }

    /// Mark the links of a seeded skill as the user's, when a call that is
    /// not seeding created or deleted one. `source_id` and `target_id` are
    /// the edge as the caller named it, under `relationship_name` or its
    /// reverse name. Best-effort, like the other `_seed` stamps: a failure to
    /// stamp is logged and does not fail the edit it follows.
    pub(super) async fn mark_seeded_links_modified(
        &self,
        relationship_name: &str,
        source_id: &str,
        target_id: &str,
    ) {
        let skill_id = match relationship_name {
            SKILL_ATTACHED_TO => source_id,
            SKILL_ATTACHED_SKILLS => target_id,
            _ => return,
        };
        let seeded = match self.store.get_node(skill_id).await {
            Ok(node) => node.is_some_and(|node| node.properties.get("_seed").is_some()),
            Err(e) => {
                tracing::warn!(skill_id, error = %e, "Failed to read a skill to mark its links modified");
                return;
            }
        };
        if !seeded {
            return;
        }
        if let Err(e) = self
            .store
            .set_property_bool(
                skill_id,
                &format!("$._seed.{}", SeedAspect::Links.modified_key()),
                true,
            )
            .await
        {
            tracing::warn!(skill_id, error = %e, "Failed to mark a skill's links modified");
        }
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
            SeedAspect::Config | SeedAspect::ContextPaths | SeedAspect::Links => node.modified_at,
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
        // A template ships no context paths and no links.
        SeedAspect::ContextPaths | SeedAspect::Links => String::new(),
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

/// The fingerprint of the set of nodes a skill ships attached to: what
/// `_seed.links_version` holds on a built-in skill, and what a pending update
/// to its links records. Order is not part of it: a link has no order.
pub fn links_version(targets: &[&str]) -> String {
    use sha2::{Digest, Sha256};

    let mut sorted: Vec<&str> = targets.to_vec();
    sorted.sort_unstable();
    let mut hasher = Sha256::new();
    for target in sorted {
        hasher.update(target.as_bytes());
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
