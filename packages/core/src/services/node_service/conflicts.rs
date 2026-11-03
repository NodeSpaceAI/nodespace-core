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

/// Where a [`NodeService::merge_nodes`] call would leave the survivor in the
/// tree — see [`NodeService::preview_merge`]. Parents ignore an edge between
/// the two nodes themselves; `None` means a root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergePreview {
    /// The survivor's parent before the merge.
    pub survivor_parent_id: Option<String>,
    /// The loser's parent before the merge. When it differs from
    /// `resulting_parent_id`, the loser's place in the outline is dropped.
    pub loser_parent_id: Option<String>,
    /// The survivor's parent after the merge.
    pub resulting_parent_id: Option<String>,
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

/// Refuse a merge whose survivor and loser are the same node. The merge would
/// otherwise treat the node's own parent edge as the loser's, drop it and
/// archive the node: it would leave its place in the tree and could not be
/// put back by un-archiving it.
fn refuse_self_merge(survivor_id: &str, loser_id: &str) -> Result<(), NodeServiceError> {
    if survivor_id == loser_id {
        return Err(NodeServiceError::invalid_update(format!(
            "Cannot merge node '{survivor_id}' into itself: the survivor and the loser must be \
             different nodes"
        )));
    }
    Ok(())
}

/// The bounds of one reconciliation scan.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanLimits {
    /// Colliding values (or names) read per query.
    pub chunk: u32,
    /// Members of one colliding group examined; a larger group is journaled for
    /// its first members and the rest are found as those are resolved.
    pub group_members: u32,
    /// Records one scan writes before stopping; a later sweep continues.
    pub max_journaled: u32,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            chunk: 200,
            group_members: 20,
            max_journaled: 500,
        }
    }
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
            .map_err(NodeServiceError::from_store)
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
            .map_err(NodeServiceError::from_store)
    }

    /// Read a single conflict record by id, if it exists.
    pub async fn get_conflict(
        &self,
        conflict_id: &str,
    ) -> Result<Option<ConflictRecord>, NodeServiceError> {
        self.store
            .get_conflict(conflict_id)
            .await
            .map_err(NodeServiceError::from_store)
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
            .map_err(NodeServiceError::from_store)
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

        // Resolved via `resolve_field_owners` rather than a direct
        // `get_schema_node(node.node_type)` lookup: the latter returns only
        // the type's own directly-declared fields, not the ADR-078
        // `extends`-chain-merged set. A `unique`/`unique_case_insensitive`
        // field declared only on an ancestor schema and inherited (not
        // redeclared) by a subtype was therefore invisible here, so a real
        // duplicate value on an inherited unique field never got journaled
        // as a `UniqueFieldCollision` conflict. Same fix pattern as
        // `workflow_state.rs`, `validation.rs`, `path_ops.rs`'s
        // `resolve_hop`, and `rel_ops.rs`'s
        // `resolve_relationship_name`/`get_node_relationships`. When
        // `node.node_type` has no schema at all, `resolve_field_owners`
        // returns an empty field set — same "nothing to journal" outcome
        // the old direct lookup produced for a missing schema.
        let (fields, owners, _chain) = self.resolve_field_owners(&node.node_type).await?;

        let unique_fields = fields
            .iter()
            .filter(|f| f.unique.unwrap_or(false) || f.unique_case_insensitive.unwrap_or(false));

        for field in unique_fields {
            // An inherited field's value is stored under its *owning*
            // ancestor schema's bucket (`bucket_properties_by_owner`, ADR-078),
            // not under `node.node_type`'s own bucket — the two coincide only
            // for a directly-declared (non-inherited) field. `owners` (from
            // the same `resolve_field_owners` call above) is exactly the
            // field-name -> owning-schema-id map that write path used to
            // bucket this value, so it's reused here to read it back from the
            // right place. Falls back to `node.node_type` for the (common,
            // unextended) case where a field owns itself.
            let bucket = owners
                .get(&field.name)
                .map(String::as_str)
                .unwrap_or(node.node_type.as_str());
            let Some(value) = node
                .properties
                .get(bucket)
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
                    bucket,
                    &field.name,
                    value,
                    Some(&node.id),
                    case_insensitive,
                )
                .await
                .map_err(NodeServiceError::from_store)?;

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
                .map_err(NodeServiceError::from_store)?;
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
        refuse_self_merge(survivor_id, loser_id)?;
        let survivor_id = survivor_id.to_string();
        let loser_id = loser_id.to_string();
        let conflict_id = conflict_id.map(|s| s.to_string());

        let survivor_id_for_tx = survivor_id.clone();
        let loser_id_for_tx = loser_id.clone();
        let service_for_tx = self.clone();

        let (properties_merged, edges_repointed, edges_dropped) = self
            .with_transaction(move |ns_tx| {
                let survivor_id = survivor_id_for_tx.clone();
                let loser_id = loser_id_for_tx.clone();
                let conflict_id = conflict_id.clone();
                let service = service_for_tx.clone();
                Box::pin(async move {
                    // Read before the merge re-points it: afterwards the loser
                    // has no parent.
                    let loser_parent =
                        crate::db::SqliteStore::get_parent_id_in_tx(ns_tx.store_tx(), &loser_id)
                            .await
                            .map_err(NodeServiceError::from_store)?;
                    let (properties_merged, superseded, repointed_edges, edges_dropped) =
                        crate::db::SqliteStore::merge_nodes_in_tx(
                            ns_tx.store_tx(),
                            &survivor_id,
                            &loser_id,
                        )
                        .await
                        .map_err(NodeServiceError::from_store)?;

                    // The store-level repoint above is schema-blind (see its
                    // doc comment) — close any cardinality-one violation it
                    // may have just produced, in the same transaction, before
                    // anything else reads the survivor's edge set.
                    let evicted_for_cardinality = service
                        .enforce_cardinality_after_merge_in_tx(
                            ns_tx,
                            &survivor_id,
                            &repointed_edges,
                        )
                        .await?;
                    let edges_repointed =
                        (repointed_edges.len() as u32).saturating_sub(evicted_for_cardinality);
                    let edges_dropped = edges_dropped + evicted_for_cardinality;

                    // Re-pointing can hand a root survivor the loser's parent
                    // edge, changing whether it is a root, so its title
                    // follows. The loser's parent is passed as the former
                    // parent: the tree it heads lost the loser's text. When
                    // the merge dropped the loser's parent edge, that tree is
                    // queued only through this. When the edge was re-pointed
                    // instead, both resolve to the same root and it is
                    // queued once.
                    let survivor_is_root =
                        crate::db::SqliteStore::get_parent_id_in_tx(ns_tx.store_tx(), &survivor_id)
                            .await
                            .map_err(NodeServiceError::from_store)?
                            .is_none();
                    service
                        .refresh_for_rootness_in_tx(
                            ns_tx,
                            &survivor_id,
                            survivor_is_root,
                            loser_parent.as_deref(),
                        )
                        .await?;

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
                        .map_err(NodeServiceError::from_store)?;
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

    /// What [`Self::merge_nodes`] would do to the tree, without doing it: the
    /// survivor's parent afterward, or the `TreeInvariantViolation` the merge
    /// would be refused with. Lets the Conflicts view say where the merged
    /// node will live, and why a merge cannot happen, before the user
    /// confirms. Writes nothing, but runs under the write guard on purpose:
    /// parents, ancestry and memberships are read from one snapshot, exactly
    /// as the merge reads them.
    pub async fn preview_merge(
        &self,
        survivor_id: &str,
        loser_id: &str,
    ) -> Result<MergePreview, NodeServiceError> {
        refuse_self_merge(survivor_id, loser_id)?;
        let survivor_id = survivor_id.to_string();
        let loser_id = loser_id.to_string();
        let plan = self
            .with_transaction(move |ns_tx| {
                Box::pin(async move {
                    crate::db::SqliteStore::preview_merge_in_tx(
                        ns_tx.store_tx(),
                        &survivor_id,
                        &loser_id,
                    )
                    .await
                    .map_err(NodeServiceError::from_store)
                })
            })
            .await?;

        Ok(MergePreview {
            resulting_parent_id: plan.resulting_parent().map(str::to_string),
            survivor_parent_id: plan.survivor_parent,
            loser_parent_id: plan.loser_parent,
        })
    }

    /// Reconciliation sweep (conflict-journal-and-resolution.md §5.4): close
    /// every OPEN conflict record whose participant(s) are gone (hard-deleted)
    /// or no longer collide (renamed, merged elsewhere), then journal any
    /// collision the inline detection missed
    /// ([`Self::journal_missed_collisions`]). That second pass is the backstop
    /// for the accepted TOCTOU gap in the pre-write-check/post-write-mark
    /// pattern (§4), and for writes that run no inline check at all (an
    /// unarchive, an inline detection whose journal write failed).
    ///
    /// Best-effort per record: one record's re-check failing (e.g. a
    /// transient store error) is logged and skipped, not propagated — a sweep
    /// is a background maintenance pass, not a user-facing operation whose
    /// failure should abort the rest of the pass. A failed scan is likewise
    /// logged and does not hide the closes that already happened.
    ///
    /// Returns the number of records closed.
    pub async fn reconcile_conflicts(&self) -> Result<u32, NodeServiceError> {
        let open = self
            .store
            .list_conflicts(Some(ConflictStatus::Open), None, None)
            .await
            .map_err(NodeServiceError::from_store)?;

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

        match self.journal_missed_collisions().await {
            Ok(journaled) if journaled > 0 => {
                tracing::debug!(
                    journaled,
                    "reconciliation sweep journaled missed collisions"
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "reconciliation sweep: collision scan failed");
            }
        }
        Ok(closed)
    }

    /// Re-run each conflict kind's detection predicate over every participating
    /// node and journal the collisions that have no record yet.
    ///
    /// * `UniqueFieldCollision`: for every participating node type, each
    ///   `unique` / `unique_case_insensitive` string field (resolved through the
    ///   `extends` chain, as inline detection does) is grouped by value.
    /// * `CollectionNameCollision`: participating collections are grouped by
    ///   their case-folded title.
    ///
    /// Only participating nodes are considered, so an archived node never
    /// collides, exactly as in inline detection. A group of colliding nodes is
    /// journaled as pairs of its lowest id with each other member, under the
    /// same deterministic ids inline detection derives, so a record that already
    /// exists in ANY status (open, dismissed, resolved) is left alone and a
    /// second run journals nothing new.
    ///
    /// Bounded: groups are found in keyset-paged chunks of `ScanLimits::chunk` by
    /// read queries (no write lock is held between them; each journal write is
    /// its own short transaction), at most `ScanLimits::group_members` members of a
    /// group are examined, and at most `ScanLimits::max_journaled` records are written
    /// per call, the rest being found by a later sweep.
    ///
    /// Returns the number of records journaled.
    pub async fn journal_missed_collisions(&self) -> Result<u32, NodeServiceError> {
        self.journal_missed_collisions_within(ScanLimits::default())
            .await
    }

    /// [`Self::journal_missed_collisions`] under explicit bounds.
    pub(crate) async fn journal_missed_collisions_within(
        &self,
        limits: ScanLimits,
    ) -> Result<u32, NodeServiceError> {
        let mut journaled = 0u32;
        // One kind's hard error must not hide the other kind's collisions.
        if let Err(e) = self
            .scan_collection_name_collisions(&mut journaled, limits)
            .await
        {
            tracing::warn!(error = %e, "reconciliation sweep: collection-name scan failed, continuing");
        }
        if let Err(e) = self
            .scan_unique_field_collisions(&mut journaled, limits)
            .await
        {
            tracing::warn!(error = %e, "reconciliation sweep: unique-field scan failed");
        }
        Ok(journaled)
    }

    /// Journal the pairs (lowest id, each other id) of one colliding group that
    /// have no record yet. `ids` are in ascending order.
    async fn journal_group(
        &self,
        kind: ConflictKind,
        ids: &[String],
        discriminator: &str,
        detail: &serde_json::Value,
        journaled: &mut u32,
        limits: ScanLimits,
    ) -> Result<(), NodeServiceError> {
        let Some((anchor, others)) = ids.split_first() else {
            return Ok(());
        };
        for other in others {
            if *journaled >= limits.max_journaled {
                return Ok(());
            }
            let node_ids = vec![anchor.clone(), other.clone()];
            let id = deterministic_conflict_id(kind, &node_ids, discriminator);
            if self
                .store
                .get_conflict(&id)
                .await
                .map_err(NodeServiceError::from_store)?
                .is_some()
            {
                continue;
            }
            self.store
                .record_conflict(
                    &id,
                    kind,
                    &node_ids,
                    detail.clone(),
                    self.client_id.as_deref(),
                )
                .await
                .map_err(NodeServiceError::from_store)?;
            *journaled += 1;
        }
        Ok(())
    }

    async fn scan_collection_name_collisions(
        &self,
        journaled: &mut u32,
        limits: ScanLimits,
    ) -> Result<(), NodeServiceError> {
        let mut after: Option<String> = None;
        loop {
            let titles = self
                .store
                .duplicated_collection_titles(after.as_deref(), limits.chunk)
                .await
                .map_err(NodeServiceError::from_store)?;
            let Some(last) = titles.last().cloned() else {
                return Ok(());
            };
            for title in &titles {
                if *journaled >= limits.max_journaled {
                    return Ok(());
                }
                let ids = self
                    .store
                    .collection_ids_with_title(title, limits.group_members)
                    .await
                    .map_err(NodeServiceError::from_store)?;
                // Named as detection and the still-holds re-check name them: by
                // the node's own content, ASCII-folded.
                let mut name: Option<String> = None;
                let mut same_name = Vec::with_capacity(ids.len());
                for id in ids {
                    let Some(node) = self.get_node(&id).await? else {
                        continue;
                    };
                    let node_name = crate::services::collection_service::normalize_collection_name(
                        &node.content,
                    );
                    match &name {
                        Some(first) if *first != node_name => continue,
                        Some(_) => {}
                        None => name = Some(node_name),
                    }
                    same_name.push(id);
                }
                let Some(name) = name else { continue };
                let detail = serde_json::json!({ "name": name });
                self.journal_group(
                    ConflictKind::CollectionNameCollision,
                    &same_name,
                    &name,
                    &detail,
                    journaled,
                    limits,
                )
                .await?;
            }
            if titles.len() < limits.chunk as usize {
                return Ok(());
            }
            after = Some(last);
            tokio::task::yield_now().await;
        }
    }

    async fn scan_unique_field_collisions(
        &self,
        journaled: &mut u32,
        limits: ScanLimits,
    ) -> Result<(), NodeServiceError> {
        let node_types = self
            .store
            .participating_node_types()
            .await
            .map_err(NodeServiceError::from_store)?;
        for node_type in node_types {
            if *journaled >= limits.max_journaled {
                return Ok(());
            }
            // One type's unreadable schema or failed query must not hide the rest.
            if let Err(e) = self
                .scan_unique_fields_of_type(&node_type, journaled, limits)
                .await
            {
                tracing::warn!(
                    node_type = %node_type,
                    error = %e,
                    "reconciliation sweep: unique-field scan failed for a node type, skipping"
                );
            }
        }
        Ok(())
    }

    async fn scan_unique_fields_of_type(
        &self,
        node_type: &str,
        journaled: &mut u32,
        limits: ScanLimits,
    ) -> Result<(), NodeServiceError> {
        let (fields, owners, _chain) = self.resolve_field_owners(node_type).await?;
        for field in fields
            .iter()
            .filter(|f| f.unique.unwrap_or(false) || f.unique_case_insensitive.unwrap_or(false))
        {
            let bucket = owners
                .get(&field.name)
                .map(String::as_str)
                .unwrap_or(node_type);
            let case_insensitive = field.unique_case_insensitive.unwrap_or(false);
            let discriminator = format!("{node_type}.{}", field.name);

            let mut after: Option<String> = None;
            loop {
                let values = self
                    .store
                    .unique_value_duplicates(
                        node_type,
                        bucket,
                        &field.name,
                        case_insensitive,
                        after.as_deref(),
                        limits.chunk,
                    )
                    .await
                    .map_err(NodeServiceError::from_store)?;
                let Some(last) = values.last().cloned() else {
                    break;
                };
                for value in &values {
                    if *journaled >= limits.max_journaled {
                        return Ok(());
                    }
                    // Detection skips empty / whitespace-only values.
                    if value.trim().is_empty() {
                        continue;
                    }
                    let ids = self
                        .store
                        .ids_with_unique_value(
                            node_type,
                            bucket,
                            &field.name,
                            case_insensitive,
                            value,
                            limits.group_members,
                        )
                        .await
                        .map_err(NodeServiceError::from_store)?;
                    // Report the value as the anchor holds it, as detection does.
                    let shown = match ids.first() {
                        Some(anchor) => self
                            .get_node(anchor)
                            .await?
                            .and_then(|n| {
                                n.properties
                                    .get(bucket)
                                    .and_then(|p| p.get(&field.name))
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| value.clone()),
                        None => continue,
                    };
                    let detail = serde_json::json!({
                        "node_type": node_type,
                        "field": field.name,
                        "value": shown,
                        "case_insensitive": case_insensitive,
                    });
                    self.journal_group(
                        ConflictKind::UniqueFieldCollision,
                        &ids,
                        &discriminator,
                        &detail,
                        journaled,
                        limits,
                    )
                    .await?;
                }
                if values.len() < limits.chunk as usize {
                    break;
                }
                after = Some(last);
                tokio::task::yield_now().await;
            }
        }
        Ok(())
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
            .map_err(NodeServiceError::from_store)?;
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

        // Resolved via `resolve_field_owners` (ADR-078 extends-chain-merged
        // set) rather than a direct own-schema-only lookup, mirroring
        // `detect_unique_field_collisions`: `field` may be inherited from an
        // ancestor schema, in which case its value is bucketed under that
        // ancestor's schema id, not under `node_type`'s own bucket
        // (`bucket_properties_by_owner`). Both participants share the same
        // `node_type` (a prerequisite for `detect_unique_field_collisions` to
        // have matched them as conflicting in the first place), so the owning
        // bucket for `field` is resolved once, outside the loop below, rather
        // than per participant.
        let bucket = self
            .resolve_field_owners(node_type)
            .await?
            .1
            .get(field)
            .map(String::as_str)
            .unwrap_or(node_type)
            .to_string();

        // Re-derive from the participants' OWN CURRENT values, not the value
        // recorded at detection time — a participant may have since changed
        // it. The record holds only while every participant still takes part
        // (an archived node never collides, so a merged-away loser closes the
        // record its merge resolved) and they all still share one value. A
        // participant matching some other node instead is a different
        // collision, journaled under its own record; searching the whole type
        // for any match would keep this one open over a value its
        // participants no longer share.
        if record.node_ids.len() < 2 {
            return Ok(true); // malformed participants — leave it for a human
        }
        let mut shared: Option<String> = None;
        for node_id in &record.node_ids {
            let Some(node) = self.get_node(node_id).await? else {
                return Ok(false); // already handled by the hard-delete check in reconcile_one
            };
            if !crate::governance::participates(&node) {
                return Ok(false);
            }
            let Some(value) = node
                .properties
                .get(&bucket)
                .and_then(|p| p.get(field))
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            else {
                return Ok(false);
            };
            // Compared as detection compares them: exactly, or case-folded
            // for a case-insensitive field.
            let value = if case_insensitive {
                value.to_lowercase()
            } else {
                value.to_string()
            };
            match &shared {
                Some(first) if *first != value => return Ok(false),
                Some(_) => {}
                None => shared = Some(value),
            }
        }
        Ok(true)
    }

    /// Whether the record's participants still collide on its name: each one
    /// is still an active collection whose name folds to the recorded one.
    ///
    /// Re-derived from the participants themselves. The record's ids are
    /// sorted, so which one comes first says nothing about the collision, and
    /// a lookup by name returns whichever matching collection it reaches
    /// first, so comparing the two would keep or close a record by id order.
    async fn collection_name_collision_still_holds(
        &self,
        record: &ConflictRecord,
    ) -> Result<bool, NodeServiceError> {
        let Some(name) = record.detail.get("name").and_then(|v| v.as_str()) else {
            return Ok(true); // malformed detail — leave it for a human, don't guess
        };
        if record.node_ids.len() < 2 {
            return Ok(true);
        }
        for node_id in &record.node_ids {
            let Some(node) = self.get_node(node_id).await? else {
                return Ok(false);
            };
            let still_named = crate::governance::participates(&node)
                && self
                    .type_is_a(&node.node_type, crate::models::CoreNodeType::Collection)
                    .await?
                && crate::services::collection_service::normalize_collection_name(&node.content)
                    == name;
            if !still_named {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod scan_limit_tests {
    use super::*;
    use crate::db::SqliteStore;
    use crate::models::{Node, NodeUpdate};
    use std::collections::HashSet;
    use std::time::Duration;

    async fn service() -> (NodeService, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(dir.path().join("db")).await.unwrap());
        (NodeService::new(&mut store).await.unwrap(), dir)
    }

    async fn set_status(svc: &NodeService, id: &str, status: &str) {
        let version = svc.get_node(id).await.unwrap().unwrap().version;
        svc.update_node(
            id,
            version,
            NodeUpdate::new().with_lifecycle_status(status.to_string()),
        )
        .await
        .unwrap();
    }

    /// `copies` collections named `name` that collide without any inline
    /// journal: all but the last are created archived and restored afterwards.
    async fn missed_group(svc: &NodeService, name: &str, copies: usize) -> Vec<String> {
        let mut ids = Vec::new();
        for i in 0..copies {
            let id = svc
                .create_node(Node::new(
                    "collection".to_string(),
                    name.to_string(),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            if i + 1 < copies {
                set_status(svc, &id, "archived").await;
            }
            ids.push(id);
        }
        for id in &ids[..copies - 1] {
            set_status(svc, id, "active").await;
        }
        ids
    }

    async fn run(svc: &NodeService, limits: ScanLimits) -> u32 {
        tokio::time::timeout(
            Duration::from_secs(60),
            svc.journal_missed_collisions_within(limits),
        )
        .await
        .expect("the scan terminates")
        .unwrap()
    }

    fn limits(chunk: u32, group_members: u32, max_journaled: u32) -> ScanLimits {
        ScanLimits {
            chunk,
            group_members,
            max_journaled,
        }
    }

    /// More groups than one chunk: paging reaches every group and ends, even
    /// with a chunk of one (a paging comparator that re-reads the last value
    /// would never end).
    #[tokio::test]
    async fn paging_across_chunks_journals_every_group_and_terminates() {
        for chunk in [1, 2] {
            let (svc, _dir) = service().await;
            for i in 0..5 {
                missed_group(&svc, &format!("group{i}"), 2).await;
            }
            assert_eq!(run(&svc, limits(chunk, 20, 500)).await, 5, "chunk {chunk}");
            assert_eq!(run(&svc, limits(chunk, 20, 500)).await, 0, "chunk {chunk}");
        }
    }

    /// A group of three is journaled as pairs of its lowest id with each other
    /// member, so every member is covered.
    #[tokio::test]
    async fn a_group_of_three_has_every_member_covered() {
        let (svc, _dir) = service().await;
        let ids = missed_group(&svc, "trio", 3).await;
        assert_eq!(run(&svc, limits(200, 20, 500)).await, 2);
        let mut covered = HashSet::new();
        for id in &ids {
            if !svc.conflicts_for_node(id).await.unwrap().is_empty() {
                covered.insert(id.clone());
            }
        }
        assert_eq!(covered, ids.into_iter().collect::<HashSet<_>>());
        assert_eq!(run(&svc, limits(200, 20, 500)).await, 0);
    }

    /// The scan stops at the cap and the next sweep continues where it left off.
    #[tokio::test]
    async fn the_cap_stops_a_sweep_and_the_next_continues() {
        let (svc, _dir) = service().await;
        for i in 0..5 {
            missed_group(&svc, &format!("group{i}"), 2).await;
        }
        let capped = limits(200, 20, 2);
        assert_eq!(run(&svc, capped).await, 2);
        assert_eq!(run(&svc, capped).await, 2);
        assert_eq!(run(&svc, capped).await, 1);
        assert_eq!(run(&svc, capped).await, 0);
        assert_eq!(svc.list_conflicts(None, None, None).await.unwrap().len(), 5);
    }

    /// Only the first `group_members` members of a larger group are examined.
    #[tokio::test]
    async fn a_group_beyond_the_member_limit_is_journaled_for_its_first_members() {
        let (svc, _dir) = service().await;
        missed_group(&svc, "crowd", 4).await;
        assert_eq!(run(&svc, limits(200, 3, 500)).await, 2);
    }
}
