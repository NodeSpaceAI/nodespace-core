//! Resolving a [`RelationshipPath`] against the schemas (ADR-086 §11).
//!
//! A path names relationships the way an author writes them. Resolving it
//! turns each name into the stored edges it means: a concrete
//! `relationship_type`, a direction, and the declared type at the far end.
//! That is what [`crate::db::SqliteStore::resolve_relationship_path`] and the
//! query service compile to SQL.
//!
//! Resolution needs only schemas, never a node, so a query filter and a play
//! are resolved the same way when they are saved and when they run.

use crate::models::schema::{
    builtin_forward_name, RelationshipCardinality, RelationshipDirection,
    BUILTIN_RELATIONSHIP_NAMES,
};
use crate::ops::OpsError;
use crate::services::{NodeService, NodeServiceError};
use nodespace_types::{HopDirection, RelationshipHop, RelationshipPath, ResolvedHop, ResolvedPath};

/// What one hop's name turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HopResolution {
    Resolved(ResolvedHop),
    /// The type declares no relationship by this name, in either direction.
    Undeclared,
    /// The name is not a built-in, and the type it would be resolved against
    /// is not known here: the walk reached this point through a built-in or
    /// untyped relationship, which any type may sit at the end of.
    TypeUnknown,
}

/// Resolve one hop against `node_type`.
///
/// Names resolve in this order, the same order the CLI's relationship reads
/// use ([`crate::ops::rel_ops::resolve_relationship_name`]):
///
/// 1. a built-in forward name (`has_child`);
/// 2. a built-in reverse name (`child_of`);
/// 3. a forward name the type declares or inherits through `extends`;
/// 4. a `reverseName` declared by a schema whose relationship targets the
///    type or one of its ancestors;
/// 5. the forward name of such a relationship, walked from its target end.
///
/// A forward name always wins over a same-spelled reverse name declared
/// elsewhere. Built-ins resolve without a type; every other name needs one.
pub async fn resolve_hop(
    node_service: &NodeService,
    node_type: Option<&str>,
    hop: &RelationshipHop,
) -> Result<HopResolution, NodeServiceError> {
    let name = hop.name.as_str();
    let resolved = |relationship_type: &str, direction| ResolvedHop {
        name: name.to_string(),
        relationship_type: relationship_type.to_string(),
        direction,
        source_type: None,
        far_type: None,
        declared_many: false,
        untyped: false,
        open_ended: hop.open_ended,
    };

    // Built-ins have no declaration to consult, and a schema may not claim
    // one of these names in either direction, so nothing is shadowed.
    if BUILTIN_RELATIONSHIP_NAMES.contains(&name) {
        return Ok(HopResolution::Resolved(resolved(
            name,
            HopDirection::Outbound,
        )));
    }
    if let Some(forward_name) = builtin_forward_name(name) {
        return Ok(HopResolution::Resolved(resolved(
            forward_name,
            HopDirection::Inbound,
        )));
    }

    let Some(node_type) = node_type else {
        return Ok(HopResolution::TypeUnknown);
    };

    // Own and inherited declarations (`resolve_relationships` merges the
    // `extends` chain), so a name declared on `task` resolves on an `issue`.
    let (own, _owners) = node_service.resolve_relationships(node_type).await?;
    if let Some(rel) = own.iter().find(|r| r.name == name) {
        let declared_many = rel.cardinality == RelationshipCardinality::Many;
        // An `in` declaration is this type's own name for a forward edge
        // stored under its `reverse_name`: it walks as that edge's reverse.
        if rel.direction == RelationshipDirection::In {
            return Ok(HopResolution::Resolved(ResolvedHop {
                source_type: rel.target_type.clone(),
                far_type: rel.target_type.clone(),
                declared_many,
                untyped: rel.target_type.is_none(),
                ..resolved(&rel.reverse_name, HopDirection::Inbound)
            }));
        }
        return Ok(HopResolution::Resolved(ResolvedHop {
            far_type: rel.target_type.clone(),
            declared_many,
            ..resolved(name, HopDirection::Outbound)
        }));
    }

    // Declarations by other schemas that target this type (or an ancestor of
    // it). Read from this end, the reverse cardinality governs: it says how
    // many sources may point at one target, whichever name spelled the walk.
    let inbound = node_service.get_inbound_relationships(node_type).await?;
    let from_target_end = |source_type: &str,
                           rel: &crate::models::schema::SchemaRelationship,
                           narrowed: bool| ResolvedHop {
        source_type: narrowed.then(|| source_type.to_string()),
        far_type: Some(source_type.to_string()),
        declared_many: rel.reverse_cardinality == RelationshipCardinality::Many,
        untyped: rel.target_type.is_none(),
        ..resolved(&rel.name, HopDirection::Inbound)
    };

    // A declaration that names this type as its target is about this type.
    // One with no target type is reported as inbound for every type, since
    // it may point at anything, so it is consulted only when no typed
    // declaration claims the name: two schemas spelling a reverse name the
    // same way must not let the untyped one answer for a type the other
    // names.
    let declared = |matches: &dyn Fn(&crate::models::schema::SchemaRelationship) -> bool| {
        inbound
            .iter()
            .find(|(_, rel)| rel.target_type.is_some() && matches(rel))
            .or_else(|| inbound.iter().find(|(_, rel)| matches(rel)))
    };

    // A reverse name belongs to exactly one declarer, so its edges are
    // narrowed to that declarer's type.
    if let Some((source_type, rel)) = declared(&|rel| rel.reverse_name == name) {
        return Ok(HopResolution::Resolved(from_target_end(
            source_type,
            rel,
            true,
        )));
    }
    // The forward name read from the target end asks for it generically, so
    // every schema declaring it is a legitimate answer: not narrowed.
    if let Some((source_type, rel)) = declared(&|rel| rel.name == name) {
        return Ok(HopResolution::Resolved(from_target_end(
            source_type,
            rel,
            false,
        )));
    }

    Ok(HopResolution::Undeclared)
}

/// Why a path could not be resolved against the schemas.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathResolveError {
    /// A hop names a relationship its type does not declare.
    #[error("{message}")]
    Undeclared {
        /// Index of the hop in the path.
        hop: usize,
        name: String,
        node_type: String,
        message: String,
    },
    /// A hop cannot be resolved because the walk reached it through a
    /// relationship with no declared target type.
    #[error(
        "'{name}' cannot be resolved after '{previous}': '{previous}' has no declared target \
         type, so the type '{name}' would be read from is not known. Only built-in relationships \
         ({builtins}, and their reverse names) can follow it."
    )]
    TypeUnknown {
        hop: usize,
        name: String,
        previous: String,
        builtins: String,
    },
    /// The path starts from every type (`*`), which declares nothing.
    #[error(
        "'{name}' cannot be resolved for every type ('*'): a schema-declared relationship is \
         resolved against one type. Name a type, or use a built-in relationship ({builtins}, \
         and their reverse names)."
    )]
    NoStartType { name: String, builtins: String },
    /// A schema lookup failed. Not a statement about the path.
    #[error("failed to resolve '{name}': {error}")]
    Lookup { name: String, error: String },
}

impl From<PathResolveError> for OpsError {
    fn from(error: PathResolveError) -> Self {
        match error {
            PathResolveError::Lookup { .. } => OpsError::Internal(error.to_string()),
            _ => OpsError::InvalidParams(error.to_string()),
        }
    }
}

fn builtin_names() -> String {
    BUILTIN_RELATIONSHIP_NAMES.join(", ")
}

/// Resolve every hop of `path`, starting from `start_type`.
///
/// `start_type` is the type the walk leaves from: a query's `target_type`, or
/// the type a play's trigger selects. `None` (a selection over every type)
/// can resolve built-in relationships only.
///
/// Each hop is resolved against the declared type the previous hop reaches.
/// A name that resolves to nothing is an error, not an empty walk: an empty
/// result would be indistinguishable from "declared, but nothing linked yet".
pub async fn resolve_path(
    node_service: &NodeService,
    start_type: Option<&str>,
    path: &RelationshipPath,
) -> Result<ResolvedPath, PathResolveError> {
    let mut hops = Vec::with_capacity(path.len());
    let mut current_type = start_type.map(str::to_string);
    for (index, hop) in path.hops().iter().enumerate() {
        let lookup = |error: NodeServiceError| PathResolveError::Lookup {
            name: hop.name.clone(),
            error: error.to_string(),
        };
        match resolve_hop(node_service, current_type.as_deref(), hop)
            .await
            .map_err(lookup)?
        {
            HopResolution::Resolved(resolved) => {
                current_type = resolved.far_type.clone();
                hops.push(resolved);
            }
            HopResolution::Undeclared => {
                let node_type = current_type.unwrap_or_default();
                let message = undeclared_message(node_service, &node_type, &hop.name)
                    .await
                    .map_err(lookup)?;
                return Err(PathResolveError::Undeclared {
                    hop: index,
                    name: hop.name.clone(),
                    node_type,
                    message,
                });
            }
            HopResolution::TypeUnknown => {
                return Err(match index.checked_sub(1) {
                    Some(previous) => PathResolveError::TypeUnknown {
                        hop: index,
                        name: hop.name.clone(),
                        previous: path.hops()[previous].name.clone(),
                        builtins: builtin_names(),
                    },
                    None => PathResolveError::NoStartType {
                        name: hop.name.clone(),
                        builtins: builtin_names(),
                    },
                });
            }
        }
    }
    Ok(ResolvedPath { hops })
}

/// The error for a name `node_type` declares in neither direction: it lists
/// the names that do resolve, so the caller can repair the path from the
/// error alone.
async fn undeclared_message(
    node_service: &NodeService,
    node_type: &str,
    name: &str,
) -> Result<String, NodeServiceError> {
    let (own, _owners) = node_service.resolve_relationships(node_type).await?;
    let inbound = node_service.get_inbound_relationships(node_type).await?;

    let mut available: Vec<String> = own
        .iter()
        .map(|rel| rel.name.clone())
        .chain(inbound.iter().flat_map(|(_, rel)| {
            // The far end of a declaration reads by its reverse name and by
            // its forward name alike.
            [rel.reverse_name.clone(), rel.name.clone()]
        }))
        .filter(|candidate| {
            !BUILTIN_RELATIONSHIP_NAMES.contains(&candidate.as_str()) && !candidate.is_empty()
        })
        .collect();
    available.sort();
    available.dedup();

    let available = if available.is_empty() {
        "no relationships are declared for this type".to_string()
    } else {
        format!("available: {}", available.join(", "))
    };
    Ok(format!(
        "Relationship '{name}' is not declared for node type '{node_type}' in either direction \
         ({available}). Built-in relationships ({}, and their reverse names) are universal.",
        builtin_names()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A service over a fresh database: the core schemas, where `task`,
    /// `project` and `person` declare the relationships used below.
    async fn service() -> (Arc<NodeService>, TempDir) {
        let dir = TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(dir.path().join("test.db")).await.unwrap());
        (Arc::new(NodeService::new(&mut store).await.unwrap()), dir)
    }

    async fn hop(svc: &NodeService, node_type: Option<&str>, name: &str) -> HopResolution {
        resolve_hop(svc, node_type, &RelationshipHop::fixed(name))
            .await
            .unwrap()
    }

    fn resolved(resolution: HopResolution) -> ResolvedHop {
        match resolution {
            HopResolution::Resolved(hop) => hop,
            other => panic!("expected a resolved hop, got {other:?}"),
        }
    }

    /// Built-ins resolve without a type: the forward name walks the edge
    /// outbound, its fixed reverse name walks the same edge inbound.
    #[tokio::test]
    async fn builtin_names_resolve_without_a_type() {
        let (svc, _dir) = service().await;

        let children = resolved(hop(&svc, None, "has_child").await);
        assert_eq!(children.relationship_type, "has_child");
        assert_eq!(children.direction, HopDirection::Outbound);
        assert_eq!(children.far_type, None);

        let parent = resolved(hop(&svc, None, "child_of").await);
        assert_eq!(parent.relationship_type, "has_child");
        assert_eq!(parent.direction, HopDirection::Inbound);
        assert_eq!(
            parent.source_type, None,
            "a built-in has no declarer to narrow to"
        );
    }

    /// A forward name resolves on the type that declares it, and on a type
    /// that inherits it.
    #[tokio::test]
    async fn a_forward_name_resolves_to_its_declared_target() {
        let (svc, _dir) = service().await;

        let tasks = resolved(hop(&svc, Some("project"), "tasks").await);
        assert_eq!(tasks.relationship_type, "tasks");
        assert_eq!(tasks.direction, HopDirection::Outbound);
        assert_eq!(tasks.far_type.as_deref(), Some("task"));
        assert!(tasks.declared_many);
        assert_eq!(tasks.source_type, None);
    }

    /// `project.tasks` and `person.tasks` both target `task`. Each reverse
    /// name belongs to one of them, so the hop is narrowed to its declarer.
    #[tokio::test]
    async fn a_reverse_name_is_narrowed_to_its_declarer() {
        let (svc, _dir) = service().await;

        let project = resolved(hop(&svc, Some("task"), "project").await);
        assert_eq!(project.relationship_type, "tasks");
        assert_eq!(project.direction, HopDirection::Inbound);
        assert_eq!(project.source_type.as_deref(), Some("project"));
        assert_eq!(project.far_type.as_deref(), Some("project"));
        assert!(!project.declared_many, "a task has one project");

        let assignee = resolved(hop(&svc, Some("task"), "assignee").await);
        assert_eq!(assignee.relationship_type, "tasks");
        assert_eq!(assignee.source_type.as_deref(), Some("person"));
    }

    /// The forward name read from the target end asks for it generically:
    /// every declarer's edges answer, so the hop is not narrowed.
    #[tokio::test]
    async fn a_forward_name_from_the_target_end_is_not_narrowed() {
        let (svc, _dir) = service().await;

        let sources = resolved(hop(&svc, Some("task"), "tasks").await);
        assert_eq!(sources.relationship_type, "tasks");
        assert_eq!(sources.direction, HopDirection::Inbound);
        assert_eq!(sources.source_type, None);
    }

    /// A declaration with no target type is inbound for every type. When a
    /// typed declaration spells the same reverse name toward this type, the
    /// typed one is the relationship meant, whichever was declared first.
    #[tokio::test]
    async fn a_typed_declaration_wins_a_reverse_name_over_an_untyped_one() {
        let (svc, _dir) = service().await;
        let relationship = |target: Option<&str>| {
            let mut rel = serde_json::json!({
                "name": "watches", "direction": "out", "cardinality": "many",
                "reverseName": "watchers", "reverseCardinality": "many"
            });
            if let Some(target) = target {
                rel["targetType"] = serde_json::json!(target);
            }
            rel
        };
        // Declared first, with no target type.
        crate::schema::handle_create_schema(
            &svc,
            serde_json::json!({ "name": "po_anything", "fields": [], "relationships": [relationship(None)] }),
        )
        .await
        .unwrap();
        crate::schema::handle_create_schema(
            &svc,
            serde_json::json!({ "name": "po_fan", "fields": [], "relationships": [relationship(Some("task"))] }),
        )
        .await
        .unwrap();

        let watchers = resolved(hop(&svc, Some("task"), "watchers").await);
        assert_eq!(watchers.source_type.as_deref(), Some("po_fan"));
        assert!(!watchers.untyped);
    }

    #[tokio::test]
    async fn an_undeclared_name_and_an_unknown_type_are_told_apart() {
        let (svc, _dir) = service().await;

        assert_eq!(
            hop(&svc, Some("task"), "no_such_relationship").await,
            HopResolution::Undeclared
        );
        assert_eq!(
            hop(&svc, None, "project").await,
            HopResolution::TypeUnknown,
            "a declared name cannot be resolved without a type"
        );
    }

    /// Each hop of a path is resolved against the declared type the previous
    /// hop reaches.
    #[tokio::test]
    async fn a_path_resolves_hop_by_hop_through_declared_types() {
        let (svc, _dir) = service().await;

        // A task's project's tasks, then each of those tasks' parent.
        let path = RelationshipPath(vec![
            RelationshipHop::fixed("project"),
            RelationshipHop::fixed("tasks"),
            RelationshipHop::open_ended("child_of"),
        ]);
        let resolved = resolve_path(&svc, Some("task"), &path).await.unwrap();

        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved.hops[0].far_type.as_deref(), Some("project"));
        assert_eq!(resolved.hops[1].direction, HopDirection::Outbound);
        assert_eq!(resolved.hops[1].far_type.as_deref(), Some("task"));
        assert!(resolved.hops[2].open_ended);
        assert_eq!(resolved.far_type(), None, "any type may be a parent");
    }

    /// A name that resolves to nothing is an error that says what does
    /// resolve, not an empty walk.
    #[tokio::test]
    async fn an_unresolvable_path_is_an_error_naming_the_hop() {
        let (svc, _dir) = service().await;

        let err = resolve_path(
            &svc,
            Some("task"),
            &RelationshipPath::from_names(["project", "no_such_relationship"]),
        )
        .await
        .unwrap_err();
        match &err {
            PathResolveError::Undeclared {
                hop,
                name,
                node_type,
                message,
            } => {
                assert_eq!(*hop, 1);
                assert_eq!(name, "no_such_relationship");
                assert_eq!(node_type, "project");
                assert!(
                    message.contains("available:") && message.contains("tasks"),
                    "{message}"
                );
            }
            other => panic!("expected Undeclared, got {other:?}"),
        }

        // A declared name after a built-in hop has no type to resolve against.
        let err = resolve_path(
            &svc,
            Some("task"),
            &RelationshipPath::from_names(["child_of", "project"]),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, PathResolveError::TypeUnknown { hop: 1, previous, .. } if previous == "child_of"),
            "{err:?}"
        );

        // And none at all from a selection over every type.
        let err = resolve_path(&svc, None, &RelationshipPath::from_names(["project"]))
            .await
            .unwrap_err();
        assert!(
            matches!(err, PathResolveError::NoStartType { .. }),
            "{err:?}"
        );
    }
}
