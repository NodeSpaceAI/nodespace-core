//! tonic `ImportService` implementation backed by `nodespace-core`.
//!
//! Preserves the two-phase pipeline from `commands/import.rs`:
//!   Phase 1 — file reads (`tokio::fs`), markdown parsing and link resolution
//!             (CPU-bound, on the blocking pool), in bounded chunks
//!   Phase 2 — DB writes, collection assignment, mention creation (async background)
//!
//! Progress events are streamed back to the caller via a tokio channel that
//! bridges the background task to the tonic server-streaming response.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nodespace_core::markdown::{
    prepare_nodes_from_markdown, transform_links_in_nodes_with_mentions, PreparedNode,
};
use nodespace_core::services::{CollectionService, NodeService as CoreNodeService};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::nodespace::{
    import_service_server::ImportService as GrpcImportService, FileImportResult,
    ImportMarkdownFilesRequest, ImportMarkdownRequest, ImportOptions, ImportProgressEvent,
};

const CHANNEL_BUFFER: usize = 64;

/// Maximum files accepted by one `ImportMarkdownFiles` call.
///
/// Every file of a batch import is parsed into memory and written in one bulk
/// insert, so the batch size bounds both the daemon's peak memory and how long
/// that single write holds the store. The cap is deliberately far above real
/// usage — the project's own docs import is a few hundred files and large
/// personal vaults run to low thousands — so it only rejects absurd requests
/// (a mistaken import of a home directory or a dependency tree) with an
/// actionable error rather than a daemon that stalls or runs out of memory.
pub(crate) const MAX_IMPORT_FILES: usize = 5_000;

/// Files read and parsed together in phase 1 of a batch import. Raw file
/// contents are held only for their own chunk and dropped once parsed, so a
/// large import never keeps every source file in memory at once, and each
/// chunk's CPU-bound parse runs in a single blocking-pool task.
const PHASE1_CHUNK_SIZE: usize = 32;

/// Stable namespace for deriving deterministic import root ids from a document's
/// identity key (its base-directory-relative path). Re-importing the same file
/// therefore addresses the same root node, which is what makes re-import
/// idempotent (no duplicate documents, no orphan "ghost" roots). This UUID is
/// arbitrary but MUST stay fixed — changing it re-keys every imported document.
const IMPORT_ROOT_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x9a, 0x1d, 0x2b, 0x7c, 0x4e, 0x63, 0x5f, 0x81, 0xa2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29,
]);

/// Derive a document's root node id deterministically from its identity key.
/// The key is the file's base-directory-relative path, so the same file yields
/// the same root id across imports regardless of the absolute checkout path.
fn deterministic_root_id(key: &str) -> String {
    uuid::Uuid::new_v5(&IMPORT_ROOT_NAMESPACE, key.as_bytes()).to_string()
}

#[derive(Clone)]
pub struct ImportServiceImpl {
    node_service: Arc<CoreNodeService>,
}

impl ImportServiceImpl {
    pub fn new(node_service: Arc<CoreNodeService>) -> Self {
        Self { node_service }
    }

    /// Resolve which database this request targets (ADR-053) and return that
    /// database's import service. The routing contract lives in
    /// [`crate::db_routing::routed_database_services`]: a header selects a
    /// registered database, header-less requests hit the default, and with no
    /// routing middleware installed a header-less request falls back to `self`
    /// while a header-carrying one is rejected.
    async fn route<T>(&self, request: &Request<T>) -> Result<ImportServiceImpl, Status> {
        crate::db_routing::route_or_self(self, request, |s| &s.import).await
    }
}

// ---------------------------------------------------------------------------
// tonic service trait
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl GrpcImportService for ImportServiceImpl {
    type ImportMarkdownStream = ReceiverStream<Result<ImportProgressEvent, Status>>;
    type ImportMarkdownFilesStream = ReceiverStream<Result<ImportProgressEvent, Status>>;

    async fn import_markdown(
        &self,
        request: Request<ImportMarkdownRequest>,
    ) -> Result<Response<Self::ImportMarkdownStream>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let opts = req.options.unwrap_or_default();
        let node_service = Arc::clone(&this.node_service);

        let (tx, rx) = mpsc::channel(CHANNEL_BUFFER);

        tokio::spawn(async move {
            run_single_file_import(node_service, req.file_path, opts, tx).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn import_markdown_files(
        &self,
        request: Request<ImportMarkdownFilesRequest>,
    ) -> Result<Response<Self::ImportMarkdownFilesStream>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        if let Some(status) = oversized_batch_error(req.file_paths.len()) {
            return Err(status);
        }
        let opts = req.options.unwrap_or_default();
        let node_service = Arc::clone(&this.node_service);

        let (tx, rx) = mpsc::channel(CHANNEL_BUFFER);

        tokio::spawn(async move {
            run_batch_import(node_service, req.file_paths, opts, tx).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// ---------------------------------------------------------------------------
// Single-file import
// ---------------------------------------------------------------------------

async fn run_single_file_import(
    node_service: Arc<CoreNodeService>,
    file_path: String,
    opts: ImportOptions,
    tx: mpsc::Sender<Result<ImportProgressEvent, Status>>,
) {
    let path = PathBuf::from(&file_path);

    let result = import_single_file(&node_service, &path, &opts).await;

    let _ = tx
        .send(Ok(ImportProgressEvent {
            step: 9,
            step_name: "complete".to_string(),
            message: if result.success {
                format!(
                    "Imported {} ({} nodes)",
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&file_path),
                    result.nodes_created
                )
            } else {
                format!(
                    "Failed: {}",
                    result.error.as_deref().unwrap_or("unknown error")
                )
            },
            current: 1,
            total: 1,
            results: vec![proto_file_result(result)],
        }))
        .await;
}

async fn import_single_file(
    node_service: &CoreNodeService,
    path: &Path,
    opts: &ImportOptions,
) -> LocalFileImportResult {
    let content = match read_import_file(path).await {
        Ok(c) => c,
        Err(msg) => return LocalFileImportResult::error(path.to_string_lossy().to_string(), &msg),
    };

    let (collection, is_archived) = if opts.auto_collection_routing {
        let base_dir = if opts.base_directory.is_empty() {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            PathBuf::from(&opts.base_directory)
        };
        let meta = derive_collection_metadata(path, &base_dir);
        (Some(meta.collection), meta.is_archived)
    } else if !opts.collection.is_empty() {
        (Some(opts.collection.clone()), false)
    } else {
        (None, false)
    };

    let title = if opts.use_filename_as_title {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    } else {
        content
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    };

    // Deterministic root id from the file's identity key (base-dir-relative
    // path when a base is set, else the file path), so a re-import addresses
    // the same document root.
    let import_key = if opts.base_directory.is_empty() {
        path.to_string_lossy().to_string()
    } else {
        path.strip_prefix(&opts.base_directory)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    };
    let root_id = deterministic_root_id(&import_key);

    match import_markdown_content(
        node_service,
        &root_id,
        &title,
        &content,
        is_archived,
        opts.replace,
    )
    .await
    {
        Ok((root_id, nodes_created)) => {
            if let Some(ref coll) = collection {
                let collection_service = CollectionService::new(node_service.store(), node_service);
                if let Err(e) = collection_service
                    .add_to_collection_by_path(&root_id, coll)
                    .await
                {
                    return LocalFileImportResult {
                        file_path: path.to_string_lossy().to_string(),
                        root_id: Some(root_id),
                        nodes_created,
                        success: true,
                        error: Some(format!("Imported but failed to add to collection: {}", e)),
                        collection,
                        archived: is_archived,
                    };
                }
            }
            LocalFileImportResult {
                file_path: path.to_string_lossy().to_string(),
                root_id: Some(root_id),
                nodes_created,
                success: true,
                error: None,
                collection,
                archived: is_archived,
            }
        }
        Err(e) => LocalFileImportResult::error(path.to_string_lossy().to_string(), &e),
    }
}

// ---------------------------------------------------------------------------
// Batch import (two-phase pipeline)
// ---------------------------------------------------------------------------

async fn run_batch_import(
    node_service: Arc<CoreNodeService>,
    file_paths: Vec<String>,
    opts: ImportOptions,
    tx: mpsc::Sender<Result<ImportProgressEvent, Status>>,
) {
    let total_files = file_paths.len();

    let base_dir = if !opts.base_directory.is_empty() {
        PathBuf::from(&opts.base_directory)
    } else {
        file_paths
            .first()
            .map(|p| {
                PathBuf::from(p)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf()
            })
            .unwrap_or_else(|| PathBuf::from("."))
    };

    // ========================================================================
    // PHASE 1: Read, parse and link-resolve every file (in memory)
    // ========================================================================

    send_progress(
        &tx,
        1,
        "scanning",
        "Scanning folder...",
        0,
        total_files,
        vec![],
    )
    .await;

    let mut prepared_files: Vec<PreparedFileImport> = Vec::new();
    let mut failed_results: Vec<LocalFileImportResult> = Vec::new();
    let mut parsed_count = 0usize;

    // Read and parse in bounded chunks: reads go through `tokio::fs` so they
    // never park a runtime worker, and each chunk's raw contents are dropped as
    // soon as they are parsed. Link resolution below still runs once over every
    // prepared file, so cross-file links resolve regardless of chunk boundaries.
    for (chunk_index, chunk) in file_paths.chunks(PHASE1_CHUNK_SIZE).enumerate() {
        let mut chunk_reads: Vec<FileReadResult> = Vec::with_capacity(chunk.len());

        for (offset, file_path) in chunk.iter().enumerate() {
            let path = PathBuf::from(file_path);
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file_path)
                .to_string();

            send_progress(
                &tx,
                2,
                "reading",
                &format!("Reading: {}", filename),
                chunk_index * PHASE1_CHUNK_SIZE + offset + 1,
                total_files,
                vec![],
            )
            .await;

            let content = match read_import_file(&path).await {
                Ok(content) => content,
                Err(msg) => {
                    failed_results.push(LocalFileImportResult::error(file_path.clone(), &msg));
                    continue;
                }
            };

            let (collection_path, is_archived) = if opts.auto_collection_routing {
                let meta = derive_collection_metadata(&path, &base_dir);
                (Some(meta.collection), meta.is_archived)
            } else if !opts.collection.is_empty() {
                (Some(opts.collection.clone()), false)
            } else {
                (None, false)
            };
            let relative_path = path
                .strip_prefix(&base_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            chunk_reads.push(FileReadResult {
                path,
                content,
                relative_path,
                collection_path,
                is_archived,
            });
        }

        if chunk_reads.is_empty() {
            continue;
        }

        // Markdown parsing is CPU-bound and scales with file size, so it runs on
        // the blocking pool rather than an async worker shared with other RPCs.
        let chunk_files: Vec<String> = chunk_reads
            .iter()
            .map(|f| f.path.to_string_lossy().to_string())
            .collect();
        let use_filename_as_title = opts.use_filename_as_title;
        let parsed = tokio::task::spawn_blocking(move || {
            chunk_reads
                .into_iter()
                .map(|file_read| prepare_file_import(file_read, use_filename_as_title))
                .collect::<Vec<_>>()
        })
        .await;

        match parsed {
            Ok(outcomes) => {
                for (filename, outcome) in outcomes {
                    parsed_count += 1;
                    send_progress(
                        &tx,
                        3,
                        "parsing",
                        &format!("Parsed: {}", filename),
                        parsed_count,
                        total_files,
                        vec![],
                    )
                    .await;
                    match outcome {
                        Ok(prepared) => prepared_files.push(prepared),
                        Err(failed) => failed_results.push(failed),
                    }
                }
            }
            Err(join_err) => {
                // A parser panic takes the whole chunk with it; report each of
                // its files as failed instead of silently dropping them.
                tracing::error!("Markdown parse task aborted: {join_err}");
                parsed_count += chunk_files.len();
                failed_results.extend(chunk_files.into_iter().map(|f| {
                    LocalFileImportResult::error(
                        f,
                        &format!("Failed to parse markdown: parser aborted ({join_err})"),
                    )
                }));
            }
        }
    }

    send_progress(
        &tx,
        4,
        "resolving",
        "Resolving internal links...",
        0,
        prepared_files.len(),
        vec![],
    )
    .await;

    // Resolve links across the whole import (one global file→root-id index),
    // off the async workers: it rewrites every node's content.
    let (prepared_files, all_mentions) =
        match tokio::task::spawn_blocking(move || resolve_import_links(prepared_files)).await {
            Ok(resolved) => resolved,
            Err(join_err) => {
                tracing::error!("Import link resolution aborted: {join_err}");
                let _ = tx
                    .send(Err(Status::internal(format!(
                        "import link resolution aborted unexpectedly: {join_err}"
                    ))))
                    .await;
                return;
            }
        };

    // Collect unique collection paths
    let unique_collections: Vec<String> = prepared_files
        .iter()
        .filter_map(|f| f.collection_path.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // Build Phase 1 results (success entries; failed_results holds failures)
    let mut phase1_results: Vec<LocalFileImportResult> = prepared_files
        .iter()
        .map(|p| LocalFileImportResult {
            file_path: p.file_path.to_string_lossy().to_string(),
            root_id: Some(p.root_id.clone()),
            nodes_created: 1 + p.children.len(),
            success: true,
            error: None,
            collection: p.collection_path.clone(),
            archived: p.is_archived,
        })
        .collect();

    // ========================================================================
    // PHASE 2: DB operations (background task, results streamed back via tx)
    // ========================================================================

    let unique_collections_count = unique_collections.len();
    let all_mentions_count = all_mentions.len();
    let replace = opts.replace;
    let store = Arc::clone(node_service.store());
    let node_service_clone = (*node_service).clone();
    let tx_bg = tx.clone();
    let tx_guard = tx.clone();

    let phase2 = tokio::spawn(async move {
        // First failure seen in phase 2. When set, the import did NOT fully
        // succeed (nodes may exist but be unlinked from collections, or mentions
        // may be missing), so every file result is marked failed rather than
        // reporting a false "complete".
        let mut phase2_error: Option<String> = None;

        send_progress(
            &tx_bg,
            5,
            "collections",
            &format!("Creating {} collections...", unique_collections_count),
            0,
            unique_collections_count,
            vec![],
        )
        .await;

        let collection_service = CollectionService::new(&store, &node_service_clone);
        let collection_map = match collection_service
            .bulk_resolve_collections(&unique_collections)
            .await
        {
            Ok(map) => map,
            Err(e) => {
                tracing::error!("Failed to bulk resolve collections: {:?}", e);
                phase2_error.get_or_insert_with(|| format!("Collection resolution failed: {e}"));
                HashMap::new()
            }
        };

        // Idempotent re-import: a document's root id is deterministic, so a
        // matching id already in the store means this file was imported before.
        // `--replace` refreshes it in place; without it, the existing document
        // is left untouched (skipped) so a plain re-import never duplicates.
        let root_ids: Vec<String> = prepared_files.iter().map(|p| p.root_id.clone()).collect();
        let existing_roots: std::collections::HashSet<String> =
            match store.get_nodes_by_ids(&root_ids).await {
                Ok(map) => map.into_keys().collect(),
                Err(e) => {
                    tracing::error!("Failed to check for existing roots: {:?}", e);
                    phase2_error.get_or_insert_with(|| format!("Existing-root check failed: {e}"));
                    std::collections::HashSet::new()
                }
            };

        let mut all_nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )> = Vec::new();
        let mut collection_assignments: Vec<(String, String)> = Vec::new();
        let mut replaced_roots: Vec<String> = Vec::new();
        let mut skipped_roots: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut new_docs: usize = 0;
        // Old child ids of replaced docs, pruned only AFTER the fresh subtree is
        // inserted so a failed insert never destroys the previous content (the
        // bulk insert is not transactional). See the prune step below.
        let mut prune_after_insert: Vec<String> = Vec::new();
        // Same deterministic root id can appear twice in one batch (a duplicated
        // or symlinked file); handle each document once so we never double-insert
        // or double-prune.
        let mut seen_roots: std::collections::HashSet<String> = std::collections::HashSet::new();

        for prepared in &prepared_files {
            if !seen_roots.insert(prepared.root_id.clone()) {
                continue;
            }
            let exists = existing_roots.contains(&prepared.root_id);

            if exists && !replace {
                // Already imported and not refreshing: skip creating anything.
                // The root stays a valid link target for other docs; only its
                // (idempotent) collection membership is re-asserted below so a
                // doc that lost its edge in a prior partial import self-heals.
                skipped_roots.insert(prepared.root_id.clone());
            } else if exists && replace {
                // Refresh in place. Update the root now (non-destructive) so its
                // id and inbound links/mentions survive, and capture its current
                // children to prune only after the fresh subtree is inserted.
                let update =
                    nodespace_core::NodeUpdate::new().with_content(prepared.root_content.clone());
                if let Err(e) = store
                    .update_node(&prepared.root_id, update, Some("import".to_string()))
                    .await
                {
                    tracing::warn!("Failed to refresh root {}: {}", prepared.root_id, e);
                }
                match node_service_clone.get_descendants(&prepared.root_id).await {
                    Ok(desc) => prune_after_insert.extend(desc.into_iter().map(|n| n.id)),
                    Err(e) => {
                        tracing::error!(
                            "Failed to read existing subtree for {}: {:?}",
                            prepared.root_id,
                            e
                        );
                        phase2_error.get_or_insert_with(|| format!("Subtree replace failed: {e}"));
                    }
                }
                replaced_roots.push(prepared.root_id.clone());
            } else {
                // New document: create the root node.
                let mut root_props = serde_json::json!({});
                if prepared.is_archived {
                    root_props["lifecycle_status"] = serde_json::json!("archived");
                }
                all_nodes.push((
                    prepared.root_id.clone(),
                    "header".to_string(),
                    prepared.root_content.clone(),
                    None,
                    1.0,
                    root_props,
                ));
                new_docs += 1;
            }

            // Children are (re)created for new and replaced roots. Skipped roots
            // keep their existing subtree, so contribute no children here.
            if !exists || replace {
                for child in &prepared.children {
                    let parent = child
                        .parent_id
                        .clone()
                        .or_else(|| Some(prepared.root_id.clone()));
                    all_nodes.push((
                        child.id.clone(),
                        child.node_type.clone(),
                        child.content.clone(),
                        parent,
                        child.order,
                        child.properties.clone(),
                    ));
                }
            }

            // Collection membership is idempotent (existing edges are skipped),
            // so re-asserting it for every file — new, replaced, or skipped —
            // both wires up new docs and repairs any that lost their edge.
            if let Some(ref coll_path) = prepared.collection_path {
                if let Some(coll_id) = collection_map.get(coll_path) {
                    collection_assignments.push((prepared.root_id.clone(), coll_id.clone()));
                }
            }
        }

        send_progress(
            &tx_bg,
            6,
            "importing",
            &format!("Importing {} nodes...", all_nodes.len()),
            0,
            all_nodes.len(),
            vec![],
        )
        .await;

        let nodes_written = all_nodes.len();
        let bulk_insert_failed = match node_service_clone
            .bulk_create_hierarchy_trusted(all_nodes)
            .await
        {
            Ok(ids) => {
                tracing::info!("Bulk created {} nodes", ids.len());
                false
            }
            Err(e) => {
                tracing::error!("Failed to bulk create nodes: {:?}", e);
                phase2_error.get_or_insert_with(|| {
                    "Bulk node insertion failed; see daemon logs".to_string()
                });
                true
            }
        };

        send_progress(
            &tx_bg,
            7,
            "assigning",
            "Assigning to collections...",
            0,
            collection_assignments.len(),
            vec![],
        )
        .await;

        if !bulk_insert_failed && !collection_assignments.is_empty() {
            // Route through the notifying variant so each new `member_of` edge
            // emits a RelationshipCreated event and thus PUSHES to cloud. The raw
            // `store.bulk_add_to_collections` writes the rows but emits nothing, so
            // the memberships would land only in the local DB — every other device
            // (and any first-time puller) would then see these collections empty.
            match node_service_clone
                .bulk_add_to_collections_notify(&collection_assignments)
                .await
            {
                Ok(count) => tracing::info!("Bulk assigned {} collection memberships", count),
                Err(e) => {
                    tracing::error!("Failed to bulk add to collections: {:?}", e);
                    phase2_error
                        .get_or_insert_with(|| format!("Collection assignment failed: {e}"));
                }
            }
        }

        send_progress(
            &tx_bg,
            8,
            "references",
            &format!("Creating {} references...", all_mentions_count),
            0,
            all_mentions_count,
            vec![],
        )
        .await;

        if !bulk_insert_failed && !all_mentions.is_empty() {
            match store.bulk_create_mentions(&all_mentions).await {
                Ok(count) => tracing::info!("Bulk created {} mentions", count),
                Err(e) => {
                    tracing::error!("Failed to bulk create mentions: {:?}", e);
                    phase2_error.get_or_insert_with(|| format!("Reference creation failed: {e}"));
                }
            }
        }

        if !bulk_insert_failed {
            for prepared in &prepared_files {
                if prepared.is_archived {
                    if let Err(e) = store
                        .update_lifecycle_status(&prepared.root_id, "archived")
                        .await
                    {
                        tracing::warn!(
                            "Failed to set lifecycle_status for {}: {}",
                            prepared.root_id,
                            e
                        );
                    }
                }
            }
        }

        // Now that the fresh subtrees are inserted, prune each replaced doc's
        // OLD children. Deferring the delete to here (rather than before the
        // insert) means a failed bulk insert leaves the previous content intact
        // instead of truncating it — re-import is never destructive on error.
        if !bulk_insert_failed && !prune_after_insert.is_empty() {
            if let Err(e) = store
                .delete_nodes_by_ids_unchecked(&prune_after_insert)
                .await
            {
                tracing::error!(
                    "Failed to prune {} stale node(s) after replace: {}",
                    prune_after_insert.len(),
                    e
                );
                phase2_error.get_or_insert_with(|| format!("Stale subtree prune failed: {e}"));
            }
        }

        // A replaced root keeps its id but gets a fresh child subtree, so its
        // content effectively changed — re-mark it stale so it re-embeds. New
        // roots already get their markers inside bulk_create_hierarchy_trusted.
        if !bulk_insert_failed && !replaced_roots.is_empty() {
            if let Err(e) = store
                .create_stale_embedding_markers_bulk(&replaced_roots)
                .await
            {
                tracing::warn!(
                    "Failed to re-mark {} replaced root(s) stale: {}",
                    replaced_roots.len(),
                    e
                );
            }
        }

        // Skipped documents (already present, no --replace) created no nodes.
        // Reflect that in their per-file result so the count is honest.
        for r in phase1_results.iter_mut() {
            if let Some(ref rid) = r.root_id {
                if skipped_roots.contains(rid) {
                    r.nodes_created = 0;
                }
            }
        }

        // Fold any phase-2 failure into the per-file results so the caller sees a
        // non-success outcome instead of a false "complete" — the disease behind
        // "nodes land with zero member_of edges while the CLI reports success".
        apply_phase2_error(&mut phase1_results, &phase2_error);

        let mut all_results: Vec<LocalFileImportResult> = failed_results;
        all_results.append(&mut phase1_results);
        let proto_results: Vec<FileImportResult> =
            all_results.into_iter().map(proto_file_result).collect();

        let message = match &phase2_error {
            Some(err) => format!("Import completed with errors: {err}"),
            None => {
                let mut summary = format!("Imported {new_docs} files ({nodes_written} nodes)");
                if !replaced_roots.is_empty() {
                    summary.push_str(&format!(", {} refreshed", replaced_roots.len()));
                }
                if !skipped_roots.is_empty() {
                    summary.push_str(&format!(", {} already present", skipped_roots.len()));
                }
                summary
            }
        };
        send_progress(
            &tx_bg,
            9,
            "complete",
            &message,
            total_files,
            total_files,
            proto_results,
        )
        .await;
    });

    // A panic or cancellation in phase 2 aborts the task before it can send its
    // terminal event, which would otherwise leave the stream to simply end after
    // the last progress message — indistinguishable from success to the CLI.
    // Await the handle and, on abnormal termination, close the stream with an
    // error so the failure is never silent.
    tokio::spawn(async move {
        if let Err(join_err) = phase2.await {
            tracing::error!("Import phase 2 aborted: {join_err}");
            let _ = tx_guard
                .send(Err(Status::internal(format!(
                    "import phase 2 aborted unexpectedly: {join_err}"
                ))))
                .await;
        }
    });
}

/// Fold a phase-2 failure into every per-file result. When phase 2 fails partway
/// (collections unresolved, membership or mention writes rejected, or the bulk
/// node insert failed), the files are not fully imported — so their results must
/// report failure rather than a false success. No-op when `error` is `None`.
fn apply_phase2_error(results: &mut [LocalFileImportResult], error: &Option<String>) {
    if let Some(err) = error {
        for r in results {
            r.success = false;
            r.error = Some(err.clone());
        }
    }
}

/// The rejection for a batch import larger than [`MAX_IMPORT_FILES`], checked
/// before any work starts; `None` when the batch is within the limit.
fn oversized_batch_error(file_count: usize) -> Option<Status> {
    (file_count > MAX_IMPORT_FILES).then(|| {
        Status::invalid_argument(format!(
            "Import batch exceeds maximum of {MAX_IMPORT_FILES} files (got {file_count}); \
             import sub-folders separately or narrow the selection with exclude patterns"
        ))
    })
}

/// Read one import source through `tokio::fs`, so the read runs on the blocking
/// pool instead of parking an async worker. Errors are user-facing messages.
async fn read_import_file(path: &Path) -> Result<String, String> {
    match tokio::fs::metadata(path).await {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err("Path is not a file".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("File does not exist".to_string());
        }
        Err(e) => return Err(format!("Failed to read file: {e}")),
    }
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("Failed to read file: {e}"))
}

/// Parse one read file into its import root and child nodes. Synchronous and
/// CPU-bound — call from the blocking pool. Returns the display filename with
/// the outcome so the caller can report progress.
fn prepare_file_import(
    file_read: FileReadResult,
    use_filename_as_title: bool,
) -> (String, Result<PreparedFileImport, LocalFileImportResult>) {
    let filename = file_read
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&file_read.relative_path)
        .to_string();

    let title = if use_filename_as_title {
        file_read
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    } else {
        file_read
            .content
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    };

    let root_id = deterministic_root_id(&file_read.relative_path);

    let root_content = if title.starts_with('#') {
        title.clone()
    } else {
        format!("# {}", title)
    };

    let content_for_children = {
        let first_line = file_read.content.lines().find(|l| !l.trim().is_empty());
        if first_line == Some(&title) {
            let lines: Vec<&str> = file_read.content.lines().collect();
            let first_idx = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
            lines[first_idx + 1..].join("\n")
        } else {
            file_read.content
        }
    };

    let outcome = match prepare_nodes_from_markdown(&content_for_children, Some(root_id.clone())) {
        Ok(children) => Ok(PreparedFileImport {
            file_path: file_read.path,
            root_id,
            root_content,
            is_archived: file_read.is_archived,
            collection_path: file_read.collection_path,
            children,
        }),
        Err(e) => Err(LocalFileImportResult::error(
            file_read.path.to_string_lossy().to_string(),
            &format!("Failed to parse markdown: {:?}", e),
        )),
    };
    (filename, outcome)
}

/// Rewrite links between imported files to `nodespace://` references and
/// collect the resulting mentions. The file→root-id index spans every prepared
/// file of the import, so links resolve across the whole batch. Synchronous and
/// CPU-bound — call from the blocking pool.
fn resolve_import_links(
    mut prepared_files: Vec<PreparedFileImport>,
) -> (Vec<PreparedFileImport>, Vec<(String, String)>) {
    let file_to_uuid_map: HashMap<PathBuf, String> = prepared_files
        .iter()
        .map(|f| (f.file_path.clone(), f.root_id.clone()))
        .collect();

    let mut all_mentions: Vec<(String, String)> = Vec::new();
    for prepared in &mut prepared_files {
        let result = transform_links_in_nodes_with_mentions(
            &mut prepared.children,
            &file_to_uuid_map,
            Some(&prepared.file_path),
            &prepared.root_id,
        );
        all_mentions.extend(result.mentions);
    }
    (prepared_files, all_mentions)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn send_progress(
    tx: &mpsc::Sender<Result<ImportProgressEvent, Status>>,
    step: u32,
    step_name: &str,
    message: &str,
    current: usize,
    total: usize,
    results: Vec<FileImportResult>,
) {
    let _ = tx
        .send(Ok(ImportProgressEvent {
            step,
            step_name: step_name.to_string(),
            message: message.to_string(),
            current: current as u32,
            total: total as u32,
            results,
        }))
        .await;
}

fn proto_file_result(r: LocalFileImportResult) -> FileImportResult {
    FileImportResult {
        file_path: r.file_path,
        root_id: r.root_id.unwrap_or_default(),
        nodes_created: r.nodes_created as u32,
        success: r.success,
        error: r.error.unwrap_or_default(),
        collection: r.collection.unwrap_or_default(),
        archived: r.archived,
    }
}

// ---------------------------------------------------------------------------
// Smart collection routing (ported from commands/import.rs)
// ---------------------------------------------------------------------------

struct CollectionMetadata {
    collection: String,
    is_archived: bool,
}

fn derive_collection_metadata(file_path: &Path, base_dir: &Path) -> CollectionMetadata {
    let relative = file_path.strip_prefix(base_dir).unwrap_or(file_path);
    let path_str = relative.to_string_lossy().to_lowercase();
    let segments: Vec<&str> = relative
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    if path_str.contains("/archived/")
        || segments.iter().any(|s| s.eq_ignore_ascii_case("archived"))
    {
        return CollectionMetadata {
            collection: "Archived".to_string(),
            is_archived: true,
        };
    }

    if path_str.contains("/decisions/") || path_str.contains("/adr/") {
        return CollectionMetadata {
            collection: "ADR".to_string(),
            is_archived: false,
        };
    }

    if path_str.contains("/lessons/") || segments.iter().any(|s| s.eq_ignore_ascii_case("lessons"))
    {
        return CollectionMetadata {
            collection: "Lessons".to_string(),
            is_archived: false,
        };
    }

    if segments
        .first()
        .map(|s| s.eq_ignore_ascii_case("troubleshooting"))
        .unwrap_or(false)
    {
        return CollectionMetadata {
            collection: "Troubleshooting".to_string(),
            is_archived: false,
        };
    }

    if segments
        .first()
        .map(|s| s.eq_ignore_ascii_case("architecture"))
        .unwrap_or(false)
    {
        let sub_segments: Vec<&str> = segments.iter().skip(1).copied().collect();

        if sub_segments
            .first()
            .map(|s| s.eq_ignore_ascii_case("components"))
            .unwrap_or(false)
        {
            return CollectionMetadata {
                collection: "Components".to_string(),
                is_archived: false,
            };
        }

        if sub_segments
            .first()
            .map(|s| s.eq_ignore_ascii_case("business-logic"))
            .unwrap_or(false)
        {
            return CollectionMetadata {
                collection: "Business Logic".to_string(),
                is_archived: false,
            };
        }

        if sub_segments
            .first()
            .map(|s| s.eq_ignore_ascii_case("development"))
            .unwrap_or(false)
        {
            let dev_sub: Vec<&str> = sub_segments.iter().skip(1).copied().collect();
            if !dev_sub.is_empty() {
                let nested = dev_sub
                    .iter()
                    .map(|s| to_title_case(s))
                    .collect::<Vec<_>>()
                    .join(":");
                return CollectionMetadata {
                    collection: format!("Development:{}", nested),
                    is_archived: false,
                };
            }
            return CollectionMetadata {
                collection: "Development".to_string(),
                is_archived: false,
            };
        }

        if sub_segments
            .first()
            .map(|s| s.eq_ignore_ascii_case("core"))
            .unwrap_or(false)
        {
            return CollectionMetadata {
                collection: "Architecture:Core".to_string(),
                is_archived: false,
            };
        }

        if !sub_segments.is_empty() {
            let arch_sub = sub_segments
                .iter()
                .map(|s| to_title_case(s))
                .collect::<Vec<_>>()
                .join(":");
            return CollectionMetadata {
                collection: format!("Architecture:{}", arch_sub),
                is_archived: false,
            };
        }

        return CollectionMetadata {
            collection: "Architecture".to_string(),
            is_archived: false,
        };
    }

    if segments
        .first()
        .map(|s| s.eq_ignore_ascii_case("performance"))
        .unwrap_or(false)
    {
        return CollectionMetadata {
            collection: "Performance".to_string(),
            is_archived: false,
        };
    }

    if segments
        .first()
        .map(|s| s.eq_ignore_ascii_case("testing"))
        .unwrap_or(false)
    {
        return CollectionMetadata {
            collection: "Testing".to_string(),
            is_archived: false,
        };
    }

    if segments.is_empty() {
        return CollectionMetadata {
            collection: "Docs".to_string(),
            is_archived: false,
        };
    }

    let collection = segments
        .iter()
        .map(|s| to_title_case(s))
        .collect::<Vec<_>>()
        .join(":");

    CollectionMetadata {
        collection,
        is_archived: false,
    }
}

fn to_title_case(s: &str) -> String {
    s.split(['-', '_'])
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Internal data types (not exposed over wire)
// ---------------------------------------------------------------------------

struct PreparedFileImport {
    file_path: PathBuf,
    root_id: String,
    root_content: String,
    is_archived: bool,
    collection_path: Option<String>,
    children: Vec<PreparedNode>,
}

struct FileReadResult {
    path: PathBuf,
    content: String,
    relative_path: String,
    collection_path: Option<String>,
    is_archived: bool,
}

struct LocalFileImportResult {
    file_path: String,
    root_id: Option<String>,
    nodes_created: usize,
    success: bool,
    error: Option<String>,
    collection: Option<String>,
    archived: bool,
}

impl LocalFileImportResult {
    fn error(file_path: String, msg: &str) -> Self {
        Self {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some(msg.to_string()),
            collection: None,
            archived: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Markdown content importer (from commands/import.rs)
// ---------------------------------------------------------------------------

async fn import_markdown_content(
    node_service: &CoreNodeService,
    root_id: &str,
    title: &str,
    content: &str,
    is_archived: bool,
    replace: bool,
) -> Result<(String, usize), String> {
    use nodespace_core::services::CreateNodeParams;

    let clean_title = if title.starts_with('#') {
        title.to_string()
    } else {
        format!("# {}", title)
    };

    let content_for_children = {
        let first_line = content.lines().find(|l| !l.trim().is_empty());
        if first_line == Some(title) {
            let lines: Vec<&str> = content.lines().collect();
            let first_idx = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
            lines[first_idx + 1..].join("\n")
        } else {
            content.to_string()
        }
    };

    // Parsing is CPU-bound and scales with file size; keep it off the async
    // workers shared with other RPCs.
    let prepared_nodes = tokio::task::spawn_blocking(move || {
        prepare_nodes_from_markdown(&content_for_children, None)
    })
    .await
    .map_err(|e| format!("Failed to parse markdown: parser aborted ({e})"))?
    .map_err(|e| format!("Failed to parse markdown: {:?}", e))?;

    // Idempotent re-import: the root id is deterministic, so an existing node
    // means this document was imported before. `--replace` refreshes it;
    // otherwise leave it untouched so a plain re-import never duplicates.
    let exists = node_service
        .get_node(root_id)
        .await
        .map_err(|e| format!("Failed to check existing document: {}", e))?
        .is_some();

    if exists && !replace {
        return Ok((root_id.to_string(), 0));
    }

    let mut nodes_created = 0;

    // Old child ids captured before insert; pruned only after the fresh subtree
    // lands, so a failed insert leaves the previous content intact.
    let mut prune_after_insert: Vec<String> = Vec::new();

    if exists {
        // Refresh in place: keep + update the root (non-destructive) so inbound
        // links/mentions survive, and capture its current children to prune only
        // after the fresh subtree is inserted below.
        let update = nodespace_core::NodeUpdate::new().with_content(clean_title);
        node_service
            .store()
            .update_node(root_id, update, Some("import".to_string()))
            .await
            .map_err(|e| format!("Failed to refresh root node: {}", e))?;
        let descendants = node_service
            .get_descendants(root_id)
            .await
            .map_err(|e| format!("Failed to read existing subtree: {}", e))?;
        prune_after_insert.extend(descendants.into_iter().map(|n| n.id));
    } else {
        let mut properties = serde_json::json!({});
        if is_archived {
            properties["lifecycle_status"] = serde_json::json!("archived");
        }
        node_service
            .create_node_with_parent(CreateNodeParams {
                id: Some(root_id.to_string()),
                node_type: "header".to_string(),
                content: clean_title,
                parent_id: None,
                position: nodespace_core::services::InsertPositionOwned::End,
                properties,
                lifecycle_status: None,
            })
            .await
            .map_err(|e| format!("Failed to create root node: {}", e))?;
        nodes_created += 1;
    }

    if is_archived {
        if let Err(e) = node_service
            .store()
            .update_lifecycle_status(root_id, "archived")
            .await
        {
            tracing::warn!(
                "Failed to set lifecycle_status to archived for {}: {}",
                root_id,
                e
            );
        }
    }

    if !prepared_nodes.is_empty() {
        let nodes_for_bulk: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )> = prepared_nodes
            .iter()
            .map(|n| {
                let parent = n.parent_id.clone().or_else(|| Some(root_id.to_string()));
                (
                    n.id.clone(),
                    n.node_type.clone(),
                    n.content.clone(),
                    parent,
                    n.order,
                    n.properties.clone(),
                )
            })
            .collect();

        let created_ids = node_service
            .bulk_create_hierarchy(nodes_for_bulk)
            .await
            .map_err(|e| format!("Failed to bulk create nodes: {}", e))?;

        nodes_created += created_ids.len();
    }

    // On replace, prune the OLD children now that the fresh subtree is in place,
    // and re-mark the (kept) root stale so it re-embeds with the new content.
    if exists {
        node_service
            .store()
            .delete_nodes_by_ids_unchecked(&prune_after_insert)
            .await
            .map_err(|e| format!("Failed to prune stale subtree: {}", e))?;
        if let Err(e) = node_service
            .store()
            .create_stale_embedding_markers_bulk(&[root_id.to_string()])
            .await
        {
            tracing::warn!("Failed to re-mark replaced root {} stale: {}", root_id, e);
        }
    }

    Ok((root_id.to_string(), nodes_created))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_routing::DATABASE_ID_HEADER;
    use crate::services::database_manager::DatabaseManager;
    use crate::services::SharedContext;
    use nodespace_agent::pty::PtySessionManager;
    use nodespace_core::services::EmbeddingScheduler;
    use nodespace_core::SqliteStore;
    use nodespace_nlp_engine::EmbeddingService;
    use tokio::sync::watch;

    /// Build a bare `NodeService` over a fresh on-disk store. The returned
    /// `TempDir` owns the database file and must be kept alive for the test.
    async fn new_service_and_dir() -> (CoreNodeService, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Arc::new(SqliteStore::new(dir.path().join("db")).await.unwrap());
        let node_service = CoreNodeService::new(&mut store).await.unwrap();
        (node_service, dir)
    }

    /// Drive `run_batch_import` to completion, draining its progress stream
    /// until the terminal step-9 event (which the daemon sends only after every
    /// phase-2 DB write has committed).
    async fn run_batch_and_wait(
        node_service: Arc<CoreNodeService>,
        files: Vec<String>,
        opts: ImportOptions,
    ) -> ImportProgressEvent {
        let (tx, mut rx) = mpsc::channel::<Result<ImportProgressEvent, Status>>(CHANNEL_BUFFER);
        // Drain concurrently: a large import emits more progress events than
        // the channel buffers, so the producer must not wait on this reader.
        let drain = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    Ok(e) if e.step == 9 => return e,
                    Ok(_) => {}
                    Err(status) => panic!("import stream failed: {status}"),
                }
            }
            panic!("import stream ended without a completion event");
        });
        run_batch_import(node_service, files, opts, tx).await;
        drain.await.unwrap()
    }

    fn test_context() -> SharedContext {
        let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
        SharedContext {
            pty_manager: Arc::new(PtySessionManager::new()),
            model,
            has_model: false,
            model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scheduler: Arc::new(EmbeddingScheduler::new()),
            subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
            local_agent: crate::SharedLocalAgent::new(
                crate::nodespace_dir()
                    .expect("nodespace dir")
                    .join("daemon.toml"),
            ),
        }
    }

    fn ok_result(file: &str) -> LocalFileImportResult {
        LocalFileImportResult {
            file_path: file.to_string(),
            root_id: Some(format!("root-{file}")),
            nodes_created: 3,
            success: true,
            error: None,
            collection: Some("docs".to_string()),
            archived: false,
        }
    }

    /// A phase-2 failure must mark every file result failed so the CLI cannot
    /// report a false "complete" while nodes are left unlinked from collections
    /// (the "searchable but unbrowsable" state). No error → results untouched.
    #[test]
    fn phase2_error_marks_all_results_failed() {
        let mut results = vec![ok_result("a.md"), ok_result("b.md")];

        apply_phase2_error(&mut results, &None);
        assert!(results.iter().all(|r| r.success && r.error.is_none()));

        apply_phase2_error(
            &mut results,
            &Some("Collection assignment failed: boom".to_string()),
        );
        assert!(results.iter().all(|r| !r.success));
        assert!(results
            .iter()
            .all(|r| r.error.as_deref() == Some("Collection assignment failed: boom")));
    }

    /// A request naming an unregistered database is rejected at the routing
    /// boundary (ADR-053), never silently served from the default.
    #[tokio::test]
    async fn import_rejects_unregistered_database_header() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            DatabaseManager::load(dir.path().join("databases.toml"), test_context())
                .await
                .unwrap(),
        );
        let default_id = manager
            .ensure_default_registered("Default".into(), dir.path().join("db.db"))
            .await
            .unwrap();
        let svc = manager
            .get_or_open(&default_id)
            .await
            .unwrap()
            .import
            .clone();

        let mut req = Request::new(ImportMarkdownRequest {
            file_path: "/nonexistent.md".into(),
            options: None,
        });
        req.extensions_mut().insert(manager.clone());
        req.metadata_mut()
            .insert(DATABASE_ID_HEADER, "ZZZ-UNREGISTERED".parse().unwrap());
        assert_eq!(
            svc.import_markdown(req).await.unwrap_err().code(),
            tonic::Code::NotFound
        );
    }

    /// A document's root id is a pure function of its base-directory-relative
    /// path: the same path always yields the same (valid) id, different paths
    /// yield different ids. This determinism is what makes re-import idempotent.
    #[test]
    fn batch_size_cap_accepts_the_limit_and_rejects_beyond_it() {
        assert!(oversized_batch_error(0).is_none());
        assert!(oversized_batch_error(MAX_IMPORT_FILES).is_none());
        let err = oversized_batch_error(MAX_IMPORT_FILES + 1).expect("over the limit");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message()
                .contains(&format!("maximum of {MAX_IMPORT_FILES} files")),
            "error names the limit: {}",
            err.message()
        );
        assert!(
            err.message()
                .contains(&format!("got {}", MAX_IMPORT_FILES + 1)),
            "error names the requested size: {}",
            err.message()
        );
    }

    /// The cap is enforced at the RPC boundary, before any file is touched.
    #[tokio::test]
    async fn import_markdown_files_rejects_oversized_batch() {
        let (ns, _dir) = new_service_and_dir().await;
        let svc = ImportServiceImpl::new(Arc::new(ns));
        let request = Request::new(ImportMarkdownFilesRequest {
            file_paths: vec!["/nonexistent.md".to_string(); MAX_IMPORT_FILES + 1],
            options: None,
        });
        match svc.import_markdown_files(request).await {
            Ok(_) => panic!("an oversized batch must be rejected"),
            Err(status) => assert_eq!(status.code(), tonic::Code::InvalidArgument),
        }
    }

    /// A batch spanning many phase-1 chunks imports every file, and links whose
    /// source and target land in different chunks still resolve to mentions.
    #[tokio::test]
    async fn large_batch_import_resolves_links_across_chunks() {
        const FILES: usize = 300;
        const LINK_STRIDE: usize = 97;
        const { assert!(FILES > PHASE1_CHUNK_SIZE * 4 && LINK_STRIDE > PHASE1_CHUNK_SIZE) };

        let (ns, dir) = new_service_and_dir().await;
        let ns = Arc::new(ns);
        let src = dir.path().join("vault");
        std::fs::create_dir_all(&src).unwrap();
        let name = |i: usize| format!("note-{i:03}.md");
        let mut files = Vec::with_capacity(FILES + 1);
        for i in 0..FILES {
            let target = name((i + LINK_STRIDE) % FILES);
            std::fs::write(
                src.join(name(i)),
                format!("# Note {i}\n\nSee [the next note](./{target}) for more.\n"),
            )
            .unwrap();
            files.push(src.join(name(i)).to_str().unwrap().to_string());
        }
        // A missing file fails on its own without failing the rest of the batch.
        let missing = src.join("missing.md").to_str().unwrap().to_string();
        files.push(missing.clone());

        let opts = ImportOptions {
            base_directory: src.to_str().unwrap().to_string(),
            ..Default::default()
        };
        let done = run_batch_and_wait(Arc::clone(&ns), files, opts).await;

        assert_eq!(done.results.len(), FILES + 1);
        let failed: Vec<_> = done.results.iter().filter(|r| !r.success).collect();
        assert_eq!(failed.len(), 1, "only the missing file fails: {failed:?}");
        assert_eq!(failed[0].file_path, missing);
        assert_eq!(failed[0].error, "File does not exist");

        for i in 0..FILES {
            let root = deterministic_root_id(&name(i));
            assert!(
                ns.get_node(&root).await.unwrap().is_some(),
                "note {i} imported"
            );
            let inbound = ns.get_mentioned_by(&root).await.unwrap();
            assert_eq!(inbound.len(), 1, "note {i} is linked from exactly one note");
        }

        // The link text itself is rewritten to the target's root id.
        let first_children = ns
            .get_descendants(&deterministic_root_id(&name(0)))
            .await
            .unwrap();
        let target_root = deterministic_root_id(&name(LINK_STRIDE));
        assert!(
            first_children
                .iter()
                .any(|n| n.content.contains(&format!("nodespace://{target_root}"))),
            "note 0 links to note {LINK_STRIDE} across chunk boundaries",
        );
    }

    #[test]
    fn deterministic_root_id_is_stable_and_path_sensitive() {
        assert_eq!(
            deterministic_root_id("architecture/system-overview.md"),
            deterministic_root_id("architecture/system-overview.md"),
        );
        assert_ne!(
            deterministic_root_id("architecture/system-overview.md"),
            deterministic_root_id("architecture/data-layer.md"),
        );
        assert!(uuid::Uuid::parse_str(&deterministic_root_id("any/path.md")).is_ok());
    }

    /// Single-file path: a plain re-import of an unchanged file is a no-op (the
    /// deterministic root id resolves to the same document, nothing is created,
    /// nothing duplicates); `--replace` keeps that root id while refreshing its
    /// child subtree from the fresh parse.
    #[tokio::test]
    async fn reimport_single_file_is_idempotent_and_replace_refreshes() {
        let (ns, dir) = new_service_and_dir().await;
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let path = src.join("guide.md");
        std::fs::write(&path, "# Guide\n\nAlpha.\n\n## Section\n\nBeta.\n").unwrap();

        let opts = ImportOptions {
            base_directory: src.to_str().unwrap().to_string(),
            ..Default::default()
        };

        // First import creates the document.
        let r1 = import_single_file(&ns, &path, &opts).await;
        assert!(r1.success, "first import failed: {:?}", r1.error);
        assert!(r1.nodes_created > 0);
        let root_id = r1.root_id.clone().expect("root id");
        let headers_after_first = ns.store().count_nodes_by_type("header").await.unwrap();
        let subtree_after_first = ns.get_descendants(&root_id).await.unwrap().len();
        assert!(subtree_after_first > 0);

        // Plain re-import: same root id, nothing created, no duplication.
        let r2 = import_single_file(&ns, &path, &opts).await;
        assert!(r2.success);
        assert_eq!(r2.nodes_created, 0, "a plain re-import must create nothing");
        assert_eq!(r2.root_id.as_deref(), Some(root_id.as_str()));
        assert_eq!(
            ns.store().count_nodes_by_type("header").await.unwrap(),
            headers_after_first,
            "a plain re-import must not duplicate any node",
        );

        // Edit the file (drop the section), re-import with --replace: the root
        // id is kept (inbound links survive) and the subtree is refreshed.
        std::fs::write(&path, "# Guide\n\nAlpha revised.\n").unwrap();
        let opts_replace = ImportOptions {
            replace: true,
            ..opts.clone()
        };
        let r3 = import_single_file(&ns, &path, &opts_replace).await;
        assert!(r3.success, "replace import failed: {:?}", r3.error);
        assert_eq!(
            r3.root_id.as_deref(),
            Some(root_id.as_str()),
            "replace keeps the root id",
        );
        assert!(
            ns.get_node(&root_id).await.unwrap().is_some(),
            "root node survives replace",
        );
        let subtree_after_replace = ns.get_descendants(&root_id).await.unwrap().len();
        assert!(
            subtree_after_replace < subtree_after_first,
            "replace pruned the removed section (was {subtree_after_first}, now {subtree_after_replace})",
        );
        assert!(
            ns.store().count_nodes_by_type("header").await.unwrap() <= headers_after_first,
            "replace refreshes in place — it must never grow the document set",
        );
    }

    /// Batch path (the `import dir` pipeline used for the docs corpus): a plain
    /// re-import never duplicates, and `--replace` keeps each document's root id
    /// (so an inbound `[[link]]` survives) and its collection membership.
    #[tokio::test]
    async fn batch_reimport_does_not_duplicate_and_replace_keeps_roots() {
        let (ns, dir) = new_service_and_dir().await;
        let ns = Arc::new(ns);
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.md"), "# Doc A\n\nSee [[Doc B]] for more.\n").unwrap();
        std::fs::write(src.join("b.md"), "# Doc B\n\nBody of B.\n").unwrap();
        let files = vec![
            src.join("a.md").to_str().unwrap().to_string(),
            src.join("b.md").to_str().unwrap().to_string(),
        ];
        let opts = ImportOptions {
            base_directory: src.to_str().unwrap().to_string(),
            auto_collection_routing: true,
            ..Default::default()
        };

        // First import: both roots exist and are assigned to a collection.
        run_batch_and_wait(Arc::clone(&ns), files.clone(), opts.clone()).await;
        let id_a = deterministic_root_id("a.md");
        let id_b = deterministic_root_id("b.md");
        assert!(ns.get_node(&id_a).await.unwrap().is_some(), "Doc A created");
        assert!(ns.get_node(&id_b).await.unwrap().is_some(), "Doc B created");
        let headers_after_first = ns.store().count_nodes_by_type("header").await.unwrap();
        assert!(
            !ns.store()
                .get_node_memberships(&id_a)
                .await
                .unwrap()
                .is_empty(),
            "Doc A is assigned to a collection",
        );

        // Plain batch re-import: no duplication.
        run_batch_and_wait(Arc::clone(&ns), files.clone(), opts.clone()).await;
        assert_eq!(
            ns.store().count_nodes_by_type("header").await.unwrap(),
            headers_after_first,
            "a plain batch re-import must not duplicate",
        );
        assert!(ns.get_node(&id_a).await.unwrap().is_some());
        assert!(ns.get_node(&id_b).await.unwrap().is_some());

        // Batch re-import with --replace: roots keep their ids, still no
        // duplication, membership intact.
        let opts_replace = ImportOptions {
            replace: true,
            ..opts.clone()
        };
        run_batch_and_wait(Arc::clone(&ns), files.clone(), opts_replace).await;
        assert_eq!(
            ns.store().count_nodes_by_type("header").await.unwrap(),
            headers_after_first,
            "replacing unchanged docs must keep the node count stable",
        );
        assert!(
            ns.get_node(&id_a).await.unwrap().is_some(),
            "Doc A root kept across replace",
        );
        assert!(
            ns.get_node(&id_b).await.unwrap().is_some(),
            "Doc B root kept across replace",
        );
        assert!(
            !ns.store()
                .get_node_memberships(&id_a)
                .await
                .unwrap()
                .is_empty(),
            "collection membership survives replace",
        );
    }

    /// The demo-repair case: a document already in the store but MISSING its
    /// topic-collection membership (its content landed only in the default
    /// collection) regains that membership from a plain re-import — no
    /// `--replace`, no duplication. This is the self-heal path the demo re-seed
    /// relies on: routing is re-derived and every root's membership is
    /// re-asserted, so a root that lost (or never had) its topic edge gets wired
    /// up in place.
    #[tokio::test]
    async fn reimport_reasserts_missing_collection_membership() {
        let (ns, dir) = new_service_and_dir().await;
        let ns = Arc::new(ns);
        // base_directory is the repo root; the file under architecture/ routes to
        // the "Architecture" collection.
        let arch = dir.path().join("architecture");
        std::fs::create_dir_all(&arch).unwrap();
        std::fs::write(arch.join("overview.md"), "# Overview\n\nBody.\n").unwrap();
        let files = vec![arch.join("overview.md").to_str().unwrap().to_string()];
        let opts = ImportOptions {
            base_directory: dir.path().to_str().unwrap().to_string(),
            auto_collection_routing: true,
            ..Default::default()
        };

        // First import: the doc root joins its topic collection.
        run_batch_and_wait(Arc::clone(&ns), files.clone(), opts.clone()).await;
        let root_id = deterministic_root_id("architecture/overview.md");
        let memberships_before = ns.store().get_node_memberships(&root_id).await.unwrap();
        assert!(
            !memberships_before.is_empty(),
            "doc routed into a collection on first import",
        );
        let topic_coll = memberships_before[0].clone();

        // Simulate the broken demo state: strip the topic membership so the root
        // is present but orphaned from its collection (unbrowsable there).
        CollectionService::new(ns.store(), &ns)
            .remove_from_collection(&root_id, &topic_coll)
            .await
            .unwrap();
        assert!(
            ns.store()
                .get_node_memberships(&root_id)
                .await
                .unwrap()
                .is_empty(),
            "membership removed — the root is now orphaned from its collection",
        );
        let headers_before = ns.store().count_nodes_by_type("header").await.unwrap();

        // Plain re-import (no --replace) must re-assert the lost membership and
        // create no new node.
        run_batch_and_wait(Arc::clone(&ns), files.clone(), opts.clone()).await;
        assert!(
            ns.store()
                .get_node_memberships(&root_id)
                .await
                .unwrap()
                .contains(&topic_coll),
            "plain re-import re-asserts the lost topic-collection membership (self-heal)",
        );
        assert_eq!(
            ns.store().count_nodes_by_type("header").await.unwrap(),
            headers_before,
            "self-heal adds only the missing edge — it must not duplicate any node",
        );
    }
}
