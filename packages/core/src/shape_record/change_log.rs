//! The change log: one hand-classified entry for each difference between two
//! consecutive snapshots (ADR-104).
//!
//! The snapshot says what differs; only a person can say what the difference
//! would require of a database that already exists. An entry records the
//! area, the class, whether a user could have produced the same state, and,
//! for seeded content and for `migrate`, the extra facts the format asks for.
//!
//! A change log entry lists the snapshot paths it covers. A path is the
//! dotted route into the snapshot document (`schemas.task.fields.priority`,
//! `tables.node`, `seeded.<id>.config`); [`diff`] reports the paths that
//! differ and an entry covers a path it names or contains.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Area {
    TableShape,
    CoreSchema,
    SeededContent,
    UserDataBucket,
    DerivedData,
    WireTooling,
    LocalAppState,
    Settings,
}

impl Area {
    /// Whether the snapshot lists this area, so an entry in it covers paths.
    fn is_in_snapshot(self) -> bool {
        matches!(
            self,
            Area::TableShape | Area::CoreSchema | Area::SeededContent
        )
    }
}

/// What a change requires of an existing database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Nothing is needed; the new shape is simply absent from an older database.
    Additive,
    /// The seed system covers it.
    Reconcile,
    /// The system regenerates it from what is stored.
    Rebuild,
    /// Existing rows must change. No mechanism exists to do it.
    Migrate,
    /// An older database cannot be opened.
    Refuse,
}

/// The part of a seeded node that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    Config,
    Guidance,
    ContextPaths,
    Links,
}

/// What a seeded-content entry records beyond the common fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeededDetail {
    pub aspect: Aspect,
    /// Whether users who kept an edit to this aspect are affected.
    pub edited_users_affected: bool,
    /// Whether the change must not wait for a user's keep-or-take decision.
    pub must_not_wait_for_decision: bool,
}

/// What a `migrate` entry records beyond the common fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrateDetail {
    /// What data is affected.
    pub affected_data: String,
    /// What the transformation would be.
    pub transformation: String,
}

/// One classified change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    /// A unique name for the entry.
    pub id: String,
    /// The snapshot the change is measured from, by its name in `snapshots/`.
    pub after: String,
    /// The snapshot paths the entry covers. Empty for an area the snapshot
    /// does not list.
    #[serde(default)]
    pub covers: Vec<String>,
    pub area: Area,
    pub class: Class,
    /// What changed, in a sentence.
    pub summary: String,
    /// Whether a user could have produced the same state themselves.
    pub user_producible: bool,
    /// Required for seeded content, and only there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeded: Option<SeededDetail>,
    /// Required for class `migrate`, and only there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrate: Option<MigrateDetail>,
}

/// The change log file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeLog {
    pub entries: Vec<Entry>,
}

impl ChangeLog {
    /// Reads a change log, refusing a file the format does not describe.
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("the change log does not parse: {e}"))
    }
}

/// The area a snapshot path belongs to, if it is one the log can cover.
pub fn area_of_path(path: &str) -> Option<Area> {
    let parts: Vec<&str> = path.split('.').collect();
    match parts.as_slice() {
        ["tables", ..] => Some(Area::TableShape),
        ["seeded", ..] => Some(Area::SeededContent),
        // A core schema's context paths are seeded content: the one part of
        // a core schema that ships with a value a user may change.
        ["schemas", _, "contextPaths", ..] => Some(Area::SeededContent),
        ["schemas" | "registry", ..] => Some(Area::CoreSchema),
        _ => None,
    }
}

/// The seeded aspect a snapshot path changes, if it is seeded content.
pub fn aspect_of_path(path: &str) -> Option<Aspect> {
    let parts: Vec<&str> = path.split('.').collect();
    match parts.as_slice() {
        ["schemas", _, "contextPaths", ..] => Some(Aspect::ContextPaths),
        ["seeded", _, "guidanceVersion"] => Some(Aspect::Guidance),
        ["seeded", _, "configVersion" | "key" | "nodeType" | "tier"] => Some(Aspect::Config),
        // A whole seed added or removed: the entry names its main aspect.
        ["seeded", _] => None,
        _ => None,
    }
}

/// Every problem with how the log is written, apart from what it covers.
pub fn validate(log: &ChangeLog) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in &log.entries {
        let id = &entry.id;
        let mut problem = |text: String| problems.push(format!("entry '{id}': {text}"));
        if id.trim().is_empty() {
            problem("has no id".to_string());
        } else if !seen.insert(id.clone()) {
            problem("the id is used twice".to_string());
        }
        if entry.after.trim().is_empty() {
            problem("does not name the snapshot it is measured from".to_string());
        }
        if entry.summary.trim().is_empty() {
            problem("has no summary".to_string());
        }
        match (&entry.seeded, entry.area == Area::SeededContent) {
            (None, true) => problem(
                "is seeded content and must record the aspect, the affected users and \
                 whether it may wait for a decision"
                    .to_string(),
            ),
            (Some(_), false) => {
                problem("records seeded details but is not seeded content".to_string())
            }
            _ => {}
        }
        match (&entry.migrate, entry.class == Class::Migrate) {
            (None, true) => problem(
                "is class migrate and must record the affected data and the transformation"
                    .to_string(),
            ),
            (Some(_), false) => {
                problem("records migrate details but is not class migrate".to_string())
            }
            (Some(detail), true)
                if detail.affected_data.trim().is_empty()
                    || detail.transformation.trim().is_empty() =>
            {
                problem("migrate details must not be empty".to_string());
            }
            _ => {}
        }
        if entry.area.is_in_snapshot() {
            if entry.covers.is_empty() {
                problem("covers no snapshot path".to_string());
            }
            for path in &entry.covers {
                if area_of_path(path) != Some(entry.area) {
                    problem(format!(
                        "covers '{path}', which is not in the entry's area; one entry covers one area"
                    ));
                } else if let (Some(seeded), Some(aspect)) = (&entry.seeded, aspect_of_path(path)) {
                    if seeded.aspect != aspect {
                        problem(format!(
                            "covers '{path}', whose aspect is not the recorded one"
                        ));
                    }
                }
            }
        } else if !entry.covers.is_empty() {
            problem(
                "is in an area the snapshot does not list and must not cover paths".to_string(),
            );
        }
    }
    problems
}

/// How deep a difference is reported: down to the unit a person classifies.
fn reports_at(prefix: &[String]) -> usize {
    match prefix.first().map(String::as_str) {
        Some("tables") => 2,
        Some("schemas") => {
            if matches!(
                prefix.get(2).map(String::as_str),
                Some("fields" | "relationships")
            ) {
                4
            } else {
                3
            }
        }
        Some("seeded" | "registry") => 3,
        Some(_) => 1,
        None => usize::MAX,
    }
}

/// Keys of the snapshot document that follow from the rest and are not
/// compared.
const NOT_COMPARED: [&str; 2] = ["hash", "formatVersion"];

/// The snapshot paths at which `new` differs from `old`: a table, a field, a
/// relationship, a schema's other declarations, a seed's aspect, or a whole
/// table, type or seed that is added or removed.
pub fn diff(old: &Value, new: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    walk(&mut Vec::new(), Some(old), Some(new), &mut paths);
    paths.sort();
    paths
}

fn walk(prefix: &mut Vec<String>, old: Option<&Value>, new: Option<&Value>, out: &mut Vec<String>) {
    if old == new {
        return;
    }
    if let (Some(Value::Object(a)), Some(Value::Object(b))) = (old, new) {
        if prefix.len() < reports_at(prefix) {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for key in keys {
                if prefix.is_empty() && NOT_COMPARED.contains(&key.as_str()) {
                    continue;
                }
                prefix.push(key.clone());
                walk(prefix, a.get(key), b.get(key), out);
                prefix.pop();
            }
            return;
        }
    }
    out.push(prefix.join("."));
}

/// The depth at which a difference under `path` is reported, whatever the
/// rest of the path is.
fn required_depth(path: &str) -> usize {
    let parts: Vec<String> = path.split('.').take(3).map(str::to_string).collect();
    reports_at(&parts)
}

/// Whether an entry that names `cover` accounts for a difference at `path`.
///
/// A cover names a difference when it is that path, or lies inside it. A cover
/// that contains the path accounts for it only when the cover is as deep as
/// the unit differences are reported at: `schemas` or `schemas.task` would
/// otherwise clear any change under it without a person looking at it.
fn names(cover: &str, path: &str) -> bool {
    cover == path
        || cover.starts_with(&format!("{path}."))
        || (path.starts_with(&format!("{cover}."))
            && cover.split('.').count() >= required_depth(cover))
}

/// The differences no entry accounts for, and the entries naming a path that
/// has not changed.
pub struct Coverage {
    pub uncovered: Vec<String>,
    pub stale: Vec<(String, String)>,
}

/// Checks `entries` (those measured from the baseline) against the paths that
/// differ.
pub fn coverage(changed: &[String], entries: &[&Entry]) -> Coverage {
    let uncovered = changed
        .iter()
        .filter(|path| {
            !entries
                .iter()
                .any(|entry| entry.covers.iter().any(|cover| names(cover, path)))
        })
        .cloned()
        .collect();
    let stale = entries
        .iter()
        .flat_map(|entry| {
            entry
                .covers
                .iter()
                .map(move |cover| (entry.id.clone(), cover.clone()))
        })
        .filter(|(_, cover)| !changed.iter().any(|path| names(cover, path)))
        .collect();
    Coverage { uncovered, stale }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(area: Area, class: Class, covers: &[&str]) -> Entry {
        Entry {
            id: "e1".to_string(),
            after: "baseline".to_string(),
            covers: covers.iter().map(|c| c.to_string()).collect(),
            area,
            class,
            summary: "a change".to_string(),
            user_producible: false,
            seeded: (area == Area::SeededContent).then_some(SeededDetail {
                aspect: Aspect::Config,
                edited_users_affected: true,
                must_not_wait_for_decision: false,
            }),
            migrate: (class == Class::Migrate).then(|| MigrateDetail {
                affected_data: "rows".to_string(),
                transformation: "move them".to_string(),
            }),
        }
    }

    fn problems_of(entries: Vec<Entry>) -> Vec<String> {
        validate(&ChangeLog { entries })
    }

    #[test]
    fn a_complete_entry_is_valid() {
        assert!(problems_of(vec![
            entry(
                Area::CoreSchema,
                Class::Refuse,
                &["schemas.task.fields.status"]
            ),
            Entry {
                id: "e2".to_string(),
                ..entry(Area::TableShape, Class::Migrate, &["tables.node"])
            },
            Entry {
                id: "e3".to_string(),
                ..entry(
                    Area::SeededContent,
                    Class::Reconcile,
                    &["seeded.x.configVersion"]
                )
            },
            Entry {
                id: "e4".to_string(),
                ..entry(Area::WireTooling, Class::Additive, &[])
            },
        ])
        .is_empty());
    }

    #[test]
    fn each_rule_of_the_format_is_enforced() {
        let cases: Vec<(&str, Entry)> = vec![
            (
                "no id",
                Entry {
                    id: " ".to_string(),
                    ..entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"])
                },
            ),
            (
                "not name the snapshot",
                Entry {
                    after: String::new(),
                    ..entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"])
                },
            ),
            (
                "no summary",
                Entry {
                    summary: String::new(),
                    ..entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"])
                },
            ),
            (
                "covers no snapshot path",
                entry(Area::CoreSchema, Class::Additive, &[]),
            ),
            (
                "not in the entry's area",
                entry(Area::CoreSchema, Class::Additive, &["tables.node"]),
            ),
            (
                "must not cover paths",
                entry(Area::Settings, Class::Additive, &["tables.node"]),
            ),
            (
                "must record the aspect",
                Entry {
                    seeded: None,
                    ..entry(
                        Area::SeededContent,
                        Class::Reconcile,
                        &["seeded.x.configVersion"],
                    )
                },
            ),
            (
                "not seeded content",
                Entry {
                    seeded: Some(SeededDetail {
                        aspect: Aspect::Config,
                        edited_users_affected: false,
                        must_not_wait_for_decision: false,
                    }),
                    ..entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"])
                },
            ),
            (
                "must record the affected data",
                Entry {
                    migrate: None,
                    ..entry(Area::CoreSchema, Class::Migrate, &["schemas.a.fields.b"])
                },
            ),
            (
                "not class migrate",
                Entry {
                    migrate: Some(MigrateDetail {
                        affected_data: "x".to_string(),
                        transformation: "y".to_string(),
                    }),
                    ..entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"])
                },
            ),
            (
                "must not be empty",
                Entry {
                    migrate: Some(MigrateDetail {
                        affected_data: String::new(),
                        transformation: "y".to_string(),
                    }),
                    ..entry(Area::CoreSchema, Class::Migrate, &["schemas.a.fields.b"])
                },
            ),
            (
                "aspect is not the recorded one",
                entry(
                    Area::SeededContent,
                    Class::Reconcile,
                    &["seeded.x.guidanceVersion"],
                ),
            ),
        ];
        for (expected, bad) in cases {
            let problems = problems_of(vec![bad]);
            assert!(
                problems.iter().any(|p| p.contains(expected)),
                "expected a problem mentioning '{expected}', got {problems:?}"
            );
        }
        let twice = problems_of(vec![
            entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"]),
            entry(Area::CoreSchema, Class::Additive, &["schemas.a.fields.b"]),
        ]);
        assert!(twice.iter().any(|p| p.contains("used twice")), "{twice:?}");
    }

    #[test]
    fn context_paths_are_seeded_content() {
        assert_eq!(
            area_of_path("schemas.task.contextPaths"),
            Some(Area::SeededContent)
        );
        assert_eq!(
            aspect_of_path("schemas.task.contextPaths"),
            Some(Aspect::ContextPaths)
        );
        assert_eq!(
            area_of_path("schemas.task.fields.status"),
            Some(Area::CoreSchema)
        );
        assert_eq!(area_of_path("tables.node"), Some(Area::TableShape));
        assert_eq!(area_of_path("hash"), None);
    }

    #[test]
    fn a_log_with_an_unknown_class_or_key_does_not_parse() {
        assert!(ChangeLog::parse(r#"{"entries":[]}"#).is_ok());
        let bad_class = r#"{"entries":[{"id":"a","after":"b","area":"core_schema","class":"nope","summary":"s","userProducible":false}]}"#;
        assert!(ChangeLog::parse(bad_class).is_err());
        let extra = r#"{"entries":[],"other":1}"#;
        assert!(ChangeLog::parse(extra).is_err());
    }

    #[test]
    fn diff_reports_the_unit_a_person_classifies() {
        let old = json!({
            "hash": "a", "formatVersion": 1,
            "tables": { "node": { "columns": ["id"], "virtual": false }, "gone": { "columns": [], "virtual": false } },
            "schemas": { "task": {
                "name": "Task", "extends": "x", "contextPaths": [],
                "fields": { "status": { "type": "enum", "coreValues": ["a"] }, "same": { "type": "text" } },
                "relationships": {}
            } },
            "seeded": { "s1": { "key": "k", "configVersion": "1", "guidanceVersion": "1" } }
        });
        let new = json!({
            "hash": "b", "formatVersion": 2,
            "tables": { "node": { "columns": ["id", "extra"], "virtual": false }, "fresh": { "columns": [], "virtual": false } },
            "schemas": { "task": {
                "name": "Task", "extends": "y", "contextPaths": [1],
                "fields": { "status": { "type": "enum", "coreValues": ["a", "b"] }, "same": { "type": "text" }, "added": { "type": "text" } },
                "relationships": {}
            }, "newtype": { "name": "N" } },
            "seeded": { "s1": { "key": "k", "configVersion": "2", "guidanceVersion": "1" } }
        });
        assert_eq!(
            diff(&old, &new),
            [
                "schemas.newtype",
                "schemas.task.contextPaths",
                "schemas.task.extends",
                "schemas.task.fields.added",
                "schemas.task.fields.status",
                "seeded.s1.configVersion",
                "tables.fresh",
                "tables.gone",
                "tables.node",
            ]
        );
        assert!(diff(&old, &old).is_empty());
    }

    #[test]
    fn coverage_finds_uncovered_differences_and_stale_entries() {
        let changed = vec![
            "schemas.task.fields.status".to_string(),
            "tables.node".to_string(),
        ];
        let covering = entry(
            Area::CoreSchema,
            Class::Refuse,
            &["schemas.task.fields.status"],
        );
        let result = coverage(&changed, &[&covering]);
        assert_eq!(result.uncovered, ["tables.node"]);
        assert!(result.stale.is_empty());

        let stale = entry(Area::TableShape, Class::Migrate, &["tables.other"]);
        let result = coverage(&changed, &[&covering, &stale]);
        assert_eq!(
            result.stale,
            [("e1".to_string(), "tables.other".to_string())]
        );
    }

    /// A cover shallower than the unit a difference is reported at names too
    /// much: it would let any change under it through without an entry.
    #[test]
    fn a_shallow_cover_does_not_cover_a_deeper_change() {
        let changed = vec![
            "schemas.task.contextPaths".to_string(),
            "tables.node".to_string(),
            "seeded.s1.configVersion".to_string(),
            "schemas.task.fields.status".to_string(),
        ];
        for shallow in [
            "schemas",
            "tables",
            "seeded",
            "schemas.task",
            "schemas.task.fields",
            "seeded.s1",
        ] {
            let e = entry(Area::CoreSchema, Class::Refuse, &[shallow]);
            let result = coverage(&changed, &[&e]);
            assert_eq!(
                result.uncovered.len(),
                changed.len(),
                "cover '{shallow}' let a change through"
            );
        }
    }

    /// A whole table, type or seed that appears is reported at its own
    /// path, and an entry names it exactly.
    #[test]
    fn an_added_whole_object_is_covered_by_its_exact_path() {
        let changed = vec![
            "schemas.newtype".to_string(),
            "registry.newtype".to_string(),
        ];
        let e = entry(
            Area::CoreSchema,
            Class::Additive,
            &["schemas.newtype", "registry.newtype"],
        );
        assert!(coverage(&changed, &[&e]).uncovered.is_empty());
    }

    #[test]
    fn registry_changes_are_core_schema_changes() {
        assert_eq!(
            area_of_path("registry.text.participation"),
            Some(Area::CoreSchema)
        );
    }
}
