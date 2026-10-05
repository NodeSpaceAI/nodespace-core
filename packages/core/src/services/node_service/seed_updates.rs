//! Pending seed updates for NodeService (ADR-094 §8): the shipped changes
//! reconciliation held back because the user had edited that aspect, and the
//! two choices that settle one. See [`NodeService::seed_nodes_from_templates`]
//! for where they are recorded.

use super::*;
use crate::markdown::PreparedNode;
use crate::models::seed_update::{PendingSeedUpdate, PendingSeedUpdateRow};

/// A pending update with both versions of the aspect, as text a person can
/// read side by side: Markdown for guidance, the name and fields for config.
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
        let Some(update) = self
            .get_pending_seed_update(&template_root.id, aspect)
            .await?
        else {
            return Ok(None);
        };

        let yours = match aspect {
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
            SeedAspect::Config => node.modified_at,
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
