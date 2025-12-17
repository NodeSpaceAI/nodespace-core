//! File import commands for bulk markdown import
//!
//! Provides direct file system import bypassing MCP for faster bulk operations.
//! Useful for importing large documentation sets (e.g., entire book chapters).

use nodespace_core::mcp::handlers::markdown::prepare_nodes_from_markdown;
use nodespace_core::services::{CreateNodeParams, NodeService};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::State;

/// Result of importing a single file
#[derive(Debug, Serialize)]
pub struct FileImportResult {
    pub file_path: String,
    pub root_id: Option<String>,
    pub nodes_created: usize,
    pub success: bool,
    pub error: Option<String>,
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
#[derive(Debug, Deserialize)]
pub struct ImportOptions {
    /// Collection path to add imported documents to (e.g., "rust-book")
    pub collection: Option<String>,
    /// Whether to use filename as title (default: true, otherwise first line of content)
    #[serde(default = "default_use_filename_as_title")]
    pub use_filename_as_title: bool,
}

fn default_use_filename_as_title() -> bool {
    true
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
    let options = options.unwrap_or(ImportOptions {
        collection: None,
        use_filename_as_title: true,
    });

    // Validate file exists and is readable
    if !path.exists() {
        return Ok(FileImportResult {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some("File does not exist".to_string()),
        });
    }

    if !path.is_file() {
        return Ok(FileImportResult {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some("Path is not a file".to_string()),
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
            });
        }
    };

    // Determine title
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
    match import_markdown_content(
        &node_service,
        &title,
        &content,
        options.collection.as_deref(),
    )
    .await
    {
        Ok((root_id, nodes_created)) => Ok(FileImportResult {
            file_path,
            root_id: Some(root_id),
            nodes_created,
            success: true,
            error: None,
        }),
        Err(e) => Ok(FileImportResult {
            file_path,
            root_id: None,
            nodes_created: 0,
            success: false,
            error: Some(e),
        }),
    }
}

/// Import multiple markdown files in batch
///
/// Processes files sequentially but uses bulk_create_hierarchy for each file
/// for maximum efficiency. Returns detailed results for each file.
#[tauri::command]
pub async fn import_markdown_files(
    node_service: State<'_, NodeService>,
    file_paths: Vec<String>,
    options: Option<ImportOptions>,
) -> Result<BatchImportResult, String> {
    let start = std::time::Instant::now();
    let total_files = file_paths.len();
    let options = options.unwrap_or(ImportOptions {
        collection: None,
        use_filename_as_title: true,
    });

    let mut results = Vec::with_capacity(total_files);
    let mut successful = 0;
    let mut failed = 0;

    for file_path in file_paths {
        let path = PathBuf::from(&file_path);

        // Skip non-existent files
        if !path.exists() || !path.is_file() {
            results.push(FileImportResult {
                file_path,
                root_id: None,
                nodes_created: 0,
                success: false,
                error: Some("File does not exist or is not a file".to_string()),
            });
            failed += 1;
            continue;
        }

        // Read file content
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                results.push(FileImportResult {
                    file_path,
                    root_id: None,
                    nodes_created: 0,
                    success: false,
                    error: Some(format!("Failed to read file: {}", e)),
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

        // Import
        match import_markdown_content(
            &node_service,
            &title,
            &content,
            options.collection.as_deref(),
        )
        .await
        {
            Ok((root_id, nodes_created)) => {
                results.push(FileImportResult {
                    file_path,
                    root_id: Some(root_id),
                    nodes_created,
                    success: true,
                    error: None,
                });
                successful += 1;
            }
            Err(e) => {
                results.push(FileImportResult {
                    file_path,
                    root_id: None,
                    nodes_created: 0,
                    success: false,
                    error: Some(e),
                });
                failed += 1;
            }
        }
    }

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
#[tauri::command]
pub async fn import_markdown_directory(
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

    // Collect all .md files
    let mut md_files: Vec<String> = Vec::new();
    collect_markdown_files(&path, &mut md_files)?;

    // Sort for consistent ordering
    md_files.sort();

    tracing::info!(
        "Found {} markdown files in {}",
        md_files.len(),
        directory_path
    );

    // Import all files
    import_markdown_files(node_service, md_files, options).await
}

/// Recursively collect markdown files from a directory
fn collect_markdown_files(dir: &PathBuf, files: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        if path.is_dir() {
            // Recurse into subdirectories
            collect_markdown_files(&path, files)?;
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
async fn import_markdown_content(
    node_service: &NodeService,
    title: &str,
    content: &str,
    collection: Option<&str>,
) -> Result<(String, usize), String> {
    // Prepare nodes from markdown (phase 1 - in memory, no DB)
    let prepared_nodes = prepare_nodes_from_markdown(content, None)
        .map_err(|e| format!("Failed to parse markdown: {:?}", e))?;

    // Create root node
    let root_id = node_service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "header".to_string(),
            content: title.to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: serde_json::json!({}),
        })
        .await
        .map_err(|e| format!("Failed to create root node: {}", e))?;

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

        let created_ids = node_service
            .bulk_create_hierarchy(nodes_for_bulk)
            .await
            .map_err(|e| format!("Failed to bulk create nodes: {}", e))?;

        nodes_created += created_ids.len();
    }

    // Add to collection if specified
    if let Some(collection_path) = collection {
        use nodespace_core::services::CollectionService;
        let collection_service = CollectionService::new(node_service.store());
        collection_service
            .add_to_collection_by_path(&root_id, collection_path)
            .await
            .map_err(|e| format!("Failed to add to collection: {}", e))?;
    }

    Ok((root_id, nodes_created))
}
