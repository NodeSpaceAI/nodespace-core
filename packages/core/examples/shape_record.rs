//! The built-in shape record's tooling (ADR-104).
//!
//!   generate        write `shape-record/current.json` from this build
//!   check           the gate: fail when the snapshot or the change log is out of step
//!   release <name>  copy `current.json` to `shape-record/snapshots/<name>.json`
//!
//! Usage: `cargo run -q -p nodespace-core --example shape_record -- <command>`
//! (`bun run shape:gen`, `shape:check`, `shape:release`).

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use nodespace_core::shape_record::gate::{check, Inputs};
use nodespace_core::shape_record::snapshot::generate;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shape-record")
}

fn read(path: PathBuf) -> Result<String, String> {
    fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn releases() -> Result<Vec<(String, String)>, String> {
    let dir = dir().join("snapshots");
    let mut releases = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        releases.push((name, read(path)?));
    }
    Ok(releases)
}

async fn run(args: &[String]) -> Result<(), String> {
    let generated = generate().await.map_err(|e| format!("{e:#}"))?;
    match args.first().map(String::as_str) {
        Some("generate") => {
            fs::write(dir().join("current.json"), &generated).map_err(|e| e.to_string())?;
            println!("wrote shape-record/current.json");
            Ok(())
        }
        Some("check") => {
            let current = read(dir().join("current.json"))?;
            let change_log = read(dir().join("change-log.json"))?;
            let problems = check(&Inputs {
                releases: releases()?,
                current: &current,
                generated: &generated,
                change_log: &change_log,
            });
            if problems.is_empty() {
                println!("shape-record: the snapshot and the change log are in step");
                return Ok(());
            }
            Err(format!(
                "The built-in shape record is out of step:\n{}\n\nSee the Built-In Shape Record page in the \
                 agent guide: regenerate with `bun run shape:gen` and add an entry to \
                 packages/core/shape-record/change-log.json for each difference.",
                problems
                    .iter()
                    .map(|p| format!("  - {p}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ))
        }
        Some("release") => {
            let name = args.get(1).ok_or("release needs a name, e.g. 0.3.10")?;
            let target = dir().join("snapshots").join(format!("{name}.json"));
            if target.exists() {
                return Err(format!("{} already exists", target.display()));
            }
            fs::write(&target, read(dir().join("current.json"))?).map_err(|e| e.to_string())?;
            println!("wrote {}", target.display());
            Ok(())
        }
        _ => Err("usage: shape_record generate | check | release <name>".to_string()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
