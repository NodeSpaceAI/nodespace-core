//! Whether the core types a database holds are the ones this build ships.
//!
//! The second half of the shape check in [`super::schema`]. A core type's
//! schema is a seeded node: it is written when a database is created and
//! never rewritten, so a database an earlier build created keeps that
//! build's core types. Opening it would leave `task` without a field this
//! build writes, or leave a type this build ships as a core type held by a
//! schema a user or an installer created under the same id. Neither can be
//! carried forward: NodeSpace does not migrate databases.
//!
//! So a database that already holds schema nodes must hold, for every core
//! type it has, a core schema whose fields are the ones this build declares.
//! That is read from the schema nodes themselves, never from a stored
//! version: nothing on disk records one.
//!
//! Only a difference another build can have produced counts. What a user may
//! do to a core schema through the schema API is left out of the comparison:
//!
//! - add a namespaced field (`custom:estimate`);
//! - add values to an extensible enum;
//! - remove or rename a field core declares as theirs to change (one with
//!   `user` protection, such as a task's `priority`);
//! - add or remove a context path. The `_seed` bookkeeping kept beside the
//!   paths is not part of the definition either.
//!
//! A user can never add a field with a bare name to a core schema, remove a
//! field core protects, or change a field's type or the values core gives an
//! enum, so each of those is a difference. A core type the database does not
//! hold yet is not one; seeding creates it.
//!
//! Fields are what is compared. A core type's relationship declarations are
//! edges, not part of its node, and are not read here: a build that differed
//! from this one only in a declared relationship would pass.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::models::core_schemas::get_core_schemas;
use crate::models::{CoreNodeType, SchemaField, SchemaProtectionLevel};

/// How one core type in a database differs from the one this build ships.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTypeMismatch {
    /// The core type's id (`task`, `spec`).
    pub type_id: String,
    pub difference: CoreTypeDifference,
}

/// The way a stored core type differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreTypeDifference {
    /// The id is held by a schema that is not a core schema: a type a user
    /// or an installer created before this build shipped a core type of
    /// that name.
    NotCore,
    /// The id is held by a node that is not a schema at all.
    NotASchema,
    /// The schema's stored definition is not one this build can read: it
    /// uses a field type or a key another build knows and this one does not.
    Unreadable(String),
    /// The core schema's fields are not the ones this build declares.
    Fields {
        /// Fields this build declares and protects that the stored schema
        /// lacks.
        missing: Vec<String>,
        /// Fields the stored schema declares that this build does not.
        unexpected: Vec<String>,
        /// Fields both declare, with a different type or different core
        /// values.
        changed: Vec<String>,
    },
}

impl std::fmt::Display for CoreTypeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.difference {
            CoreTypeDifference::NotCore => write!(
                f,
                "type '{}' is not the core type this version ships",
                self.type_id
            ),
            CoreTypeDifference::NotASchema => {
                write!(f, "'{}' is not a schema", self.type_id)
            }
            CoreTypeDifference::Unreadable(reason) => write!(
                f,
                "type '{}' has a definition this version cannot read ({reason})",
                self.type_id
            ),
            CoreTypeDifference::Fields {
                missing,
                unexpected,
                changed,
            } => {
                let mut parts = Vec::new();
                for (label, names) in [
                    ("missing", missing),
                    ("unexpected", unexpected),
                    ("changed", changed),
                ] {
                    if !names.is_empty() {
                        parts.push(format!("{label} {}", names.join(", ")));
                    }
                }
                write!(f, "type '{}': {}", self.type_id, parts.join("; "))
            }
        }
    }
}

/// What of a field the comparison reads: its type, its item type, and the
/// values core gives an enum.
#[derive(Debug, PartialEq, Eq)]
struct FieldShape {
    field_type: String,
    item_type: Option<String>,
    core_values: Vec<String>,
}

impl FieldShape {
    fn of(field: &SchemaField) -> Self {
        Self {
            field_type: field.field_type.as_str().to_string(),
            item_type: field.item_type.map(|t| t.as_str().to_string()),
            core_values: field
                .core_values
                .iter()
                .flatten()
                .map(|v| v.value.clone())
                .collect(),
        }
    }
}

/// The fields core declares on a schema, by name. A namespaced field
/// (`custom:estimate`) is one somebody added to the type, not core's.
fn core_fields(fields: &[SchemaField]) -> BTreeMap<&str, FieldShape> {
    fields
        .iter()
        .filter(|field| !field.name.contains(':'))
        .map(|field| (field.name.as_str(), FieldShape::of(field)))
        .collect()
}

fn compare_fields(
    type_id: &str,
    shipped: &[SchemaField],
    stored: &[SchemaField],
) -> Option<CoreTypeMismatch> {
    // A field a user may remove or rename is not missing when it is gone.
    let missing: Vec<String> = shipped
        .iter()
        .filter(|field| field.protection != SchemaProtectionLevel::User)
        .filter(|field| !stored.iter().any(|other| other.name == field.name))
        .map(|field| field.name.clone())
        .collect();

    let shipped = core_fields(shipped);
    let stored = core_fields(stored);
    let unexpected: Vec<String> = stored
        .keys()
        .filter(|name| !shipped.contains_key(*name))
        .map(|name| name.to_string())
        .collect();
    let changed: Vec<String> = shipped
        .iter()
        .filter(|(name, shape)| stored.get(*name).is_some_and(|other| other != *shape))
        .map(|(name, _)| name.to_string())
        .collect();
    if missing.is_empty() && unexpected.is_empty() && changed.is_empty() {
        return None;
    }
    Some(CoreTypeMismatch {
        type_id: type_id.to_string(),
        difference: CoreTypeDifference::Fields {
            missing,
            unexpected,
            changed,
        },
    })
}

/// Every core type the database on `conn` holds that differs from the one
/// this build ships, in the order this build declares them. Reads only.
///
/// The caller has already checked that the database has this build's tables,
/// so `node` exists and has the columns read here.
pub(super) async fn find_core_type_mismatches(
    conn: &libsql::Connection,
) -> Result<Vec<CoreTypeMismatch>> {
    let mut mismatches = Vec::new();
    for shipped in get_core_schemas() {
        let type_id = shipped.envelope.id.as_str();
        let mut rows = conn
            .query(
                "SELECT node_type, properties FROM node WHERE id = ?1",
                libsql::params![type_id],
            )
            .await
            .with_context(|| format!("Failed to read the node for core type '{type_id}'"))?;
        let Some(row) = rows
            .next()
            .await
            .with_context(|| format!("Failed to read the node for core type '{type_id}'"))?
        else {
            continue;
        };
        // Copied out before the cursor moves on: a libsql row reads through
        // its cursor.
        let node_type: String = row.get(0).context("Failed to read a node's type")?;
        let properties: String = row.get(1).context("Failed to read a node's properties")?;
        drop(rows);

        let difference = |difference| CoreTypeMismatch {
            type_id: type_id.to_string(),
            difference,
        };
        if !CoreNodeType::Schema.is_exactly(&node_type) {
            mismatches.push(difference(CoreTypeDifference::NotASchema));
            continue;
        }
        // A definition this build cannot read is another build's, and is
        // refused like any other difference: failing the open with a plain
        // error would leave no marker and have the daemon restarted into it.
        let properties: serde_json::Value = match serde_json::from_str(&properties) {
            Ok(properties) => properties,
            Err(e) => {
                mismatches.push(difference(CoreTypeDifference::Unreadable(e.to_string())));
                continue;
            }
        };
        if properties.get("isCore").and_then(|v| v.as_bool()) != Some(true) {
            mismatches.push(difference(CoreTypeDifference::NotCore));
            continue;
        }
        let stored: Vec<SchemaField> = match properties.get("fields") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(fields) => match serde_json::from_value(fields.clone()) {
                Ok(stored) => stored,
                Err(e) => {
                    mismatches.push(difference(CoreTypeDifference::Unreadable(e.to_string())));
                    continue;
                }
            },
        };
        mismatches.extend(compare_fields(type_id, &shipped.fields, &stored));
    }
    Ok(mismatches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::EnumValue;
    use crate::models::schema_node::to_node;
    use crate::models::{SchemaFieldType, SchemaNode};

    async fn database() -> (libsql::Connection, tempfile::TempDir) {
        crate::db::ensure_sqlite_vec_registered().await;
        let dir = tempfile::TempDir::new().unwrap();
        let db = libsql::Builder::new_local(dir.path().join("test.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        crate::db::schema::create_schema(&conn).await.unwrap();
        (conn, dir)
    }

    async fn insert(conn: &libsql::Connection, schema: &SchemaNode) {
        let node = to_node(schema);
        conn.execute(
            "INSERT OR REPLACE INTO node (id, node_type, content, properties, created_at, modified_at) \
             VALUES (?1, ?2, ?3, ?4, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            libsql::params![
                node.id,
                node.node_type,
                node.content,
                node.properties.to_string()
            ],
        )
        .await
        .unwrap();
    }

    fn shipped(id: &str) -> SchemaNode {
        get_core_schemas()
            .into_iter()
            .find(|s| s.envelope.id == id)
            .unwrap_or_else(|| panic!("{id} is a core schema"))
    }

    async fn seed_all(conn: &libsql::Connection) {
        for schema in get_core_schemas() {
            insert(conn, &schema).await;
        }
    }

    /// A database with no schema nodes yet, and one holding every core schema
    /// as this build ships it, both match.
    #[tokio::test]
    async fn this_builds_own_core_types_match() {
        let (conn, _dir) = database().await;
        assert!(find_core_type_mismatches(&conn).await.unwrap().is_empty());
        seed_all(&conn).await;
        assert!(find_core_type_mismatches(&conn).await.unwrap().is_empty());
    }

    /// A core type the database does not hold is not a difference: seeding
    /// creates it.
    #[tokio::test]
    async fn a_core_type_the_database_lacks_is_not_a_difference() {
        let (conn, _dir) = database().await;
        seed_all(&conn).await;
        conn.execute("DELETE FROM node WHERE id = 'decision'", ())
            .await
            .unwrap();
        assert!(find_core_type_mismatches(&conn).await.unwrap().is_empty());
    }

    /// A type a user or an installer created under an id this build ships as
    /// a core type is not that core type.
    #[tokio::test]
    async fn a_core_id_held_by_a_non_core_schema_is_a_mismatch() {
        let (conn, _dir) = database().await;
        seed_all(&conn).await;
        let mut spec = shipped("spec");
        spec.is_core = false;
        insert(&conn, &spec).await;

        assert_eq!(
            find_core_type_mismatches(&conn).await.unwrap(),
            [CoreTypeMismatch {
                type_id: "spec".to_string(),
                difference: CoreTypeDifference::NotCore,
            }]
        );
    }

    /// A core schema as an earlier build seeded it differs in ways no user
    /// could have produced: a core enum without a value this build has, a
    /// bare-named field this build does not declare, a protected field gone,
    /// a field of another type.
    #[tokio::test]
    async fn a_core_schema_another_build_seeded_is_a_mismatch() {
        let (conn, _dir) = database().await;
        seed_all(&conn).await;

        // `task` as an earlier build seeded it: no `in_review`, and a
        // bare-named field this build does not declare.
        let mut task = shipped("task");
        let status = task.fields.iter_mut().find(|f| f.name == "status").unwrap();
        status
            .core_values
            .as_mut()
            .unwrap()
            .retain(|v| v.value != "in_review");
        let mut retired = task.fields[1].clone();
        retired.name = "estimate".to_string();
        task.fields.push(retired);
        insert(&conn, &task).await;

        let mismatches = find_core_type_mismatches(&conn).await.unwrap();
        assert_eq!(
            mismatches,
            [CoreTypeMismatch {
                type_id: "task".to_string(),
                difference: CoreTypeDifference::Fields {
                    missing: vec![],
                    unexpected: vec!["estimate".to_string()],
                    changed: vec!["status".to_string()],
                },
            }]
        );
        assert_eq!(
            mismatches[0].to_string(),
            "type 'task': unexpected estimate; changed status"
        );
        insert(&conn, &shipped("task")).await;

        // A field core protects cannot be removed through the schema API, so
        // one that is gone was never seeded.
        let mut spec = shipped("spec");
        spec.fields.retain(|f| f.name != "spec_status");
        insert(&conn, &spec).await;
        assert_eq!(
            find_core_type_mismatches(&conn).await.unwrap(),
            [CoreTypeMismatch {
                type_id: "spec".to_string(),
                difference: CoreTypeDifference::Fields {
                    missing: vec!["spec_status".to_string()],
                    unexpected: vec![],
                    changed: vec![],
                },
            }]
        );
        insert(&conn, &shipped("spec")).await;

        // A field whose type changed is a change too.
        let mut project = shipped("project");
        let repository = project
            .fields
            .iter_mut()
            .find(|f| f.name == "repository")
            .unwrap();
        repository.field_type = SchemaFieldType::Text;
        insert(&conn, &project).await;
        assert_eq!(
            find_core_type_mismatches(&conn).await.unwrap(),
            [CoreTypeMismatch {
                type_id: "project".to_string(),
                difference: CoreTypeDifference::Fields {
                    missing: vec![],
                    unexpected: vec![],
                    changed: vec!["repository".to_string()],
                },
            }]
        );
    }

    /// A definition this build cannot read is refused as a difference, not
    /// raised as an error: here a field of a type only another build knows.
    #[tokio::test]
    async fn a_core_schema_this_build_cannot_read_is_a_mismatch() {
        let (conn, _dir) = database().await;
        seed_all(&conn).await;

        conn.execute(
            "UPDATE node SET properties = json_set(properties, '$.fields', json(
                '[{\"name\":\"status\",\"friendlyName\":\"Status\",\"type\":\"duration\",\"protection\":\"core\",\"indexed\":true}]'))
             WHERE id = 'task'",
            (),
        )
        .await
        .unwrap();
        let mismatches = find_core_type_mismatches(&conn).await.unwrap();
        assert_eq!(mismatches.len(), 1, "{mismatches:?}");
        assert_eq!(mismatches[0].type_id, "task");
        assert!(
            matches!(&mismatches[0].difference, CoreTypeDifference::Unreadable(reason) if reason.contains("duration")),
            "{mismatches:?}"
        );
        assert!(mismatches[0]
            .to_string()
            .starts_with("type 'task' has a definition this version cannot read"));
    }

    /// What a user may do to a core schema is not a difference: a namespaced
    /// field they added, a value they added to an extensible enum, and a
    /// field of theirs to change that they removed.
    #[tokio::test]
    async fn a_users_changes_to_a_core_schema_are_not_a_mismatch() {
        let (conn, _dir) = database().await;
        seed_all(&conn).await;

        let mut task = shipped("task");
        let mut estimate = task.fields[2].clone();
        estimate.name = "custom:estimate".to_string();
        task.fields.push(estimate);
        let status = task.fields.iter_mut().find(|f| f.name == "status").unwrap();
        status.user_values = Some(vec![EnumValue::new(
            "backlog".to_string(),
            "Backlog".to_string(),
        )]);
        // `priority` and the links are the user's to remove.
        assert_eq!(
            shipped("task").get_field("priority").unwrap().protection,
            SchemaProtectionLevel::User
        );
        task.fields
            .retain(|f| !["priority", "pull_request", "commits"].contains(&f.name.as_str()));
        insert(&conn, &task).await;

        assert!(find_core_type_mismatches(&conn).await.unwrap().is_empty());
    }

    /// A core schema's context paths are the user's to change, and the
    /// bookkeeping beside them is not part of its definition: neither is a
    /// difference, whether the paths were changed or removed altogether.
    #[tokio::test]
    async fn a_users_context_paths_and_their_bookkeeping_are_not_a_mismatch() {
        let (conn, _dir) = database().await;
        seed_all(&conn).await;
        assert!(!shipped("task").context_paths.is_empty());

        for paths in [vec!["project", "blocked_by"], vec![]] {
            let mut task = shipped("task");
            task.context_paths = paths.iter().map(|path| path.parse().unwrap()).collect();
            insert(&conn, &task).await;
            conn.execute(
                "UPDATE node SET properties = json_set(properties, '$._seed', json(?1)) \
                 WHERE id = 'task'",
                libsql::params![
                    r#"{"context_paths_version":"an-earlier-one","context_paths_modified":true}"#
                ],
            )
            .await
            .unwrap();
            assert!(find_core_type_mismatches(&conn).await.unwrap().is_empty());
        }
    }
}
