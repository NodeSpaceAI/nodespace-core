//! Conflict-journal operations for NodeService (ADR-068): deterministic
//! conflict identity, the thin `NodeService`-level wrappers over
//! `SqliteStore`'s conflict-journal methods, and the `UniqueFieldCollision`
//! detection hook wired into create/update (see `crud.rs`).

use super::*;
use crate::models::conflict::{ConflictKind, ConflictRecord, ConflictStatus, Resolution};

/// The outcome of a [`NodeService::merge_nodes`] call — what actually
/// happened, for the caller (Conflicts view) to report to the user.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOutcome {
    pub survivor_id: String,
    pub loser_id: String,
    pub properties_merged: u32,
    pub edges_repointed: u32,
    pub edges_dropped: u32,
}

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

    /// Read a single conflict record by id, if it exists.
    pub async fn get_conflict(
        &self,
        conflict_id: &str,
    ) -> Result<Option<ConflictRecord>, NodeServiceError> {
        self.store
            .get_conflict(conflict_id)
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

    /// Merge `loser` into `survivor` (conflict-journal-and-resolution.md
    /// §5.2, ADR-068): property union onto the survivor (survivor wins ties,
    /// the loser's overwritten values are snapshotted into
    /// `resolution.superseded`), every relationship edge touching the loser
    /// re-pointed to the survivor, and the loser archived (see
    /// `SqliteStore::merge_nodes_in_tx`'s doc comment for why `archived`
    /// rather than a literal "deleted" tombstone). If `conflict_id` is given,
    /// the record is closed as `resolved` with a `Resolution::Merge` in the
    /// SAME transaction.
    ///
    /// **User-initiated only** — this method performs the merge unconditionally
    /// whenever called; it is the caller's responsibility (the Conflicts view)
    /// to gate this behind an explicit user action. Nothing in this crate
    /// calls it automatically, at any confidence: two nodes sharing a value is
    /// evidence, not proof (ADR-065 §5 — email is a claim, not an identity
    /// key), and an auto-merge on a false positive would silently destroy a
    /// distinct node's data and re-point its edges onto the wrong survivor.
    pub async fn merge_nodes(
        &self,
        survivor_id: &str,
        loser_id: &str,
        conflict_id: Option<&str>,
    ) -> Result<MergeOutcome, NodeServiceError> {
        let survivor_id = survivor_id.to_string();
        let loser_id = loser_id.to_string();
        let conflict_id = conflict_id.map(|s| s.to_string());

        let survivor_id_for_tx = survivor_id.clone();
        let loser_id_for_tx = loser_id.clone();

        let (properties_merged, edges_repointed, edges_dropped) = self
            .with_transaction(move |ns_tx| {
                let survivor_id = survivor_id_for_tx.clone();
                let loser_id = loser_id_for_tx.clone();
                let conflict_id = conflict_id.clone();
                Box::pin(async move {
                    let (properties_merged, superseded, edges_repointed, edges_dropped) =
                        crate::db::SqliteStore::merge_nodes_in_tx(
                            ns_tx.store_tx(),
                            &survivor_id,
                            &loser_id,
                        )
                        .await
                        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

                    if let Some(conflict_id) = conflict_id {
                        let resolution = crate::models::Resolution::Merge {
                            survivor: survivor_id.clone(),
                            loser: loser_id.clone(),
                            superseded,
                            edges_repointed,
                            edges_dropped,
                        };
                        crate::db::SqliteStore::resolve_conflict_in_tx(
                            ns_tx.store_tx(),
                            &conflict_id,
                            &resolution,
                        )
                        .await
                        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
                    }

                    Ok((properties_merged, edges_repointed, edges_dropped))
                })
            })
            .await?;

        Ok(MergeOutcome {
            survivor_id,
            loser_id,
            properties_merged,
            edges_repointed,
            edges_dropped,
        })
    }

    /// Reconciliation sweep (conflict-journal-and-resolution.md §5.4): close
    /// every OPEN conflict record whose participant(s) are gone (hard-deleted)
    /// or no longer collide (renamed, merged elsewhere). Not required for S1-S3
    /// correctness — a stale open record for a departed node is a stale row,
    /// not a wrong one — but required before the Conflicts view is fully
    /// trustworthy, and the backstop for the accepted TOCTOU gap in the
    /// pre-write-check/post-write-mark detection pattern (§4): a full re-scan
    /// re-runs each kind's predicate and closes anything the inline check
    /// alone would have missed.
    ///
    /// Best-effort per record: one record's re-check failing (e.g. a
    /// transient store error) is logged and skipped, not propagated — a sweep
    /// is a background maintenance pass, not a user-facing operation whose
    /// failure should abort the rest of the pass.
    ///
    /// Returns the number of records closed.
    pub async fn reconcile_conflicts(&self) -> Result<u32, NodeServiceError> {
        let open = self
            .store
            .list_conflicts(Some(ConflictStatus::Open), None, None)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let mut closed = 0u32;
        for record in open {
            match self.reconcile_one(&record).await {
                Ok(true) => closed += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        conflict_id = %record.id,
                        error = %e,
                        "reconciliation sweep: failed to re-check conflict record, skipping"
                    );
                }
            }
        }
        Ok(closed)
    }

    /// Re-check one open record; close it (resolved, no user action) if a
    /// participant is gone or the kind's predicate no longer finds a
    /// collision. Returns whether it was closed.
    async fn reconcile_one(&self, record: &ConflictRecord) -> Result<bool, NodeServiceError> {
        // A hard-deleted participant closes the record unconditionally,
        // regardless of kind — there is nothing left to re-collide.
        for node_id in &record.node_ids {
            if self.get_node(node_id).await?.is_none() {
                self.close_stale(&record.id, "participant_deleted").await?;
                return Ok(true);
            }
        }

        let still_conflicts = match record.kind {
            ConflictKind::UniqueFieldCollision => {
                self.unique_field_collision_still_holds(record).await?
            }
            ConflictKind::CollectionNameCollision => {
                self.collection_name_collision_still_holds(record).await?
            }
            // No detection yet for these kinds (S1-S3 scope); nothing to
            // re-check, so never auto-close one.
            ConflictKind::SupersededEdit | ConflictKind::DuplicateReactiveCreate => true,
        };

        if !still_conflicts {
            self.close_stale(&record.id, "no_longer_conflicting")
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    async fn close_stale(&self, conflict_id: &str, reason: &str) -> Result<(), NodeServiceError> {
        self.store
            .resolve_conflict(
                conflict_id,
                Resolution::SelfResolved {
                    reason: reason.to_string(),
                },
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        Ok(())
    }

    async fn unique_field_collision_still_holds(
        &self,
        record: &ConflictRecord,
    ) -> Result<bool, NodeServiceError> {
        let (Some(node_type), Some(field), Some(_original_value)) = (
            record.detail.get("node_type").and_then(|v| v.as_str()),
            record.detail.get("field").and_then(|v| v.as_str()),
            record.detail.get("value").and_then(|v| v.as_str()),
        ) else {
            return Ok(true); // malformed detail — leave it for a human, don't guess
        };
        let case_insensitive = record
            .detail
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Re-derive from each participant's OWN CURRENT value, not the value
        // recorded at detection time — a participant may have since changed
        // it (as this exact scenario exercises: Bob edits his email away
        // from Alice's). Checking a stale `detail.value` from an arbitrary
        // participant's exclusion perspective is wrong in both directions:
        // excluding the participant who MOVED AWAY still finds the OTHER
        // participant's still-correct value and reports a false collision.
        for node_id in &record.node_ids {
            let Some(node) = self.get_node(node_id).await? else {
                continue; // already handled by the hard-delete check in reconcile_one
            };
            let Some(current_value) = node
                .properties
                .get(node_type)
                .and_then(|p| p.get(field))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if current_value.trim().is_empty() {
                continue;
            }

            let conflicting = self
                .store
                .find_conflicting_unique(
                    node_type,
                    field,
                    current_value,
                    Some(node_id),
                    case_insensitive,
                )
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            if conflicting.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn collection_name_collision_still_holds(
        &self,
        record: &ConflictRecord,
    ) -> Result<bool, NodeServiceError> {
        let Some(name) = record.detail.get("name").and_then(|v| v.as_str()) else {
            return Ok(true);
        };
        let Some(subject_id) = record.node_ids.first() else {
            return Ok(true);
        };
        let existing = self
            .store
            .get_collection_by_name(name)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        Ok(match existing {
            // A different active collection still holds the name -> still colliding.
            Some(node) => &node.id != subject_id,
            None => false,
        })
    }
}
