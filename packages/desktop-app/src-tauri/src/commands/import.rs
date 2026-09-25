//! File import commands — thin gRPC proxy to ImportService in nodespaced.
//!
//! All import logic (smart collection routing, two-phase async pipeline) runs
//! inside the daemon so imports continue if the Tauri window is closed
//! mid-import. These handlers subscribe to the progress stream and forward
//! each event to the frontend via Tauri events.

use nodespace_proto::nodespace::{
    ImportMarkdownFilesRequest, ImportMarkdownRequest, ImportOptions as ProtoImportOptions,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime, State};
use tokio_stream::StreamExt;
use tonic::Request;

use crate::services::GrpcClient;
use crate::window_routing::emit_routed;

/// Options for file import (mirrors proto ImportOptions).
///
/// The three folder-walk fields (`include_agent_files`, `include_hidden`,
/// `no_recursion`) name the *opt-out* of a default-on filter, matching the proto
/// polarity: their `bool` default (`false`, via `#[serde(default)]` and
/// `Default`) is the default-on behavior, so an absent or partial options object
/// from the frontend still excludes agent files, skips hidden entries, and
/// recurses into sub-folders.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImportOptions {
    pub collection: Option<String>,
    #[serde(default)]
    pub use_filename_as_title: bool,
    #[serde(default)]
    pub auto_collection_routing: bool,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    pub base_directory: Option<String>,
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub include_agent_files: bool,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default)]
    pub no_recursion: bool,
}

impl ImportOptions {
    fn into_proto(self) -> ProtoImportOptions {
        ProtoImportOptions {
            collection: self.collection.unwrap_or_default(),
            use_filename_as_title: self.use_filename_as_title,
            auto_collection_routing: self.auto_collection_routing,
            exclude_patterns: self.exclude_patterns,
            base_directory: self.base_directory.unwrap_or_default(),
            replace: self.replace,
            include_agent_files: self.include_agent_files,
            include_hidden: self.include_hidden,
            no_recursion: self.no_recursion,
        }
    }
}

/// Folder-walk filters, in active/default-on sense (true = filter applied).
/// Derived from `ImportOptions` opt-out fields at the collection call site.
#[derive(Clone, Copy)]
struct WalkFilters {
    /// Skip files whose basename is CLAUDE.md / AGENTS.md / DESIGN.md (case-insensitive).
    exclude_agent_files: bool,
    /// Skip any entry (file or dir) whose name starts with '.'.
    skip_hidden: bool,
    /// Descend into sub-folders. When false, only the top-level dir is scanned.
    recursive: bool,
}

impl WalkFilters {
    fn from_options(opts: &ImportOptions) -> Self {
        Self {
            exclude_agent_files: !opts.include_agent_files,
            skip_hidden: !opts.include_hidden,
            recursive: !opts.no_recursion,
        }
    }
}

/// Basenames dropped by the "exclude agent files" filter — matched
/// case-insensitively against the file's basename at any depth.
fn is_agent_file(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.eq_ignore_ascii_case("CLAUDE.md")
                || n.eq_ignore_ascii_case("AGENTS.md")
                || n.eq_ignore_ascii_case("DESIGN.md")
        })
        .unwrap_or(false)
}

/// A hidden entry is one whose own name starts with '.'. The walk visits every
/// path component as an entry, so testing the immediate name at each level is
/// equivalent to "any path component starting with '.'".
fn is_hidden(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

/// Result of importing a single file
#[derive(Debug, Clone, Serialize)]
pub struct FileImportResult {
    pub file_path: String,
    pub root_id: Option<String>,
    pub nodes_created: usize,
    pub success: bool,
    pub error: Option<String>,
    pub collection: Option<String>,
    pub archived: bool,
}

/// Result of batch file import
#[derive(Debug, Serialize)]
pub struct BatchImportResult {
    pub total_files: usize,
    pub successful: usize,
    pub failed: usize,
    pub results: Vec<FileImportResult>,
}

/// Progress event forwarded to the frontend during import.
///
/// `database_id` names the database the import writes into — the one its
/// request was routed to — so the event reaches only the window(s) showing
/// that database, and a listener can tell concurrent imports into different
/// databases apart. Omitted when the import targets the daemon's default
/// database (no routing header), matching `emit_routed`'s "no id" case.
#[derive(Debug, Clone, Serialize)]
pub struct ImportProgressEvent {
    pub step: u8,
    pub step_name: String,
    pub message: String,
    pub current: usize,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_id: Option<String>,
}

/// Forward one daemon progress event to the frontend, routed to the window(s)
/// pinned to `database_id` rather than broadcast to every window. `None` (no
/// routing header — the daemon default) goes to the focused window, not to
/// "the default database's window": a window may be pinned to a real id while
/// the gRPC client still sends no header, and the progress must still arrive.
fn forward_progress<R: Runtime>(
    app: &AppHandle<R>,
    event: &nodespace_proto::nodespace::ImportProgressEvent,
    database_id: Option<&str>,
) {
    emit_routed(
        app,
        "import-progress",
        ImportProgressEvent {
            step: event.step as u8,
            step_name: event.step_name.clone(),
            message: event.message.clone(),
            current: event.current as usize,
            total: event.total as usize,
            database_id: database_id.map(str::to_string),
        },
        database_id,
    );
}

/// Import a single markdown file via gRPC ImportService
#[tauri::command]
pub async fn import_markdown_file(
    app: AppHandle,
    grpc: State<'_, GrpcClient>,
    file_path: String,
    options: Option<ImportOptions>,
) -> Result<FileImportResult, String> {
    let (mut client, database_id) = grpc.import_client().await;
    let req = ImportMarkdownRequest {
        file_path: file_path.clone(),
        options: Some(options.unwrap_or_default().into_proto()),
    };

    let mut stream = client
        .import_markdown(Request::new(req))
        .await
        .map_err(|e| e.to_string())?
        .into_inner();

    let mut last_result: Option<FileImportResult> = None;

    while let Some(event) = stream.next().await {
        let event = event.map_err(|e| e.to_string())?;

        forward_progress(&app, &event, database_id.as_deref());

        if event.step == 9 {
            if let Some(r) = event.results.into_iter().next() {
                last_result = Some(FileImportResult {
                    file_path: r.file_path,
                    root_id: if r.root_id.is_empty() {
                        None
                    } else {
                        Some(r.root_id)
                    },
                    nodes_created: r.nodes_created as usize,
                    success: r.success,
                    error: if r.error.is_empty() {
                        None
                    } else {
                        Some(r.error)
                    },
                    collection: if r.collection.is_empty() {
                        None
                    } else {
                        Some(r.collection)
                    },
                    archived: r.archived,
                });
            }
        }
    }

    last_result.ok_or_else(|| "Import stream ended without a result".to_string())
}

/// Import multiple markdown files via gRPC ImportService
#[tauri::command]
pub async fn import_markdown_files(
    app: AppHandle,
    grpc: State<'_, GrpcClient>,
    file_paths: Vec<String>,
    options: Option<ImportOptions>,
) -> Result<BatchImportResult, String> {
    let total_files = file_paths.len();
    let (mut client, database_id) = grpc.import_client().await;
    let req = ImportMarkdownFilesRequest {
        file_paths,
        options: Some(options.unwrap_or_default().into_proto()),
    };

    let mut stream = client
        .import_markdown_files(Request::new(req))
        .await
        .map_err(|e| e.to_string())?
        .into_inner();

    let mut final_results: Vec<FileImportResult> = Vec::new();

    while let Some(event) = stream.next().await {
        let event = event.map_err(|e| e.to_string())?;

        forward_progress(&app, &event, database_id.as_deref());

        if event.step == 9 {
            final_results = event
                .results
                .into_iter()
                .map(|r| FileImportResult {
                    file_path: r.file_path,
                    root_id: if r.root_id.is_empty() {
                        None
                    } else {
                        Some(r.root_id)
                    },
                    nodes_created: r.nodes_created as usize,
                    success: r.success,
                    error: if r.error.is_empty() {
                        None
                    } else {
                        Some(r.error)
                    },
                    collection: if r.collection.is_empty() {
                        None
                    } else {
                        Some(r.collection)
                    },
                    archived: r.archived,
                })
                .collect();
        }
    }

    let successful = final_results.iter().filter(|r| r.success).count();
    let failed = final_results.len().saturating_sub(successful);

    Ok(BatchImportResult {
        total_files,
        successful,
        failed,
        results: final_results,
    })
}

/// Import markdown from a directory — collects .md files and delegates to batch import
#[tauri::command]
pub async fn import_markdown_directory(
    app: AppHandle,
    grpc: State<'_, GrpcClient>,
    directory_path: String,
    options: Option<ImportOptions>,
) -> Result<BatchImportResult, String> {
    use std::path::PathBuf;

    let dir = PathBuf::from(&directory_path);
    if !dir.exists() {
        return Err("Directory does not exist".to_string());
    }
    if !dir.is_dir() {
        return Err("Path is not a directory".to_string());
    }

    let dir = dir
        .canonicalize()
        .map_err(|e| format!("Failed to resolve directory: {}", e))?;
    check_within_allowed_base(&dir)?;

    let mut opts = options.unwrap_or_default();
    let exclude_patterns = opts.exclude_patterns.clone();
    let filters = WalkFilters::from_options(&opts);

    // Set base_directory if not provided
    if opts.base_directory.is_none() {
        opts.base_directory = Some(directory_path.clone());
    }

    let mut md_files: Vec<String> = Vec::new();
    collect_markdown_files_with_exclusions(&dir, &mut md_files, &exclude_patterns, filters)?;
    md_files.sort();

    tracing::info!(
        "Found {} markdown files in {} (excluded patterns: {:?})",
        md_files.len(),
        directory_path,
        exclude_patterns
    );

    import_markdown_files(app, grpc, md_files, Some(opts)).await
}

/// Reject import directories outside the user's home directory.
///
/// Defense-in-depth: under the current single-user-local trust model the
/// Tauri command boundary is already restricted to the daemon owner, but this
/// keeps a directory-import call from walking arbitrary system paths (e.g.
/// `/etc`, `/System`) if that boundary is ever weakened. `dir` must already be
/// canonicalized so symlink/`..` tricks can't escape the check.
fn check_within_allowed_base(dir: &std::path::Path) -> Result<(), String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())?;
    let home = home
        .canonicalize()
        .map_err(|e| format!("Failed to resolve home directory: {}", e))?;

    if !dir.starts_with(&home) {
        return Err(format!(
            "Import directory must be within the home directory ({})",
            home.display()
        ));
    }

    Ok(())
}

fn collect_markdown_files_with_exclusions(
    dir: &std::path::Path,
    files: &mut Vec<String>,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let path = entry.path();

        let path_str = path.to_string_lossy();
        let should_exclude = exclude_patterns.iter().any(|pattern| {
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

        // Skip hidden entries (files and dirs) whose name starts with '.'
        // (e.g. .git/, .claude/, dotfiles) before any descent or push.
        if filters.skip_hidden && is_hidden(&path) {
            tracing::debug!("Skipping hidden entry: {}", path_str);
            continue;
        }

        // Use symlink_metadata (lstat semantics) rather than path.is_dir()/is_file(),
        // which follow symlinks: a symlink inside the confined root could otherwise
        // point outside the allowed base and bypass check_within_allowed_base.
        let metadata = entry
            .metadata()
            .map_err(|e| format!("Failed to read metadata for {}: {}", path_str, e))?;

        if metadata.is_symlink() {
            tracing::debug!("Skipping symlink: {}", path_str);
            continue;
        }

        if metadata.is_dir() {
            if filters.recursive {
                collect_markdown_files_with_exclusions(&path, files, exclude_patterns, filters)?;
            }
        } else if metadata.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
            if filters.exclude_agent_files && is_agent_file(&path) {
                tracing::debug!("Skipping agent file: {}", path_str);
                continue;
            }
            if let Some(s) = path.to_str() {
                files.push(s.to_string());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const ALL_ON: WalkFilters = WalkFilters {
        exclude_agent_files: true,
        skip_hidden: true,
        recursive: true,
    };

    #[test]
    fn accepts_home_directory_itself() {
        let home = dirs::home_dir().unwrap().canonicalize().unwrap();
        assert!(check_within_allowed_base(&home).is_ok());
    }

    #[test]
    fn accepts_subdirectory_of_home() {
        let home = dirs::home_dir().unwrap().canonicalize().unwrap();
        assert!(check_within_allowed_base(&home.join("Documents")).is_ok());
    }

    #[test]
    fn rejects_path_outside_home() {
        let outside = std::path::Path::new("/etc");
        let result = check_within_allowed_base(outside);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("home directory"));
    }

    #[test]
    fn rejects_root() {
        let result = check_within_allowed_base(std::path::Path::new("/"));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_sibling_with_home_as_string_prefix() {
        // Regression guard for a component-wise Path::starts_with vs a naive
        // string-prefix check: "/Users/malibio-other" starts with the string
        // "/Users/malibio" but is not actually inside that directory.
        let home = dirs::home_dir().unwrap().canonicalize().unwrap();
        let sibling = home.with_file_name(format!(
            "{}-other",
            home.file_name().unwrap().to_str().unwrap()
        ));

        let result = check_within_allowed_base(&sibling);
        assert!(result.is_err());
    }

    #[test]
    fn does_not_follow_symlinked_directories_during_collection() {
        let tmp =
            std::env::temp_dir().join(format!("ns-import-symlink-test-{}", std::process::id()));
        let import_root = tmp.join("import_root");
        let outside = tmp.join("outside");
        let outside_file = outside.join("secret.md");
        let link = import_root.join("escape");

        std::fs::create_dir_all(&import_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(&outside_file, "# secret").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, &link).unwrap();

        let mut files = Vec::new();
        collect_markdown_files_with_exclusions(&import_root, &mut files, &[], ALL_ON).unwrap();

        assert!(
            files.is_empty(),
            "symlinked directory outside the import root should not be traversed, found: {:?}",
            files
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// Build a fixture tree covering every filter axis. Mirrors the CLI test
    /// fixture; this collection path additionally skips symlinks.
    ///   top.md            top-level plain doc
    ///   CLAUDE.md         top-level agent file (mixed case)
    ///   AGENTS.md         top-level agent file
    ///   DESIGN.md         top-level design file
    ///   .dotfile.md       top-level hidden dotfile doc
    ///   sub/nested.md     doc in a sub-folder
    ///   sub/claude.md     nested, lowercase agent file (case-insensitive + depth)
    ///   sub/design.md     nested, lowercase design file (case-insensitive + depth)
    ///   .hidden/hidden.md doc under a hidden directory
    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("top.md"), "# top").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "# claude").unwrap();
        std::fs::write(root.join("AGENTS.md"), "# agents").unwrap();
        std::fs::write(root.join("DESIGN.md"), "# design").unwrap();
        std::fs::write(root.join(".dotfile.md"), "# dotfile").unwrap();

        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("nested.md"), "# nested").unwrap();
        std::fs::write(sub.join("claude.md"), "# nested claude").unwrap();
        std::fs::write(sub.join("design.md"), "# nested design").unwrap();

        let hidden = root.join(".hidden");
        std::fs::create_dir(&hidden).unwrap();
        std::fs::write(hidden.join("hidden.md"), "# hidden").unwrap();

        tmp
    }

    fn names(
        tmp: &tempfile::TempDir,
        exclude: &[String],
        filters: WalkFilters,
    ) -> BTreeSet<String> {
        let mut files = Vec::new();
        collect_markdown_files_with_exclusions(tmp.path(), &mut files, exclude, filters).unwrap();
        files
            .into_iter()
            .map(|p| {
                std::path::Path::new(&p)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_exclude_agent_and_hidden_but_recurse() {
        let tmp = fixture();
        assert_eq!(names(&tmp, &[], ALL_ON), set(&["top.md", "nested.md"]));
    }

    #[test]
    fn including_agent_files_keeps_claude_and_agents_at_any_depth() {
        let tmp = fixture();
        let filters = WalkFilters {
            exclude_agent_files: false,
            ..ALL_ON
        };
        assert_eq!(
            names(&tmp, &[], filters),
            set(&[
                "top.md",
                "nested.md",
                "CLAUDE.md",
                "AGENTS.md",
                "DESIGN.md",
                "claude.md",
                "design.md",
            ])
        );
    }

    #[test]
    fn including_hidden_keeps_dotfiles_and_dot_dirs() {
        let tmp = fixture();
        let filters = WalkFilters {
            skip_hidden: false,
            ..ALL_ON
        };
        assert_eq!(
            names(&tmp, &[], filters),
            set(&["top.md", "nested.md", ".dotfile.md", "hidden.md"])
        );
    }

    #[test]
    fn non_recursive_scans_only_top_level() {
        let tmp = fixture();
        let filters = WalkFilters {
            recursive: false,
            ..ALL_ON
        };
        assert_eq!(names(&tmp, &[], filters), set(&["top.md"]));
    }

    #[test]
    fn all_filters_off_collects_everything() {
        let tmp = fixture();
        let filters = WalkFilters {
            exclude_agent_files: false,
            skip_hidden: false,
            recursive: true,
        };
        assert_eq!(
            names(&tmp, &[], filters),
            set(&[
                "top.md",
                "nested.md",
                "CLAUDE.md",
                "AGENTS.md",
                "DESIGN.md",
                "claude.md",
                "design.md",
                ".dotfile.md",
                "hidden.md",
            ])
        );
    }

    #[test]
    fn filters_compose_with_exclude_patterns() {
        let tmp = fixture();
        assert_eq!(names(&tmp, &["sub".to_string()], ALL_ON), set(&["top.md"]));
    }

    fn proto_progress(step: u32) -> nodespace_proto::nodespace::ImportProgressEvent {
        nodespace_proto::nodespace::ImportProgressEvent {
            step,
            step_name: "reading".to_string(),
            message: "Reading: a.md".to_string(),
            current: 1,
            total: 3,
            results: Vec::new(),
        }
    }

    /// Two windows pinned to two databases: an import into db-1 must report
    /// progress only to db-1's window, tagged with db-1's id — the payload
    /// alone must be enough to tell concurrent imports apart.
    #[test]
    fn forward_progress_reaches_only_the_window_showing_the_import_database() {
        use crate::window_routing::WindowDatabaseRegistry;
        use std::sync::{Arc, Mutex};
        use tauri::{Listener, Manager};

        let app = tauri::test::mock_app();
        app.manage(WindowDatabaseRegistry::default());
        let handle = app.handle().clone();

        let win_a = tauri::WebviewWindowBuilder::new(&app, "win-a", Default::default())
            .build()
            .expect("failed to build mock window a");
        let win_b = tauri::WebviewWindowBuilder::new(&app, "win-b", Default::default())
            .build()
            .expect("failed to build mock window b");
        let registry = handle.state::<WindowDatabaseRegistry>();
        registry.pin("win-a", "db-1");
        registry.pin("win-b", "db-2");

        let received_a: Arc<Mutex<Vec<String>>> = Arc::default();
        let received_b: Arc<Mutex<Vec<String>>> = Arc::default();
        let ra = received_a.clone();
        let rb = received_b.clone();
        win_a.listen("import-progress", move |e| {
            ra.lock().unwrap().push(e.payload().to_string())
        });
        win_b.listen("import-progress", move |e| {
            rb.lock().unwrap().push(e.payload().to_string())
        });

        forward_progress(&handle, &proto_progress(2), Some("db-1"));

        let received_a = received_a.lock().unwrap();
        assert_eq!(received_a.len(), 1, "db-1's window must receive progress");
        let payload: serde_json::Value = serde_json::from_str(&received_a[0]).unwrap();
        assert_eq!(payload["database_id"], "db-1");
        assert_eq!(payload["step_name"], "reading");
        assert!(
            received_b.lock().unwrap().is_empty(),
            "db-2's window must not see db-1's import progress"
        );
    }

    /// The path production takes today: the window is pinned to a real
    /// database id but the import ran with no routing header (`None`). The
    /// progress must still reach that window rather than being dropped for
    /// naming no pinned database.
    #[test]
    fn forward_progress_without_database_id_reaches_the_pinned_window() {
        use crate::window_routing::WindowDatabaseRegistry;
        use std::sync::{Arc, Mutex};
        use tauri::{Listener, Manager};

        let app = tauri::test::mock_app();
        app.manage(WindowDatabaseRegistry::default());
        let handle = app.handle().clone();

        let win = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("failed to build mock window");
        handle
            .state::<WindowDatabaseRegistry>()
            .pin("main", "db-remembered");

        let received: Arc<Mutex<Vec<String>>> = Arc::default();
        let r = received.clone();
        win.listen("import-progress", move |e| {
            r.lock().unwrap().push(e.payload().to_string())
        });

        forward_progress(&handle, &proto_progress(2), None);

        assert_eq!(
            received.lock().unwrap().len(),
            1,
            "an unrouted import's progress must reach the one pinned window"
        );
    }

    /// An import into the daemon's default database (no routing header)
    /// omits `database_id` rather than sending an empty string or null.
    #[test]
    fn progress_event_omits_database_id_for_default_database() {
        let event = ImportProgressEvent {
            step: 2,
            step_name: "reading".to_string(),
            message: String::new(),
            current: 0,
            total: 0,
            database_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("database_id").is_none());
    }

    #[test]
    fn walk_filters_from_options_inverts_opt_out_fields() {
        // Default options (all opt-out fields false) => every filter on.
        let f = WalkFilters::from_options(&ImportOptions::default());
        assert!(f.exclude_agent_files && f.skip_hidden && f.recursive);

        // Setting the opt-out fields disables each filter.
        let opts = ImportOptions {
            include_agent_files: true,
            include_hidden: true,
            no_recursion: true,
            ..ImportOptions::default()
        };
        let f = WalkFilters::from_options(&opts);
        assert!(!f.exclude_agent_files && !f.skip_hidden && !f.recursive);
    }
}
