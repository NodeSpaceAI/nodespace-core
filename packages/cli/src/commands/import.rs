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
) -> Result<ImportMarkdownRequest> {
    Ok(ImportMarkdownRequest {
        file_path: paths::absolute_in(&file, cwd)?,
        options: Some(options),
    })
}

/// Walk `dir` (as the user typed it, so `--exclude` patterns match exactly what
/// the user sees and never the ancestors of the cwd) and return the absolute
/// base directory plus the absolute paths of the files found. The daemon
/// resolves paths against its own cwd, so only absolute paths go on the wire.
fn scan_directory(
    dir: &str,
    cwd: &std::path::Path,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<(String, Vec<String>)> {
    let base = paths::absolute_in(dir, cwd)?;
    let files = walk_markdown_files(
        std::path::Path::new(&base),
        std::path::Path::new(dir),
        exclude_patterns,
        filters,
    )?;
    Ok((base, files))
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

    let request = import_file_request(args.file, opts, &std::env::current_dir()?)?;
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
    let (directory, file_paths) = scan_directory(
        &directory,
        &std::env::current_dir()?,
        &args.exclude_patterns,
        filters,
    )?;

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
    // Phase 1: scan every directory before issuing any RPC, so an unreadable
    // directory is reported alongside the others. The daemon keys each
    // document by its source directory as well as its relative path, so
    // same-named files in different directories import as separate documents.
    let mut scanned: Vec<(String, Vec<String>)> = Vec::new();
    let mut all_results: Vec<FileImportResult> = Vec::new();

    let cwd = std::env::current_dir()?;
    for directory in &args.directories {
        match scan_directory(directory, &cwd, &args.exclude_patterns, filters) {
            Ok(scanned_dir) => scanned.push(scanned_dir),
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

#[cfg(test)]
fn collect_markdown_files(
    dir: &str,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<Vec<String>> {
    let root = std::path::Path::new(dir);
    walk_markdown_files(root, root, exclude_patterns, filters)
}

/// Walk `walk_root` and report its files, matching `--exclude` patterns
/// against each entry's path as spelled under `display_root` (the directory as
/// the user typed it) so ancestors of the walk root never match a pattern.
fn walk_markdown_files(
    walk_root: &std::path::Path,
    display_root: &std::path::Path,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_recursive(
        walk_root,
        (walk_root, display_root),
        &mut files,
        exclude_patterns,
        filters,
    )?;
    files.sort();
    Ok(files)
}

fn collect_recursive(
    dir: &std::path::Path,
    roots: (&std::path::Path, &std::path::Path),
    files: &mut Vec<String>,
    exclude_patterns: &[String],
    filters: WalkFilters,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let shown = roots.1.join(path.strip_prefix(roots.0).unwrap_or(&path));
        let path_str = shown.to_string_lossy();

        // User-supplied --exclude patterns (component name or path substring),
        // preserved and composed with the new default-on filters below.
        let excluded = exclude_patterns.iter().any(|p| {
            shown.components().any(|c| {
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

        // lstat semantics: a symlink is never descended into or imported, so a
        // link back to the root or an ancestor cannot re-enter the tree.
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            if filters.recursive {
                collect_recursive(&path, roots, files, exclude_patterns, filters)?;
            }
        } else if file_type.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
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
        )
        .unwrap();
        assert_eq!(req.file_path, "/home/u/project/adr.md");
    }

    fn filters() -> WalkFilters {
        WalkFilters {
            exclude_agent_files: true,
            skip_hidden: true,
            recursive: true,
        }
    }

    #[test]
    fn scan_sends_absolute_base_and_files_under_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("docs")).unwrap();
        std::fs::write(tmp.path().join("docs/a.md"), "# a").unwrap();
        let (base, files) = scan_directory("./docs", tmp.path(), &[], filters()).unwrap();
        assert_eq!(base, tmp.path().join("docs").to_string_lossy());
        assert_eq!(files.len(), 1);
        assert!(Path::new(&files[0]).starts_with(&base));
    }

    #[test]
    fn scan_is_run_per_directory_for_multi_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        for d in ["one", "two"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
            std::fs::write(tmp.path().join(d).join("x.md"), "# x").unwrap();
        }
        for d in ["one", "two"] {
            let (base, files) = scan_directory(d, tmp.path(), &[], filters()).unwrap();
            assert!(Path::new(&base).is_absolute());
            assert!(files.iter().all(|f| Path::new(f).starts_with(&base)));
            assert_eq!(files.len(), 1);
        }
    }

    #[test]
    fn exclude_does_not_match_cwd_ancestors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cwd = tmp.path().join("archive");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("a.md"), "# a").unwrap();
        let (_, files) = scan_directory(".", &cwd, &["archive".to_string()], filters()).unwrap();
        assert_eq!(files.len(), 1, "cwd ancestor name must not exclude files");
    }

    #[test]
    fn exclude_still_matches_entries_under_the_walked_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("archive")).unwrap();
        std::fs::write(tmp.path().join("archive/old.md"), "# o").unwrap();
        std::fs::write(tmp.path().join("keep.md"), "# k").unwrap();
        let (_, files) =
            scan_directory(".", tmp.path(), &["archive".to_string()], filters()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("keep.md"));
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

    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_directory_symlinks() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.md"), "# a").unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("b.md"), "# b").unwrap();
        // Cycle back to the root and a link to a sibling directory.
        std::os::unix::fs::symlink(".", root.join("latest")).unwrap();
        std::os::unix::fs::symlink("sub", root.join("alias")).unwrap();

        let files = collect_markdown_files(root.to_str().unwrap(), &[], ALL_ON).unwrap();
        let rel: Vec<String> = files
            .iter()
            .map(|f| {
                std::path::Path::new(f)
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(rel, vec!["a.md".to_string(), "sub/b.md".to_string()]);
    }
}
