//! The gate: keeps the change log complete (ADR-104).
//!
//! It regenerates the snapshot and fails when
//!
//! - the committed `current.json` is not what the build generates;
//! - a difference between the latest release snapshot and the current one has
//!   no change-log entry covering it, or an entry names a path that did not
//!   change;
//! - the log or a committed snapshot is malformed or was edited by hand.
//!
//! The decision is a pure function of the file contents, so both directions
//! are tested without a build. `examples/shape_record.rs` reads the files and
//! calls it.

use serde_json::Value;

use super::change_log::{coverage, diff, validate, ChangeLog};
use super::snapshot::{canonical_pretty, shape_hash};

/// The files the gate reads.
pub struct Inputs<'a> {
    /// The snapshot each release shipped, by name, in any order.
    pub releases: Vec<(String, String)>,
    /// The committed snapshot of the working tree.
    pub current: &'a str,
    /// What the build generates now.
    pub generated: &'a str,
    /// The committed change log.
    pub change_log: &'a str,
}

/// Orders release names oldest first: dotted numbers by value, anything else
/// (the starting `baseline`) before them.
fn release_order(name: &str) -> (bool, Vec<u64>) {
    let parts: Option<Vec<u64>> = name
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().ok())
        .collect();
    match parts {
        Some(parts) => (true, parts),
        None => (false, Vec::new()),
    }
}

/// Problems with one committed snapshot: not canonical text, or a hash that
/// does not match its content.
fn snapshot_problems(label: &str, text: &str) -> (Vec<String>, Option<Value>) {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(e) => return (vec![format!("{label} does not parse: {e}")], None),
    };
    let mut problems = Vec::new();
    if canonical_pretty(&value) != text {
        problems.push(format!(
            "{label} is not in the generated form; it was edited by hand"
        ));
    }
    let recomputed = shape_hash(&value["tables"], &value["schemas"]);
    if value["hash"].as_str() != Some(recomputed.as_str()) {
        problems.push(format!("{label}: its hash does not match its content"));
    }
    (problems, Some(value))
}

/// Every problem the gate finds; empty when it passes.
pub fn check(inputs: &Inputs) -> Vec<String> {
    let mut problems = Vec::new();

    if inputs.current != inputs.generated {
        problems.push(
            "shape-record/current.json is not what this build generates. Run \
             `bun run shape:gen` and commit it, then classify the difference in \
             shape-record/change-log.json"
                .to_string(),
        );
    }

    let log = match ChangeLog::parse(inputs.change_log) {
        Ok(log) => log,
        Err(e) => {
            problems.push(e);
            return problems;
        }
    };
    problems.extend(validate(&log));

    let mut releases = inputs.releases.clone();
    releases.sort_by_key(|(name, _)| release_order(name));
    let Some((baseline_name, baseline_text)) = releases.last() else {
        problems
            .push("shape-record/snapshots/ holds no release snapshot to measure from".to_string());
        return problems;
    };
    for (name, text) in &releases {
        problems.extend(snapshot_problems(&format!("snapshots/{name}.json"), text).0);
    }
    let (current_problems, current) = snapshot_problems("current.json", inputs.current);
    problems.extend(current_problems);
    let (Some(current), Ok(baseline)) = (current, serde_json::from_str::<Value>(baseline_text))
    else {
        return problems;
    };

    for entry in &log.entries {
        if !releases.iter().any(|(name, _)| *name == entry.after) {
            problems.push(format!(
                "entry '{}': measured from '{}', which is not a snapshot in shape-record/snapshots/",
                entry.id, entry.after
            ));
        }
    }

    let changed = diff(&baseline, &current);
    let entries: Vec<_> = log
        .entries
        .iter()
        .filter(|e| e.after == *baseline_name)
        .collect();
    let result = coverage(&changed, &entries);
    for path in result.uncovered {
        problems.push(format!(
            "{path} changed since snapshot '{baseline_name}' and no change-log entry covers it"
        ));
    }
    for (id, path) in result.stale {
        problems.push(format!(
            "entry '{id}' covers {path}, which did not change since snapshot '{baseline_name}'"
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_record::snapshot::{assemble, TableRow};
    use serde_json::json;

    fn snapshot(columns: &[&str], seed_version: &str) -> String {
        let tables: Vec<TableRow> = vec![(
            "node".to_string(),
            columns.iter().map(|c| c.to_string()).collect(),
            false,
        )];
        canonical_pretty(&assemble(
            &tables,
            &[],
            json!({ "s1": { "key": "k", "configVersion": seed_version, "guidanceVersion": "g" } }),
        ))
    }

    fn log(entries: Value) -> String {
        json!({ "entries": entries }).to_string()
    }

    fn entry(id: &str, area: &str, class: &str, covers: &[&str]) -> Value {
        let mut e = json!({
            "id": id, "after": "baseline", "covers": covers, "area": area,
            "class": class, "summary": "s", "userProducible": false
        });
        if area == "seeded_content" {
            e["seeded"] = json!({ "aspect": "config", "editedUsersAffected": true, "mustNotWaitForDecision": false });
        }
        if class == "migrate" {
            e["migrate"] = json!({ "affectedData": "rows", "transformation": "move" });
        }
        e
    }

    fn run(current: &str, generated: &str, change_log: &str) -> Vec<String> {
        check(&Inputs {
            releases: vec![("baseline".to_string(), snapshot(&["id"], "1"))],
            current,
            generated,
            change_log,
        })
    }

    #[test]
    fn an_unchanged_shape_passes_with_an_empty_log() {
        let same = snapshot(&["id"], "1");
        assert_eq!(run(&same, &same, &log(json!([]))), Vec::<String>::new());
    }

    /// A change with no entry fails.
    #[test]
    fn a_table_change_without_an_entry_fails() {
        let changed = snapshot(&["id", "extra"], "1");
        let problems = run(&changed, &changed, &log(json!([])));
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("tables.node changed")),
            "{problems:?}"
        );
    }

    /// The same change with an entry passes.
    #[test]
    fn the_same_change_with_an_entry_passes() {
        let changed = snapshot(&["id", "extra"], "1");
        let covered = log(json!([entry(
            "add-extra",
            "table_shape",
            "refuse",
            &["tables.node"]
        )]));
        assert_eq!(run(&changed, &changed, &covered), Vec::<String>::new());
    }

    #[test]
    fn a_seeded_change_needs_an_entry_too() {
        let changed = snapshot(&["id"], "2");
        let problems = run(&changed, &changed, &log(json!([])));
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("seeded.s1.configVersion changed")),
            "{problems:?}"
        );
        let covered = log(json!([entry(
            "seed",
            "seeded_content",
            "reconcile",
            &["seeded.s1.configVersion"]
        )]));
        assert_eq!(run(&changed, &changed, &covered), Vec::<String>::new());
    }

    #[test]
    fn an_entry_for_a_path_that_did_not_change_fails() {
        let same = snapshot(&["id"], "1");
        let stale = log(json!([entry(
            "stale",
            "table_shape",
            "refuse",
            &["tables.node"]
        )]));
        let problems = run(&same, &same, &stale);
        assert!(
            problems.iter().any(|p| p.contains("did not change")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_current_snapshot_the_build_does_not_generate_fails() {
        let committed = snapshot(&["id"], "1");
        let generated = snapshot(&["id", "extra"], "1");
        let problems = run(&committed, &generated, &log(json!([])));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("not what this build generates")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_hand_edited_snapshot_fails() {
        let tampered = snapshot(&["id"], "1").replace("\"id\"", "\"identity\"");
        let problems = run(&tampered, &tampered, &log(json!([])));
        assert!(
            problems.iter().any(|p| p.contains("hash does not match")),
            "{problems:?}"
        );
        let reflowed = snapshot(&["id"], "1").replace('\n', "\n ");
        let problems = run(&reflowed, &reflowed, &log(json!([])));
        assert!(
            problems.iter().any(|p| p.contains("edited by hand")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_malformed_log_fails() {
        let same = snapshot(&["id"], "1");
        assert!(!run(&same, &same, "not json").is_empty());
        let bad = log(json!([entry("x", "core_schema", "migrate", &[])]));
        assert!(!run(&same, &same, &bad).is_empty());
    }

    #[test]
    fn entries_measured_from_an_older_snapshot_are_history() {
        let changed = snapshot(&["id", "extra"], "1");
        let mut old = entry("old", "table_shape", "refuse", &["tables.node"]);
        old["after"] = json!("0.1.0");
        let inputs = Inputs {
            releases: vec![
                ("0.1.0".to_string(), snapshot(&["id"], "1")),
                ("0.2.0".to_string(), changed.clone()),
            ],
            current: &changed,
            generated: &changed,
            change_log: &log(json!([old])),
        };
        assert_eq!(check(&inputs), Vec::<String>::new());
    }

    #[test]
    fn the_latest_release_is_the_highest_version() {
        let early = snapshot(&["id"], "1");
        let late = snapshot(&["id", "extra"], "1");
        let inputs = Inputs {
            releases: vec![
                ("0.10.0".to_string(), late.clone()),
                ("0.9.0".to_string(), early),
                ("baseline".to_string(), snapshot(&["x"], "1")),
            ],
            current: &late,
            generated: &late,
            change_log: &log(json!([])),
        };
        assert_eq!(check(&inputs), Vec::<String>::new());
    }

    #[test]
    fn no_release_snapshot_fails() {
        let same = snapshot(&["id"], "1");
        let problems = check(&Inputs {
            releases: vec![],
            current: &same,
            generated: &same,
            change_log: &log(json!([])),
        });
        assert!(
            problems.iter().any(|p| p.contains("no release snapshot")),
            "{problems:?}"
        );
    }
}
