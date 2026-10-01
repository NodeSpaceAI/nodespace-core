//! Collection Operations
//!
//! Thin orchestration wrappers over `CollectionService`. Extracted from the daemon
//! layer so both the gRPC adapter and future callers share the same logic.

use crate::models::{Node, NodeUpdate};
use crate::ops::OpsError;
use crate::services::collection_service::deterministic_collection_id;
use crate::services::{
    CollectionService, CreateNodeParams, InsertPositionOwned, NodeService, NodeServiceError,
};
use std::sync::Arc;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct GetAllCollectionsInput;

#[derive(Debug)]
pub struct CollectionEntry {
    pub node: Node,
    /// Number of **content** members (user-authored nodes) in this collection.
    /// System/definition/decorative members (person, schema, database-settings,
    /// nested collection nodes, horizontal-line) are NOT counted, so this matches
    /// what the collection viewer lists and the sidebar's empty-collection pruning.
    pub member_count: usize,
    pub parent_collection_ids: Vec<String>,
}

#[derive(Debug)]
pub struct GetAllCollectionsOutput {
    pub collections: Vec<CollectionEntry>,
}

#[derive(Debug)]
pub struct GetCollectionMembersInput {
    pub collection_id: String,
}

#[derive(Debug)]
pub struct GetCollectionMembersOutput {
    pub members: Vec<Node>,
    pub collection_id: String,
}

#[derive(Debug)]
pub struct GetCollectionMembersRecursiveInput {
    pub collection_id: String,
}

#[derive(Debug)]
pub struct GetCollectionMembersRecursiveOutput {
    pub members: Vec<Node>,
    pub collection_id: String,
}

#[derive(Debug)]
pub struct GetNodeCollectionsInput {
    pub node_id: String,
}

#[derive(Debug)]
pub struct GetNodeCollectionsOutput {
    pub collection_ids: Vec<String>,
}

#[derive(Debug)]
pub struct AddNodeToCollectionInput {
    pub node_id: String,
    pub collection_id: String,
}

#[derive(Debug)]
pub struct AddNodeToCollectionOutput;

#[derive(Debug)]
pub struct AddNodeToCollectionByPathInput {
    pub node_id: String,
    pub collection_path: String,
}

#[derive(Debug)]
pub struct AddNodeToCollectionByPathOutput {
    pub collection_id: String,
}

#[derive(Debug)]
pub struct RemoveNodeFromCollectionInput {
    pub node_id: String,
    pub collection_id: String,
}

#[derive(Debug)]
pub struct RemoveNodeFromCollectionOutput;

#[derive(Debug)]
pub struct FindCollectionByPathInput {
    pub collection_path: String,
}

#[derive(Debug)]
pub struct FindCollectionByPathOutput {
    pub collection: Option<Node>,
}

#[derive(Debug)]
pub struct GetCollectionByNameInput {
    pub name: String,
}

#[derive(Debug)]
pub struct GetCollectionByNameOutput {
    pub collection: Option<Node>,
}

#[derive(Debug)]
pub struct CreateCollectionInput {
    pub name: String,
    pub description: String,
}

#[derive(Debug)]
pub struct CreateCollectionOutput {
    pub collection_id: String,
}

#[derive(Debug)]
pub struct RenameCollectionInput {
    pub collection_id: String,
    pub new_name: String,
    pub version: i64,
}

#[derive(Debug)]
pub struct RenameCollectionOutput {
    pub node: Node,
}

// ============================================================================
// Operations
// ============================================================================

pub async fn get_all_collections(
    node_service: &Arc<NodeService>,
    _input: GetAllCollectionsInput,
) -> Result<GetAllCollectionsOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let entries = collection_service
        .get_all_collections_with_counts()
        .await
        .map_err(OpsError::from)?;

    Ok(GetAllCollectionsOutput {
        collections: entries
            .into_iter()
            .map(
                |(node, member_count, parent_collection_ids)| CollectionEntry {
                    node,
                    member_count,
                    parent_collection_ids,
                },
            )
            .collect(),
    })
}

pub async fn get_collection_members(
    node_service: &Arc<NodeService>,
    input: GetCollectionMembersInput,
) -> Result<GetCollectionMembersOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let members = collection_service
        .get_collection_members(&input.collection_id, false)
        .await
        .map_err(OpsError::from)?;

    Ok(GetCollectionMembersOutput {
        collection_id: input.collection_id,
        members,
    })
}

pub async fn get_collection_members_recursive(
    node_service: &Arc<NodeService>,
    input: GetCollectionMembersRecursiveInput,
) -> Result<GetCollectionMembersRecursiveOutput, OpsError> {
    let store = node_service.store();
    let collection_service = CollectionService::new(store, node_service);
    let member_ids = collection_service
        .get_collection_members_recursive(&input.collection_id, false)
        .await
        .map_err(OpsError::from)?;

    let nodes_map = store
        .get_nodes_by_ids(&member_ids)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to batch fetch nodes: {}", e)))?;

    // Preserve ordering from member_ids; filter out missing entries.
    let members: Vec<Node> = member_ids
        .iter()
        .filter_map(|id| nodes_map.get(id).cloned())
        .collect();

    Ok(GetCollectionMembersRecursiveOutput {
        collection_id: input.collection_id,
        members,
    })
}

pub async fn get_node_collections(
    node_service: &Arc<NodeService>,
    input: GetNodeCollectionsInput,
) -> Result<GetNodeCollectionsOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let collection_ids = collection_service
        .get_node_collections(&input.node_id)
        .await
        .map_err(OpsError::from)?;

    Ok(GetNodeCollectionsOutput { collection_ids })
}

pub async fn add_node_to_collection(
    node_service: &Arc<NodeService>,
    input: AddNodeToCollectionInput,
) -> Result<AddNodeToCollectionOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    collection_service
        .add_to_collection(&input.node_id, &input.collection_id)
        .await
        .map_err(OpsError::from)?;

    Ok(AddNodeToCollectionOutput)
}

pub async fn add_node_to_collection_by_path(
    node_service: &Arc<NodeService>,
    input: AddNodeToCollectionByPathInput,
) -> Result<AddNodeToCollectionByPathOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let resolved = collection_service
        .add_to_collection_by_path(&input.node_id, &input.collection_path)
        .await
        .map_err(OpsError::from)?;

    Ok(AddNodeToCollectionByPathOutput {
        collection_id: resolved.leaf_id().to_string(),
    })
}

pub async fn remove_node_from_collection(
    node_service: &Arc<NodeService>,
    input: RemoveNodeFromCollectionInput,
) -> Result<RemoveNodeFromCollectionOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    collection_service
        .remove_from_collection(&input.node_id, &input.collection_id)
        .await
        .map_err(OpsError::from)?;

    Ok(RemoveNodeFromCollectionOutput)
}

pub async fn find_collection_by_path(
    node_service: &Arc<NodeService>,
    input: FindCollectionByPathInput,
) -> Result<FindCollectionByPathOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let collection = collection_service
        .find_collection_by_path(&input.collection_path)
        .await
        .map_err(OpsError::from)?;

    Ok(FindCollectionByPathOutput { collection })
}

pub async fn get_collection_by_name(
    node_service: &Arc<NodeService>,
    input: GetCollectionByNameInput,
) -> Result<GetCollectionByNameOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let collection = collection_service
        .get_collection_by_name(&input.name)
        .await
        .map_err(OpsError::from)?;

    Ok(GetCollectionByNameOutput { collection })
}

/// The id a new collection named `name` takes, or `AlreadyExists` when a node
/// already holds it.
///
/// The id is a pure hash of the normalized name (`deterministic_collection_id`),
/// so a collection created here converges with the same-named collection
/// created on another device or by import (`CollectionService::create_collection`
/// derives the same id) instead of minting a random UUID that becomes a
/// duplicate.
///
/// Two checks, in this order:
///
/// 1. **The id.** It is independent of lifecycle_status — any existing node at
///    that id, active OR archived, already occupies the row the INSERT needs.
///    A name lookup alone would miss an archived collection
///    (`CollectionService::get_collection_by_name` — correctly, for its own
///    callers — only matches active ones), and the INSERT would then fail with
///    an opaque primary-key-constraint error, since `NodeService::create_node`
///    has no get-or-create/upsert semantics for collections (unlike, e.g., the
///    `database-settings` singleton).
/// 2. **The active name.** A renamed collection keeps the id of its first
///    name, so the id of its new name is free: without this check, a create
///    under that new name would make a second active collection of the name.
///    For the same reason the id of a renamed collection's first name stays
///    taken.
pub(crate) async fn new_collection_id(
    node_service: &Arc<NodeService>,
    name: &str,
) -> Result<String, OpsError> {
    let deterministic_id = deterministic_collection_id(name);
    let in_the_way = match node_service.get_node(&deterministic_id).await? {
        Some(holder) => Some(holder),
        // Trimmed, as the id is, so a padded name cannot slip past.
        None => {
            CollectionService::new(node_service.store(), node_service)
                .get_collection_by_name(name.trim())
                .await?
        }
    };
    match in_the_way {
        Some(existing) => Err(collection_in_the_way(&existing)),
        None => Ok(deterministic_id),
    }
}

/// The refusal for a collection name that `existing` already holds. It names
/// that collection and its id, which is what the caller needs to act on it
/// instead.
fn collection_in_the_way(existing: &Node) -> OpsError {
    OpsError::AlreadyExists {
        id: format!("collection '{}' (id {})", existing.content, existing.id),
    }
}

pub async fn create_collection(
    node_service: &Arc<NodeService>,
    input: CreateCollectionInput,
) -> Result<CreateCollectionOutput, OpsError> {
    let deterministic_id = new_collection_id(node_service, &input.name).await?;

    let properties = if input.description.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({ "collection": { "description": input.description } })
    };

    let collection_id = node_service
        .create_node_with_parent(CreateNodeParams {
            id: Some(deterministic_id),
            node_type: "collection".to_string(),
            content: input.name,
            parent_id: None,
            position: InsertPositionOwned::End,
            properties,
            lifecycle_status: None,
        })
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to create collection node: {}", e)))?;

    Ok(CreateCollectionOutput { collection_id })
}

pub async fn rename_collection(
    node_service: &Arc<NodeService>,
    input: RenameCollectionInput,
) -> Result<RenameCollectionOutput, OpsError> {
    let collection_service = CollectionService::new(node_service.store(), node_service);

    if let Some(existing) = collection_service
        .get_collection_by_name(&input.new_name)
        .await
        .map_err(OpsError::from)?
    {
        if existing.id != input.collection_id {
            return Err(collection_in_the_way(&existing));
        }
    }

    let update = NodeUpdate {
        content: Some(input.new_name),
        ..Default::default()
    };

    let node = node_service
        .update_node(&input.collection_id, input.version, update)
        .await
        .map_err(|e| match e {
            NodeServiceError::VersionConflict {
                node_id,
                expected_version,
                actual_version,
            } => OpsError::VersionConflict {
                node_id,
                expected: expected_version,
                actual: actual_version,
                current_node: None,
            },
            other => OpsError::from(other),
        })?;

    Ok(RenameCollectionOutput { node })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;
    use crate::ops::node_ops;
    use crate::services::NodeService;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn make_service() -> (Arc<NodeService>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let svc = Arc::new(NodeService::new(&mut store).await.unwrap());
        (svc, tmp)
    }

    #[tokio::test]
    async fn create_collection_duplicate_name_returns_already_exists() {
        let (svc, _tmp) = make_service().await;

        let input = CreateCollectionInput {
            name: "my-collection".to_string(),
            description: String::new(),
        };
        let created = create_collection(&svc, input).await.unwrap();

        let dup = CreateCollectionInput {
            name: "my-collection".to_string(),
            description: String::new(),
        };
        let err = create_collection(&svc, dup).await.unwrap_err();
        assert_already_exists(&err, "my-collection", &created.collection_id);
    }

    /// The refusal names the collection in the way and its id, so the caller
    /// can update that collection instead.
    fn assert_already_exists(err: &OpsError, existing_name: &str, existing_id: &str) {
        let expected = format!("collection '{existing_name}' (id {existing_id})");
        assert!(
            matches!(err, OpsError::AlreadyExists { id } if *id == expected),
            "expected AlreadyExists for {expected}, got {err:?}"
        );
    }

    /// A collection created via the UI/gRPC op on one store must get the SAME id as the
    /// same-named collection created via the import path (`resolve_path`) on another
    /// store, so two devices converge instead of syncing up duplicate collection nodes.
    #[tokio::test]
    async fn ui_created_collection_id_matches_import_path_across_stores() {
        let (svc_a, _a) = make_service().await;
        let (svc_b, _b) = make_service().await;

        let ui = create_collection(
            &svc_a,
            CreateCollectionInput {
                name: "Architecture".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();

        let imported = CollectionService::new(svc_b.store(), &svc_b)
            .resolve_path("Architecture")
            .await
            .unwrap();

        assert_eq!(
            ui.collection_id, imported.leaf.id,
            "UI-created and import-created collections of the same name converge on one id"
        );
        assert_eq!(
            ui.collection_id,
            deterministic_collection_id("Architecture")
        );
    }

    fn generic_create(name: &str, properties: serde_json::Value) -> node_ops::CreateNodeInput {
        node_ops::CreateNodeInput {
            id: None,
            node_type: "collection".to_string(),
            content: name.to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        }
    }

    #[tokio::test]
    async fn create_collection_stores_the_description_in_the_collection_bucket() {
        let (svc, _tmp) = make_service().await;

        let created = create_collection(
            &svc,
            CreateCollectionInput {
                name: "Clients".to_string(),
                description: "Accounts we bill".to_string(),
            },
        )
        .await
        .unwrap();

        let node = svc.get_node(&created.collection_id).await.unwrap().unwrap();
        assert_eq!(
            node.properties,
            serde_json::json!({ "collection": { "description": "Accounts we bill" } }),
            "the description is in the collection bucket and nowhere else"
        );
    }

    #[tokio::test]
    async fn create_collection_without_a_description_stores_none() {
        let (svc, _tmp) = make_service().await;

        let created = create_collection(
            &svc,
            CreateCollectionInput {
                name: "Clients".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();

        let node = svc.get_node(&created.collection_id).await.unwrap().unwrap();
        assert_eq!(node.properties, serde_json::json!({ "collection": {} }));
    }

    /// The generic create is what the CLI's `node create --type collection`
    /// and the agent's `create_node` call. It must make the same node the
    /// collection op and an import make, not a second collection of that name
    /// under a random id.
    #[tokio::test]
    async fn generic_create_of_a_collection_uses_the_deterministic_id() {
        let (svc, _tmp) = make_service().await;

        let created = node_ops::create_node(
            &svc,
            generic_create(
                "Clients",
                serde_json::json!({ "description": "Accounts we bill" }),
            ),
        )
        .await
        .unwrap();

        assert_eq!(created.node_id, deterministic_collection_id("Clients"));
        let node = svc.get_node(&created.node_id).await.unwrap().unwrap();
        assert_eq!(
            node.properties,
            serde_json::json!({ "collection": { "description": "Accounts we bill" } })
        );
    }

    #[tokio::test]
    async fn generic_create_of_an_existing_collection_name_returns_already_exists() {
        let (svc, _tmp) = make_service().await;

        let created = create_collection(
            &svc,
            CreateCollectionInput {
                name: "Clients".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();

        // Names are compared case-insensitively, as the id is.
        let err = node_ops::create_node(&svc, generic_create("clients", serde_json::json!({})))
            .await
            .unwrap_err();
        assert_already_exists(&err, "Clients", &created.collection_id);
    }

    /// A subtype of collection is a collection: it is named the same way, so
    /// it takes the name-derived id, and its inherited description lands in
    /// the collection bucket.
    #[tokio::test]
    async fn generic_create_of_a_collection_subtype_uses_the_deterministic_id() {
        let (svc, _tmp) = make_service().await;
        crate::schema::handle_create_schema(
            &svc,
            serde_json::json!({
                "name": "Shelf",
                "extends": "collection",
                "fields": [
                    { "name": "room", "type": "text", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .unwrap();

        let mut input = generic_create(
            "Cookbooks",
            serde_json::json!({ "description": "Recipes we kept", "room": "kitchen" }),
        );
        input.node_type = "shelf".to_string();
        let created = node_ops::create_node(&svc, input).await.unwrap();

        assert_eq!(created.node_id, deterministic_collection_id("Cookbooks"));
        let node = svc.get_node(&created.node_id).await.unwrap().unwrap();
        assert_eq!(
            node.properties,
            serde_json::json!({
                "shelf": { "room": "kitchen" },
                "collection": { "description": "Recipes we kept" }
            })
        );

        // The agent's workspace context lists it like any other collection.
        let listed = CollectionService::new(svc.store(), &svc)
            .get_all_collection_descriptions()
            .await
            .unwrap();
        assert_eq!(
            listed,
            [("Cookbooks".to_string(), Some("Recipes we kept".to_string()))]
        );
    }

    /// A renamed collection keeps the id of its first name, so the id check
    /// alone would let a second collection take its new name.
    #[tokio::test]
    async fn create_refuses_the_name_of_a_renamed_collection() {
        let (svc, _tmp) = make_service().await;

        let created = create_collection(
            &svc,
            CreateCollectionInput {
                name: "Clients".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
        let version = svc
            .get_node(&created.collection_id)
            .await
            .unwrap()
            .unwrap()
            .version;
        rename_collection(
            &svc,
            RenameCollectionInput {
                collection_id: created.collection_id.clone(),
                new_name: "Accounts".to_string(),
                version,
            },
        )
        .await
        .unwrap();

        // Padded and differently-cased spellings are the same name.
        for name in ["Accounts", "Clients", " accounts "] {
            let err = node_ops::create_node(&svc, generic_create(name, serde_json::json!({})))
                .await
                .unwrap_err();
            assert_already_exists(&err, "Accounts", &created.collection_id);

            let err = create_collection(
                &svc,
                CreateCollectionInput {
                    name: name.to_string(),
                    description: String::new(),
                },
            )
            .await
            .unwrap_err();
            assert_already_exists(&err, "Accounts", &created.collection_id);
        }
    }

    #[tokio::test]
    async fn generic_update_sets_changes_and_clears_a_collection_description() {
        let (svc, _tmp) = make_service().await;
        let created = node_ops::create_node(&svc, generic_create("Clients", serde_json::json!({})))
            .await
            .unwrap();

        let update = |properties: serde_json::Value| node_ops::UpdateNodeInput {
            node_id: created.node_id.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(properties),
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        };
        let stored = || async {
            svc.get_node(&created.node_id)
                .await
                .unwrap()
                .unwrap()
                .properties
        };

        node_ops::update_node(
            &svc,
            update(serde_json::json!({ "description": "Accounts" })),
        )
        .await
        .unwrap();
        assert_eq!(
            stored().await,
            serde_json::json!({ "collection": { "description": "Accounts" } })
        );

        node_ops::update_node(
            &svc,
            update(serde_json::json!({ "description": "Accounts we bill" })),
        )
        .await
        .unwrap();
        assert_eq!(
            stored().await,
            serde_json::json!({ "collection": { "description": "Accounts we bill" } })
        );

        // The generic update clears a field by writing null, which reads as
        // no description.
        node_ops::update_node(&svc, update(serde_json::json!({ "description": null })))
            .await
            .unwrap();
        assert_eq!(
            stored().await,
            serde_json::json!({ "collection": { "description": null } })
        );
        let listed = CollectionService::new(svc.store(), &svc)
            .get_all_collection_descriptions()
            .await
            .unwrap();
        assert_eq!(listed, [("Clients".to_string(), None)]);
    }

    /// A text field is not type-checked on write, and the CLI parses
    /// `--property description=42` as a number. The listing runs on every
    /// agent turn, so a stored non-text value must read as no description
    /// rather than fail it.
    #[tokio::test]
    async fn a_non_text_collection_description_is_listed_as_none() {
        let (svc, _tmp) = make_service().await;
        for (name, description) in [
            ("Numbered", serde_json::json!(42)),
            ("Flagged", serde_json::json!(true)),
            ("Nested", serde_json::json!({ "text": "Accounts" })),
        ] {
            node_ops::create_node(
                &svc,
                generic_create(name, serde_json::json!({ "description": description })),
            )
            .await
            .unwrap();
        }

        let listed = CollectionService::new(svc.store(), &svc)
            .get_all_collection_descriptions()
            .await
            .unwrap();
        assert_eq!(
            listed,
            [
                ("Flagged".to_string(), None),
                ("Nested".to_string(), None),
                ("Numbered".to_string(), None),
            ]
        );
    }

    #[tokio::test]
    async fn collection_descriptions_are_listed_by_name() {
        let (svc, _tmp) = make_service().await;
        for (name, description) in [("Research", ""), ("Clients", "Accounts we bill")] {
            create_collection(
                &svc,
                CreateCollectionInput {
                    name: name.to_string(),
                    description: description.to_string(),
                },
            )
            .await
            .unwrap();
        }

        let listed = CollectionService::new(svc.store(), &svc)
            .get_all_collection_descriptions()
            .await
            .unwrap();
        assert_eq!(
            listed,
            [
                ("Clients".to_string(), Some("Accounts we bill".to_string())),
                ("Research".to_string(), None),
            ]
        );
    }

    #[tokio::test]
    async fn rename_collection_to_same_name_succeeds() {
        let (svc, _tmp) = make_service().await;

        let output = create_collection(
            &svc,
            CreateCollectionInput {
                name: "orig".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();

        let node = svc.get_node(&output.collection_id).await.unwrap().unwrap();
        let result = rename_collection(
            &svc,
            RenameCollectionInput {
                collection_id: output.collection_id.clone(),
                new_name: "orig".to_string(),
                version: node.version,
            },
        )
        .await;
        assert!(result.is_ok(), "self-rename should succeed");
    }

    #[tokio::test]
    async fn rename_collection_to_existing_name_returns_already_exists() {
        let (svc, _tmp) = make_service().await;

        let alpha = create_collection(
            &svc,
            CreateCollectionInput {
                name: "alpha".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();

        let beta = create_collection(
            &svc,
            CreateCollectionInput {
                name: "beta".to_string(),
                description: String::new(),
            },
        )
        .await
        .unwrap();

        let node = svc.get_node(&beta.collection_id).await.unwrap().unwrap();
        let err = rename_collection(
            &svc,
            RenameCollectionInput {
                collection_id: beta.collection_id,
                new_name: "alpha".to_string(),
                version: node.version,
            },
        )
        .await
        .unwrap_err();
        assert_already_exists(&err, "alpha", &alpha.collection_id);
    }
}
