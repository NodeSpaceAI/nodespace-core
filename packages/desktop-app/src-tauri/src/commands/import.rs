//! File import commands for bulk markdown import
//!
//! Provides direct file system import bypassing MCP for faster bulk operations.
//! Useful for importing large documentation sets (e.g., entire book chapters).
//!
//! ## Smart Collection Routing
//!
//! When `auto_collection_routing` is enabled, files are automatically assigned
//! to collections based on their directory path:
//!
//! - `*/archived/*` → "Archived" collection (with lifecycle_status: "archived")
//! - `*/decisions/*` or `*/adr/*` → "ADR" collection
//! - `*/lessons/*` → "Lessons" collection
//! - `*/troubleshooting/*` → "Troubleshooting" collection
//! - `*/components/*` → "Components" collection
//! - `*/business-logic/*` → "Business Logic" collection
//! - `*/development/process/*` → "Development:Process" collection
//! - `*/architecture/core/*` → "Architecture:Core" collection
//! - etc.

use nodespace_core::mcp::handlers::markdown::prepare_nodes_from_markdown;
use nodespace_core::services::{CreateNodeParams, NodeService};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State};

/// Result of importing a single file
#[derive(Debug, Clone, Serialize)]
pub struct FileImportResult {
    pub file_path: String,
    pub root_id: Option<String>,
    pub nodes_created: usize,
    pub success: bool,
    pub error: Option<String>,
    /// Collection the file was assigned to (if auto-routing enabled)
    pub collection: Option<String>,
    /// Whether the file was marked as archived
    pub archived: bool,
}

/// Result of batch file import
#[derive(Debug, Serialize)]
pub struct BatchImportResult {
    pub total_files: usize,
    pub successful: usize,
    pub failed: usize,
    pub results: Vec<FileImportResult>,
    pub duration_ms: u128,
}

/// Options for file import
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImportOptions {
    /// Collection path to add imported documents to (e.g., "rust-book")
    /// Ignored if `auto_collection_routing` is true.
    pub collection: Option<String>,
    /// Whether to use filename as title (default: false - uses first heading/line)
    #[serde(default)]
    pub use_filename_as_title: bool,
    /// Enable smart collection routing based on file paths (default: false)
    /// When enabled, files are assigned to collections based on directory structure.
    #[serde(default)]
    pub auto_collection_routing: bool,
    /// Directory patterns to exclude (e.g., ["design-system", "node_modules"])
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    /// Base directory for relative path calculation in auto-routing
    /// If not set, uses the import directory as base.
    pub base_directory: Option<String>,
}

/// Progress event emitted during import
#[derive(Debug, Clone, Serialize)]
pub struct ImportProgressEvent {
    /// Current file being processed (1-indexed)
    pub current: usize,
    /// Total number of files to process
    pub total: usize,
    /// Current file path (relative)
    pub file_path: String,
    /// Status of the current file
    pub status: String,
    /// Collection assigned (if any)
    pub collection: Option<String>,
}

/// Metadata derived from file path for collection routing
#[derive(Debug, Clone)]
struct CollectionMetadata {
    /// Collection path (e.g., "Architecture:Core")
    collection: String,
    /// Whether the file should be marked as archived
    is_archived: bool,
}

/// Derive collection and metadata from file path relative to base directory
///
/// Implements smart routing rules:
/// - `*/archived/*` → "Archived" (is_archived: true)
/// - `*/decisions/*` → "ADR"
/// - `*/lessons/*` → "Lessons"
/// - `*/troubleshooting/*` → "Troubleshooting"
/// - `*/components/*` → "Components"
/// - `*/business-logic/*` → "Business Logic"
/// - `*/development/X/*` → "Development:X"
/// - `*/architecture/core/*` → "Architecture:Core"
/// - `*/architecture/X/*` → "Architecture:X"
fn derive_collection_metadata(file_path: &Path, base_dir: &Path) -> CollectionMetadata {
    let relative = file_path.strip_prefix(base_dir).unwrap_or(file_path);

    let path_str = relative.to_string_lossy().to_lowercase();
    let segments: Vec<&str> = relative
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // Check for archived content anywhere in path
    if path_str.contains("/archived/")
        || segments.iter().any(|s| s.eq_ignore_ascii_case("archived"))
    {
        return CollectionMetadata {
            collection: "Archived".to_string(),
            is_archived: true,
        };
    }

    // ADR / Decisions
    if path_str.contains("/decisions/") || path_str.contains("/adr/") {
        return CollectionMetadata {
            collection: "ADR".to_string(),
            is_archived: false,
        };
    }

    // Lessons
    if path_str.contains("/lessons/") || segments.iter().any(|s| s.eq_ignore_ascii_case("lessons"))
    {
        return CollectionMetadata {
            collection: "Lessons".to_string(),
            is_archived: false,
        };
    }

    // Troubleshooting
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

    // Architecture-specific routing
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

    // Performance
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

    // Testing
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

    // Default: use directory path as collection
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

/// Convert kebab-case or snake_case to Title Case
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

/// Import a single markdown file directly into NodeSpace
///
/// Bypasses MCP for faster direct import. Reads file from disk,
/// parses markdown, and creates nodes using bulk_create_hierarchy.
#[tauri::command]
pub async fn import_markdown_file(
    node_service: State<'_, NodeService>,
    file_path: String,
    options: Option<ImportOptions>,
) -> Result<FileImportResult, String> {
    let path = PathBuf::from(&file_path);
    let options = options.unwrap_or_default();

    // Validate file exists and is readable
    if !path.exists() {
        return Ok(FileImportResult {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some("File does not exist".to_string()),
            collection: None,
            archived: false,
        });
    }

    if !path.is_file() {
        return Ok(FileImportResult {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some("Path is not a file".to_string()),
            collection: None,
            archived: false,
        });
    }

    // Read file content
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(FileImportResult {
                file_path,
                root_id: None,
                nodes_created: 0,
                success: false,
                error: Some(format!("Failed to read file: {}", e)),
                collection: None,
                archived: false,
            });
        }
    };

    // Determine collection and metadata
    let (collection, is_archived) = if options.auto_collection_routing {
        let base_dir = options
            .base_directory
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf());
        let metadata = derive_collection_metadata(&path, &base_dir);
        (Some(metadata.collection), metadata.is_archived)
    } else {
        (options.collection.clone(), false)
    };

    // Determine title - prefer first heading from content
    let title = if options.use_filename_as_title {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    } else {
        // Use first non-empty line as title
        content
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    };

    // Import using internal function
    match import_markdown_content(&node_service, &title, &content, is_archived).await {
        Ok((root_id, nodes_created)) => {
            // For single file import, do collection assignment immediately
            if let Some(ref coll) = collection {
                use nodespace_core::services::CollectionService;
                let collection_service =
                    CollectionService::new(node_service.store(), &node_service);
                if let Err(e) = collection_service
                    .add_to_collection_by_path(&root_id, coll)
                    .await
                {
                    return Ok(FileImportResult {
                        file_path,
                        root_id: Some(root_id),
                        nodes_created,
                        success: true,
                        error: Some(format!("Imported but failed to add to collection: {}", e)),
                        collection,
                        archived: is_archived,
                    });
                }
            }
            Ok(FileImportResult {
                file_path,
                root_id: Some(root_id),
                nodes_created,
                success: true,
                error: None,
                collection,
                archived: is_archived,
            })
        }
        Err(e) => Ok(FileImportResult {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some(e),
            collection: None,
            archived: false,
        }),
    }
}

/// Import multiple markdown files in batch
///
/// Processes files sequentially but uses bulk_create_hierarchy for each file
/// for maximum efficiency. Returns detailed results for each file.
/// Emits "import-progress" events to the frontend for progress tracking.
#[tauri::command]
pub async fn import_markdown_files(
    app: AppHandle,
    node_service: State<'_, NodeService>,
    file_paths: Vec<String>,
    options: Option<ImportOptions>,
) -> Result<BatchImportResult, String> {
    let start = std::time::Instant::now();
    let total_files = file_paths.len();
    let options = options.unwrap_or_default();

    let mut results = Vec::with_capacity(total_files);
    let mut successful = 0;
    let mut failed = 0;

    // Determine base directory for auto-routing
    let base_dir = options
        .base_directory
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| {
            file_paths.first().map(|p| {
                PathBuf::from(p)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf()
            })
        })
        .unwrap_or_else(|| PathBuf::from("."));

    for (index, file_path) in file_paths.iter().enumerate() {
        let path = PathBuf::from(file_path);
        let relative_path = path
            .strip_prefix(&base_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        // Skip non-existent files
        if !path.exists() || !path.is_file() {
            // Emit progress event
            let _ = app.emit(
                "import-progress",
                ImportProgressEvent {
                    current: index + 1,
                    total: total_files,
                    file_path: relative_path.clone(),
                    status: "skipped".to_string(),
                    collection: None,
                },
            );

            results.push(FileImportResult {
                file_path: file_path.clone(),
                root_id: None,
                nodes_created: 0,
                success: false,
                error: Some("File does not exist or is not a file".to_string()),
                collection: None,
                archived: false,
            });
            failed += 1;
            continue;
        }

        // Determine collection and metadata
        let (collection, is_archived) = if options.auto_collection_routing {
            let metadata = derive_collection_metadata(&path, &base_dir);
            (Some(metadata.collection), metadata.is_archived)
        } else {
            (options.collection.clone(), false)
        };

        // Emit progress event
        let _ = app.emit(
            "import-progress",
            ImportProgressEvent {
                current: index + 1,
                total: total_files,
                file_path: relative_path.clone(),
                status: "importing".to_string(),
                collection: collection.clone(),
            },
        );

        // Read file content
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                results.push(FileImportResult {
                    file_path: file_path.clone(),
                    root_id: None,
                    nodes_created: 0,
                    success: false,
                    error: Some(format!("Failed to read file: {}", e)),
                    collection: None,
                    archived: false,
                });
                failed += 1;
                continue;
            }
        };

        // Determine title
        let title = if options.use_filename_as_title {
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

        // Import with inline collection assignment (real-time UI updates)
        match import_markdown_content(&node_service, &title, &content, is_archived).await {
            Ok((root_id, nodes_created)) => {
                // Assign to collection inline for real-time UI visibility
                if let Some(ref coll) = collection {
                    let _ = assign_to_collection(&node_service, &root_id, coll).await;
                }
                results.push(FileImportResult {
                    file_path: file_path.clone(),
                    root_id: Some(root_id),
                    nodes_created,
                    success: true,
                    error: None,
                    collection,
                    archived: is_archived,
                });
                successful += 1;
            }
            Err(e) => {
                results.push(FileImportResult {
                    file_path: file_path.clone(),
                    root_id: None,
                    nodes_created: 0,
                    success: false,
                    error: Some(e),
                    collection: None,
                    archived: false,
                });
                failed += 1;
            }
        }
    }

    // Emit completion event
    let _ = app.emit(
        "import-progress",
        ImportProgressEvent {
            current: total_files,
            total: total_files,
            file_path: "".to_string(),
            status: "complete".to_string(),
            collection: None,
        },
    );

    Ok(BatchImportResult {
        total_files,
        successful,
        failed,
        results,
        duration_ms: start.elapsed().as_millis(),
    })
}

/// Import markdown from a directory (all .md files)
///
/// Recursively finds all markdown files and imports them.
/// Supports exclusion patterns and auto-collection routing.
#[tauri::command]
pub async fn import_markdown_directory(
    app: AppHandle,
    node_service: State<'_, NodeService>,
    directory_path: String,
    options: Option<ImportOptions>,
) -> Result<BatchImportResult, String> {
    let path = PathBuf::from(&directory_path);

    if !path.exists() {
        return Err("Directory does not exist".to_string());
    }

    if !path.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    let options = options.unwrap_or_default();
    let exclude_patterns = &options.exclude_patterns;

    // Collect all .md files, respecting exclusion patterns
    let mut md_files: Vec<String> = Vec::new();
    collect_markdown_files_with_exclusions(&path, &mut md_files, exclude_patterns)?;

    // Sort for consistent ordering
    md_files.sort();

    tracing::info!(
        "Found {} markdown files in {} (excluded patterns: {:?})",
        md_files.len(),
        directory_path,
        exclude_patterns
    );

    // Set base directory if not already set
    let mut import_options = options.clone();
    if import_options.base_directory.is_none() {
        import_options.base_directory = Some(directory_path.clone());
    }

    // Import all files
    import_markdown_files(app, node_service, md_files, Some(import_options)).await
}

/// Recursively collect markdown files from a directory with exclusion patterns
fn collect_markdown_files_with_exclusions(
    dir: &PathBuf,
    files: &mut Vec<String>,
    exclude_patterns: &[String],
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        // Check if this path matches any exclusion pattern
        let path_str = path.to_string_lossy();
        let should_exclude = exclude_patterns.iter().any(|pattern| {
            // Simple pattern matching: check if any path component equals the pattern
            path.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|s| s.eq_ignore_ascii_case(pattern))
                    .unwrap_or(false)
            }) || path_str.contains(pattern)
        });

        if should_exclude {
            tracing::debug!("Excluding path: {}", path_str);
            continue;
        }

        if path.is_dir() {
            // Recurse into subdirectories
            collect_markdown_files_with_exclusions(&path, files, exclude_patterns)?;
        } else if path.is_file() {
            // Check for .md extension
            if let Some(ext) = path.extension() {
                if ext == "md" {
                    if let Some(path_str) = path.to_str() {
                        files.push(path_str.to_string());
                    }
                }
            }
        }
    }

    Ok(())
}

/// Internal function to import markdown content
///
/// Creates root node and bulk-creates children efficiently.
/// Collection assignment is deferred - returns root_id for later batch assignment.
/// Supports lifecycle_status for archived docs.
///
/// **De-duplication Logic**: If the title was extracted from the first line of content
/// (indicated by `title` being the raw first line), the children are parsed from content
/// AFTER the first line to avoid duplicate nodes.
async fn import_markdown_content(
    node_service: &NodeService,
    title: &str,
    content: &str,
    is_archived: bool,
) -> Result<(String, usize), String> {
    // Determine root node type - if title starts with # it's a header, otherwise text
    let (node_type, clean_title) = if title.starts_with('#') {
        ("header", title.to_string())
    } else {
        ("header", format!("# {}", title))
    };

    // Skip the first line of content if it matches the title (de-duplication)
    // This prevents creating a duplicate H1 child when the title came from content
    let content_for_children = {
        let first_line = content.lines().find(|l| !l.trim().is_empty());
        if first_line == Some(title) {
            // Title matches first line - skip it in children
            let lines: Vec<&str> = content.lines().collect();
            let first_idx = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
            lines[first_idx + 1..].join("\n")
        } else {
            // Title was provided separately or doesn't match - use full content
            content.to_string()
        }
    };

    // Prepare nodes from markdown (phase 1 - in memory, no DB)
    let prepared_nodes = prepare_nodes_from_markdown(&content_for_children, None)
        .map_err(|e| format!("Failed to parse markdown: {:?}", e))?;

    // Create root node with lifecycle_status if archived
    let mut properties = serde_json::json!({});
    if is_archived {
        properties["lifecycle_status"] = serde_json::json!("archived");
    }

    let root_id = node_service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: node_type.to_string(),
            content: clean_title,
            parent_id: None,
            insert_after_node_id: None,
            properties,
        })
        .await
        .map_err(|e| format!("Failed to create root node: {}", e))?;

    // If archived, update lifecycle_status explicitly (in case properties didn't work)
    if is_archived {
        if let Err(e) = node_service
            .store()
            .update_lifecycle_status(&root_id, "archived")
            .await
        {
            tracing::warn!(
                "Failed to set lifecycle_status to archived for {}: {}",
                root_id,
                e
            );
        }
    }

    let mut nodes_created = 1; // Root node

    // Bulk create children if any
    if !prepared_nodes.is_empty() {
        // Remap parent_ids: None -> root_id, Some(id) -> id
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
                let parent = n.parent_id.clone().or_else(|| Some(root_id.clone()));
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

        // Use root-only notification - emits single event for root node
        // Other clients receive "tree created" event without per-node overhead
        let created_ids = node_service
            .bulk_create_hierarchy_root_notify(nodes_for_bulk)
            .await
            .map_err(|e| format!("Failed to bulk create nodes: {}", e))?;

        nodes_created += created_ids.len();
    }

    // NOTE: Collection assignment is now deferred to batch processing after all imports
    // This avoids N+1 collection lookups that cause slowdown

    Ok((root_id, nodes_created))
}

/// Assign a node to a collection by path (creates collection if needed)
async fn assign_to_collection(
    node_service: &NodeService,
    node_id: &str,
    collection_path: &str,
) -> Result<(), String> {
    use nodespace_core::services::CollectionService;

    let collection_service = CollectionService::new(node_service.store(), node_service);

    // This creates the collection path if needed AND adds the node to it
    collection_service
        .add_to_collection_by_path(node_id, collection_path)
        .await
        .map_err(|e| {
            format!(
                "Failed to add {} to collection '{}': {:?}",
                node_id, collection_path, e
            )
        })?;

    Ok(())
}
