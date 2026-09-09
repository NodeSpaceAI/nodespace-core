//! Conflict-journal operations for NodeService (ADR-068): deterministic
//! conflict identity, the thin `NodeService`-level wrappers over
//! `SqliteStore`'s conflict-journal methods, and the `UniqueFieldCollision`
//! detection hook wired into create/update (see `crud.rs`).

use super::*;
use crate::models::conflict::{ConflictKind, ConflictRecord, ConflictStatus, Resolution};

/// Stable namespace for deterministic conflict ids (UUIDv5). A fixed,
/// arbitrary UUID — do NOT change it: existing open/resolved/dismissed
/// records are keyed by ids derived from it, and changing it would silently
/// orphan every record ever written (re-detection would mint a new id and
/// never find the old row, including any dismissal). Distinct from
/// `collection_service::COLLECTION_ID_NAMESPACE` — the two id spaces must
/// never collide.
const CONFLICT_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x2c6f1a9d_5b3e_4c8f_91d2_7a4e6f0b3c8du128);

/// Deterministic conflict id from `kind`, the sorted participant set, and a
/// kind-specific `discriminator` (conflict-journal-and-resolution.md §2.3):
///
/// | Kind                       | Discriminator                  |
/// |-----------------------------|--------------------------------|
/// | `UniqueFieldCollision`       | `"{node_type}.{field}"`       |
/// | `CollectionNameCollision`    | the case-folded collection name |
/// | `SupersededEdit`             | the superseding write's `modified_at` |
///
/// Sorting `node_ids` before hashing is what makes the id **symmetric**:
/// whichever of the two colliding nodes is "new" and whichever is
/// "pre-existing" is an accident of arrival order (device A converges before
/// device B, or vice versa) and must not produce two different ids for what
/// is structurally the same conflict. Determinism plus that symmetry is also
/// what makes re-detection **idempotent** — the same collision observed again
/// hashes to the same id, so `record_conflict`'s upsert bumps `occurrences`
/// on the existing row instead of inserting a duplicate — and what lets a
/// **dismissed** conflict recognize its own re-detection instead of being
/// silently re-raised (the exact defect `_possible_duplicate` had: it was
/// content-compared per ADR-026/ADR-060's echo-suppression finding that
/// identity must come from structure, never from comparing values).
pub fn deterministic_conflict_id(
    kind: ConflictKind,
    node_ids: &[String],
    discriminator: &str,
) -> String {
    let mut sorted: Vec<String> = node_ids.to_vec();
    sorted.sort();
    let seed = format!("{kind}|{}|{discriminator}", sorted.join(","));
    uuid::Uuid::new_v5(&CONFLICT_ID_NAMESPACE, seed.as_bytes()).to_string()
}

impl NodeService {
    /// List conflict records, optionally filtered by status/kind.
    pub async fn list_conflicts(
        &self,
        status: Option<ConflictStatus>,
        kind: Option<ConflictKind>,
        limit: Option<u32>,
    ) -> Result<Vec<ConflictRecord>, NodeServiceError> {
        self.store
            .list_conflicts(status, kind, limit)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Every open-or-otherwise conflict record naming `node_id` as a
    /// participant. Backing the inline "is this node in a conflict"
    /// indicator — a derived read of the journal, never a stored property
    /// (conflict-journal-and-resolution.md §6.3).
    pub async fn conflicts_for_node(
        &self,
        node_id: &str,
    ) -> Result<Vec<ConflictRecord>, NodeServiceError> {
        self.store
            .conflicts_for_node(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Apply a resolution to an existing conflict record.
    pub async fn resolve_conflict(
        &self,
        conflict_id: &str,
        resolution: Resolution,
    ) -> Result<ConflictRecord, NodeServiceError> {
        self.store
            .resolve_conflict(conflict_id, resolution)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Scan `node_id`'s unique-flagged, string-valued schema fields for a
    /// conflicting active node of the same type and, for each collision
    /// found, journal a `UniqueFieldCollision` conflict record naming both.
    ///
    /// This is the real caller `mark_possible_duplicates` never had: it is
    /// wired into `create_node`/`update_node` (see `crud.rs`) as a **post-commit,
    /// best-effort, error-swallowing** step — the write it follows has
    /// already succeeded and must never be undone or blocked by a
    /// journal-write failure (ADR-065 §4, unchanged). Callers must log and
    /// swallow any `Err` this returns rather than propagate it.
    ///
    /// Generic across node types by construction: walks whatever fields the
    /// type's schema flags `unique`/`unique_case_insensitive`, exactly as
    /// `mark_possible_duplicates` did — so an extension type with its own
    /// `unique` field gets conflict-journal detection with zero schema
    /// participation, no code change required.
    pub(crate) async fn detect_unique_field_collisions(
        &self,
        node_id: &str,
    ) -> Result<(), NodeServiceError> {
        let Some(node) = self.get_node(node_id).await? else {
            return Ok(());
        };

        let schema = self
            .store
            .get_schema_node(&node.node_type)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let Some(schema) = schema else {
            return Ok(());
        };

        let unique_fields = schema
            .fields
            .iter()
            .filter(|f| f.unique.unwrap_or(false) || f.unique_case_insensitive.unwrap_or(false));

        for field in unique_fields {
            let Some(value) = node
                .properties
                .get(&node.node_type)
                .and_then(|p| p.get(&field.name))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }

            let case_insensitive = field.unique_case_insensitive.unwrap_or(false);
            let conflicting_id = self
                .store
                .find_conflicting_unique(
                    &node.node_type,
                    &field.name,
                    value,
                    Some(&node.id),
                    case_insensitive,
                )
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

            let Some(conflicting_id) = conflicting_id else {
                continue;
            };

            let discriminator = format!("{}.{}", node.node_type, field.name);
            let node_ids = vec![node.id.clone(), conflicting_id.clone()];
            let id = deterministic_conflict_id(
                ConflictKind::UniqueFieldCollision,
                &node_ids,
                &discriminator,
            );
            let detail = serde_json::json!({
                "node_type": node.node_type,
                "field": field.name,
                "value": value,
                "case_insensitive": case_insensitive,
            });

            self.store
                .record_conflict(
                    &id,
                    ConflictKind::UniqueFieldCollision,
                    &node_ids,
                    detail,
                    self.client_id.as_deref(),
                )
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        }

        Ok(())
    }
}
