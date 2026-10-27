//! `nodespace import` — import markdown files into NodeSpace via the daemon.

use anyhow::Result;
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    FileImportResult, ImportMarkdownFilesRequest, ImportMarkdownRequest, ImportOptions,
};
use tokio_stream::StreamExt;
use tonic::Request;

use crate::paths;
use crate::ImportClient;

#[derive(Subcommand, Debug)]
pub enum ImportAction {
    /// Import a single markdown file.
    File(ImportFileArgs),
    /// Import all markdown files from a directory (recurses into sub-folders by
    /// default; see --no-recursive).
    Dir(ImportDirArgs),
}

#[derive(Args, Debug)]
pub struct ImportFileArgs {
    /// Path to the markdown file.
    pub file: String,

    /// Collection path to assign the document to (e.g. "docs:rust").
    #[arg(long)]
    pub collection: Option<String>,

    /// Use the filename stem as the document title.
    #[arg(long)]
    pub use_filename_as_title: bool,

    /// Route files to collections based on directory structure.
    #[arg(long)]
    pub auto_collection_routing: bool,

    /// Refresh an already-imported document in place: replace its child subtree
    /// from the fresh parse, keeping the root node so inbound links survive.
    /// Without this, an already-imported document is left untouched.
    #[arg(long)]
    pub replace: bool,
}

#[derive(Args, Debug)]
pub struct ImportDirArgs {
    /// Path(s) to the directory containing markdown files. Pass more than
    /// one to import several directories in a single call: each directory
    /// is walked and routed relative to its own root, and results are
    /// combined into one summary rather than reported per directory.
    #[arg(required = true, num_args = 1..)]
    pub directories: Vec<String>,

    /// Collection path to assign all documents to.
    #[arg(long)]
    pub collection: Option<String>,

    /// Use filename stems as document titles.
    #[arg(long)]
    pub use_filename_as_title: bool,

    /// Route files to collections based on directory structure.
    #[arg(long)]
    pub auto_collection_routing: bool,

    /// Directory names to exclude (repeatable, e.g. --exclude node_modules).
    #[arg(long = "exclude")]
    pub exclude_patterns: Vec<String>,

    /// Include CLAUDE.md / AGENTS.md / DESIGN.md files (default: excluded).
    /// Matched by basename, case-insensitive, at any depth.
    #[arg(long)]
    pub include_agent_files: bool,

    /// Include hidden files and folders — any path component starting with '.',
    /// e.g. .git/, .claude/, dotfiles (default: skipped).
    #[arg(long)]
    pub include_hidden: bool,

    /// Import only the top-level directory; do not descend into sub-folders
    /// (default: recurses into sub-folders).
    #[arg(long)]
    pub no_recursive: bool,

    /// Refresh already-imported documents in place: replace each existing
    /// document's child subtree from the fresh parse, keeping its root node so
    /// inbound links survive. Without this, already-imported documents are
    /// skipped (a plain re-import never duplicates).
    #[arg(long)]
    pub replace: bool,
}

pub async fn run(client: &mut ImportClient, action: ImportAction, json: bool) -> Result<()> {
    match action {
        ImportAction::File(args) => run_file(client, args, json).await,
        ImportAction::Dir(args) => run_dir(client, args, json).await,
    }
}

fn results_to_json(results: &[FileImportResult]) -> serde_json::Value {
    serde_json::Value::Array(
        results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "file_path": r.file_path,
                    "root_id": r.root_id,
                    "nodes_created": r.nodes_created,
                    "success": r.success,
                    "error": r.error,
                    "collection": r.collection,
                    "archived": r.archived,
                })
            })
            .collect(),
    )
}

/// Build the single-file import request with the file path resolved against
/// the caller's `cwd`, since the daemon resolves paths against its own.
fn import_file_request(
    file: String,
    options: ImportOptions,
    cwd: &std::path::Path,
) -> ImportMarkdownRequest {
    ImportMarkdownRequest {
        file_path: paths::absolute_in(&file, cwd),
        options: Some(options),
    }
}

/// Resolve every directory argument against the caller's `cwd` so the walked
/// file paths and `base_directory` sent to the daemon are absolute.
fn absolutize_directories(directories: &mut [String], cwd: &std::path::Path) {
    for d in directories.iter_mut() {
        *d = paths::absolute_in(d, cwd);
    }
}

async fn run_file(client: &mut ImportClient, args: ImportFileArgs, json: bool) -> Result<()> {
    let opts = ImportOptions {
        collection: args.collection.unwrap_or_default(),
        use_filename_as_title: args.use_filename_as_title,
        auto_collection_routing: args.auto_collection_routing,
        exclude_patterns: vec![],
        base_directory: String::new(),
        replace: args.replace,
        // A single explicit file is never walked, so the folder-walk filters
        // are inert here; carry their proto defaults for a uniform option surface.
        include_agent_files: false,
        include_hidden: false,
        no_recursion: false,
    };

    let request = import_file_request(args.file, opts, &std::env::current_dir()?);
    let mut stream = client
        .import_markdown(Request::new(request))
        .await?
        .into_inner();

    while let Some(event) = stream.next().await {
        let event = event?;
        if json {
            if event.step == 9 {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&results_to_json(&event.results))?
                );
            }
        } else {
            eprintln!("[{}/9] {}: {}", event.step, event.step_name, event.message);
            if event.step == 9 {
                for r in &event.results {
                    if r.success {
                        println!(
                            "✓ {} ({} nodes){}",
                            r.file_path,
                            r.nodes_created,
                            if r.collection.is_empty() {
                                String::new()
                            } else {
                                format!(" → {}", r.collection)
                            }
                        );
                    } else {
                        println!("✗ {}: {}", r.file_path, r.error);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Dispatches on directory count: a single directory keeps behaving exactly
/// as before this flag existed (`run_one_dir` is that original code path,
/// unmodified — including a scan failure aborting the whole command). Two or
/// more directories go through `run_multi_dir`, which walks and imports each
/// one against its own `base_directory` (so `--auto-collection-routing`
/// stays relative to each directory's own root, never a synthesised common
/// ancestor) and combines the results into one summary instead of one per
/// directory. A per-directory scan or import failure there is recorded and
/// the remaining directories still run.
async fn run_dir(client: &mut ImportClient, mut args: ImportDirArgs, json: bool) -> Result<()> {
    absolutize_directories(&mut args.directories, &std::env::current_dir()?);
    let filters = WalkFilters {
        exclude_agent_files: !args.include_agent_files,
        skip_hidden: !args.include_hidden,
        recursive: !args.no_recursive,
    };

    if args.directories.len() == 1 {
        // `mem::take` rather than `args.directories.into_iter().next()` so
        // `args` (used below for its other fields) is never partially moved.
        let directory = std::mem::take(&mut args.directories)
            .into_iter()
            .next()
            .expect("len checked above");
        return run_one_dir(client, directory, args, filters, json).await;
    }

    run_multi_dir(client, args, filters, json).await
}

/// Import a single directory. This is the original single-directory behavior
/// verbatim: a directory that fails to scan (doesn't exist, unreadable, ...) aborts the
/// whole command via `?` rather than being reported as a partial failure —
/// `run_multi_dir` is deliberately more lenient about that, but a single
/// explicit directory should still fail loudly.
async fn run_one_dir(
    client: &mut ImportClient,
    directory: String,
    args: ImportDirArgs,
    filters: WalkFilters,
    json: bool,
) -> Result<()> {
    let file_paths = collect_markdown_files(&directory, &args.exclude_patterns, filters)?;

    if file_paths.is_empty() {
        if !json {
            eprintln!("No markdown files found in {}", directory);
        }
        return Ok(());
    }

    let opts = ImportOptions {
        collection: args.collection.unwrap_or_default(),
        use_filename_as_title: args.use_filename_as_title,
        auto_collection_routing: args.auto_collection_routing,
        exclude_patterns: args.exclude_patterns,
        base_directory: directory,
        replace: args.replace,
        include_agent_files: args.include_agent_files,
        include_hidden: args.include_hidden,
        no_recursion: args.no_recursive,
    };

    let mut stream = client
        .import_markdown_files(Request::new(ImportMarkdownFilesRequest {
            file_paths,
            options: Some(opts),
        }))
        .await?
        .into_inner();

    while let Some(event) = stream.next().await {
        let event = event?;
        if json {
            if event.step == 9 {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&results_to_json(&event.results))?
                );
            }
        } else {
            eprintln!("[{}/9] {}: {}", event.step, event.step_name, event.message);
            if event.step == 9 {
                // The daemon's summary is the source of truth — it distinguishes
                // newly imported, refreshed (--replace), and already-present
                // (skipped) documents, so a plain re-import reads honestly
                // instead of claiming to have re-imported everything.
                println!("{}", event.message);
                for r in event.results.iter().filter(|r| !r.success) {
                    println!("  ✗ {}: {}", r.file_path, r.error);
                }
            }
        }
    }

    Ok(())
}

/// Import several directories in one call, combining their results into one
/// summary instead of reporting one per directory. Each directory is walked
/// and imported against its own `base_directory`, so `--auto-collection-routing`
/// stays relative to that directory's own root rather than a synthesised
/// common ancestor across every directory passed. A directory that fails to
/// scan (doesn't exist, unreadable, ...) or whose import RPC fails is
/// recorded as a failure in the combined results; it does not stop the rest
/// from running. Only ever called with 2+ directories — `run_dir` routes a
/// single directory to `run_one_dir` instead.
async fn run_multi_dir(
    client: &mut ImportClient,
    args: ImportDirArgs,
    filters: WalkFilters,
    json: bool,
) -> Result<()> {
    // Phase 1: scan every directory before issuing any RPC. The daemon
    // derives each document's deterministic id from its path *relative to
    // that call's own base_directory* (one independent RPC per directory
    // here), so two different directories that each contain a same-named
    // file (e.g. both have a top-level README.md) would otherwise collide on
    // identity: the second either gets silently skipped as "already
    // imported" or, with --replace, silently overwrites the first's content.
    // Scanning everything up front lets that collision be caught and
    // reported before anything is written, rather than after.
    let mut scanned: Vec<(String, Vec<String>)> = Vec::new();
    let mut all_results: Vec<FileImportResult> = Vec::new();

    for directory in &args.directories {
        match collect_markdown_files(directory, &args.exclude_patterns, filters) {
            Ok(file_paths) => scanned.push((directory.clone(), file_paths)),
            Err(e) => {
                if !json {
                    eprintln!("✗ {}: {}", directory, e);
                }
                all_results.push(directory_failure_result(
                    directory,
                    &format!("Failed to scan directory: {e}"),
                ));
            }
        }
    }

    if let Some(collisions) = find_cross_directory_collisions(&scanned) {
        return Err(anyhow::anyhow!(
            "Refusing to import: {} file(s) share the same identity across directories \
             (same path relative to their own directory's root) — importing would silently \
             skip or overwrite one with the other:\n{}",
            collisions.len(),
            collisions.join("\n")
        ));
    }

    // Phase 2: one ImportMarkdownFiles RPC per directory that scanned
    // successfully and has at least one file.
    for (directory, file_paths) in scanned {
        if file_paths.is_empty() {
            if !json {
                eprintln!("No markdown files found in {}", directory);
            }
            continue;
        }

        let opts = ImportOptions {
            collection: args.collection.clone().unwrap_or_default(),
            use_filename_as_title: args.use_filename_as_title,
            auto_collection_routing: args.auto_collection_routing,
            exclude_patterns: args.exclude_patterns.clone(),
            base_directory: directory.clone(),
            replace: args.replace,
            include_agent_files: args.include_agent_files,
            include_hidden: args.include_hidden,
            no_recursion: args.no_recursive,
        };

        match stream_import(client, file_paths, opts, json).await {
            Ok(mut results) => all_results.append(&mut results),
            Err(e) => {
                if !json {
                    eprintln!("✗ {}: {}", directory, e);
                }
                all_results.push(directory_failure_result(&directory, &e.to_string()));
            }
        }
    }

    let succeeded = all_results.iter().filter(|r| r.success).count();
    let failed = all_results.len() - succeeded;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&results_to_json(&all_results))?
        );
    } else {
        let nodes: u32 = all_results
            .iter()
            .map(|r| u64::from(r.nodes_created))
            .sum::<u64>()
            .min(u64::from(u32::MAX)) as u32;
        println!(
            "Imported {} file(s) across {} directories ({} nodes){}",
            succeeded,
            args.directories.len(),
            nodes,
            if failed > 0 {
                format!(", {failed} failed")
            } else {
                String::new()
            }
        );
        for r in all_results.iter().filter(|r| !r.success) {
            println!("  ✗ {}: {}", r.file_path, r.error);
        }
    }

    // Match run_one_dir's contract: a directory that could not be imported
    // aborts the command with a non-zero exit — the earlier per-directory
    // resilience is about not letting one bad directory stop the *others*
    // from being attempted, not about the process reporting success when
    // something genuinely failed.
    if failed > 0 {
        return Err(anyhow::anyhow!(
            "{failed} of {} file(s)/director{} failed to import",
            all_results.len(),
            if all_results.len() == 1 { "y" } else { "ies" }
        ));
    }

    Ok(())
}

/// A file's import identity key as the daemon computes it: its path relative
/// to the `base_directory` that will accompany it on the wire (mirrors
/// `import_key` in `packages/daemon/src/services/import_service.rs`).
fn relative_identity_key(file_path: &str, base_directory: &str) -> String {
    std::path::Path::new(file_path)
        .strip_prefix(base_directory)
        .unwrap_or_else(|_| std::path::Path::new(file_path))
        .to_string_lossy()
        .into_owned()
}

/// Cross-directory identity collisions among files that already scanned
/// successfully: two different directories each holding a file whose path
/// *relative to its own directory* is identical (e.g. both have a top-level
/// `README.md`), which would compute the same deterministic root id server
/// side. Returns `None` when there are none.
fn find_cross_directory_collisions(scanned: &[(String, Vec<String>)]) -> Option<Vec<String>> {
    let mut seen: std::collections::HashMap<String, (&str, &str)> =
        std::collections::HashMap::new();
    let mut collisions = Vec::new();

    for (directory, file_paths) in scanned {
        for file_path in file_paths {
            let key = relative_identity_key(file_path, directory);
            match seen.get(&key) {
                Some((prev_dir, prev_file)) => {
                    collisions.push(format!(
                        "  \"{key}\": {prev_file} (under {prev_dir}) and {file_path} (under {directory})"
                    ));
                }
                None => {
                    seen.insert(key, (directory.as_str(), file_path.as_str()));
                }
            }
        }
    }

    if collisions.is_empty() {
        None
    } else {
        Some(collisions)
    }
}

/// Stream one `ImportMarkdownFiles` call to completion, printing per-step
/// progress as it happens (human mode only), and returning the terminal
/// (step 9) event's per-file results. Used by `run_multi_dir`, which defers
/// printing any per-directory summary text so results from every directory
/// combine into one summary at the end instead of one per directory.
async fn stream_import(
    client: &mut ImportClient,
    file_paths: Vec<String>,
    opts: ImportOptions,
    json: bool,
) -> Result<Vec<FileImportResult>> {
    let mut stream = client
        .import_markdown_files(Request::new(ImportMarkdownFilesRequest {
            file_paths,
            options: Some(opts),
        }))
        .await?
        .into_inner();

    let mut final_results = Vec::new();
    while let Some(event) = stream.next().await {
        let event = event?;
        if !json {
            eprintln!("[{}/9] {}: {}", event.step, event.step_name, event.message);
        }
        if event.step == 9 {
            final_results = event.results;
        }
    }
    Ok(final_results)
}

/// A synthetic per-file result standing in for a directory-level failure
/// (couldn't be scanned, or the import RPC itself failed), so it surfaces in
/// the combined results/summary the same way a single file's failure does,
/// with no separate handling needed downstream.
fn directory_failure_result(directory: &str, error: &str) -> FileImportResult {
    FileImportResult {
        file_path: directory.to_string(),
        root_id: String::new(),
        nodes_created: 0,
        success: false,
        error: error.to_string(),
        collection: String::new(),
        archived: false,
    }
}

/// Filters applied while walking a directory for import. Each field is stated
/// in its active/default-on sense (the folder-import options all default ON),
/// so the fields are true when the corresponding filter is applied.
#[derive(Clone, Copy)]
struct WalkFilters {
    /// Skip files whose basename is CLAUDE.md / AGENTS.md / DESIGN.md (case-insensitive).
    exclude_agent_files: bool,
    /// Skip any entry (file or dir) whose name starts with '.'.
    skip_hidden: bool,
    /// Descend into sub-folders. When false, only the top-level dir is scanned.
    recursive: bool,
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

fn collect_markdown_files(
    dir: &str,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_recursive(
        std::path::Path::new(dir),
        &mut files,
        exclude_patterns,
        filters,
    )?;
    files.sort();
    Ok(files)
}

fn collect_recursive(
    dir: &std::path::Path,
    files: &mut Vec<String>,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let path_str = path.to_string_lossy();

        // User-supplied --exclude patterns (component name or path substring),
        // preserved and composed with the new default-on filters below.
        let excluded = exclude_patterns.iter().any(|p| {
            path.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|s| s.eq_ignore_ascii_case(p))
                    .unwrap_or(false)
            }) || path_str.contains(p.as_str())
        });

        if excluded {
            continue;
        }

        // Skip hidden entries (files and dirs) before any descent or push.
        if filters.skip_hidden && is_hidden(&path) {
            continue;
        }

        if path.is_dir() {
            if filters.recursive {
                collect_recursive(&path, files, exclude_patterns, filters)?;
            }
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            if filters.exclude_agent_files && is_agent_file(&path) {
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
mod path_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn file_request_path_is_resolved_against_cwd() {
        let req = import_file_request(
            "./adr.md".into(),
            ImportOptions::default(),
            Path::new("/home/u/project"),
        );
        assert_eq!(req.file_path, "/home/u/project/adr.md");
    }

    #[test]
    fn directories_are_resolved_against_cwd() {
        let mut dirs = vec!["./docs".to_string(), "/abs/other".to_string()];
        absolutize_directories(&mut dirs, Path::new("/home/u/project"));
        assert_eq!(dirs, vec!["/home/u/project/docs", "/abs/other"]);
    }

    #[test]
    fn scanned_files_and_base_directory_are_absolute() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.md"), "# a").unwrap();
        let mut dirs = vec![".".to_string()];
        absolutize_directories(&mut dirs, tmp.path());
        let files = collect_markdown_files(
            &dirs[0],
            &[],
            WalkFilters {
                exclude_agent_files: true,
                skip_hidden: true,
                recursive: true,
            },
        )
        .unwrap();
        assert!(Path::new(&dirs[0]).is_absolute());
        assert_eq!(files.len(), 1);
        assert!(Path::new(&files[0]).is_absolute());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    /// Build a fixture tree covering every filter axis:
    ///   top.md            top-level plain doc
    ///   CLAUDE.md         top-level agent file (mixed case)
    ///   AGENTS.md         top-level agent file
    ///   DESIGN.md         top-level design file
    ///   .dotfile.md       top-level hidden dotfile doc
    ///   sub/nested.md     doc in a sub-folder
    ///   sub/claude.md     nested, lowercase agent file (case-insensitive + depth)
    ///   sub/design.md     nested, lowercase design file (case-insensitive + depth)
    ///   .hidden/hidden.md doc under a hidden directory
    fn fixture() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("top.md"), "# top").unwrap();
        fs::write(root.join("CLAUDE.md"), "# claude").unwrap();
        fs::write(root.join("AGENTS.md"), "# agents").unwrap();
        fs::write(root.join("DESIGN.md"), "# design").unwrap();
        fs::write(root.join(".dotfile.md"), "# dotfile").unwrap();

        let sub = root.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.md"), "# nested").unwrap();
        fs::write(sub.join("claude.md"), "# nested claude").unwrap();
        fs::write(sub.join("design.md"), "# nested design").unwrap();

        let hidden = root.join(".hidden");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("hidden.md"), "# hidden").unwrap();

        tmp
    }

    /// Sorted set of basenames from the collected absolute paths.
    fn names(tmp: &TempDir, exclude: &[String], filters: WalkFilters) -> BTreeSet<String> {
        collect_markdown_files(tmp.path().to_str().unwrap(), exclude, filters)
            .unwrap()
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

    const ALL_ON: WalkFilters = WalkFilters {
        exclude_agent_files: true,
        skip_hidden: true,
        recursive: true,
    };

    #[test]
    fn defaults_exclude_agent_and_hidden_but_recurse() {
        let tmp = fixture();
        // Agent files (any depth, any case) and hidden entries are dropped;
        // sub-folders are still walked.
        assert_eq!(names(&tmp, &[], ALL_ON), set(&["top.md", "nested.md"]));
    }

    #[test]
    fn including_agent_files_keeps_claude_and_agents_at_any_depth() {
        let tmp = fixture();
        let filters = WalkFilters {
            exclude_agent_files: false,
            ..ALL_ON
        };
        // Top-level CLAUDE.md/AGENTS.md/DESIGN.md and the nested lowercase
        // claude.md/design.md return; hidden entries stay excluded.
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
        // The dotfile doc and the doc under .hidden/ appear; agent files still drop.
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
        // Only the top-level plain doc; sub/ is not descended into, agent +
        // hidden top-level entries still filtered.
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
        // --exclude sub removes the whole sub-folder; default filters still drop
        // agent + hidden entries, leaving only the top-level plain doc.
        assert_eq!(names(&tmp, &["sub".to_string()], ALL_ON), set(&["top.md"]));
    }

    #[test]
    fn non_recursive_still_excludes_top_level_agent_and_hidden() {
        let tmp = fixture();
        // Turn every filter off except recursion to prove the non-recursive mode
        // is orthogonal: at top level it keeps agent + hidden entries too.
        let filters = WalkFilters {
            exclude_agent_files: false,
            skip_hidden: false,
            recursive: false,
        };
        assert_eq!(
            names(&tmp, &[], filters),
            set(&[
                "top.md",
                "CLAUDE.md",
                "AGENTS.md",
                "DESIGN.md",
                ".dotfile.md"
            ])
        );
    }
}
