//! The daemon's extension points (ADR-082 §5), used the way a build composing
//! the daemon uses them: through `build_shared_services` and the public API
//! alone.
//!
//! This file tests the daemon half of the extension points (ADR-082 §9).
//!
//! Every test that calls `build_shared_services` first points the daemon's
//! home at a temporary directory and its embedding model at a file that does
//! not exist, so no test here reads the user's state or loads a model.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use nodespace_core::behaviors::{BehaviorRegistrationError, CollectionNodeBehavior, NodeBehavior};
use nodespace_core::extensions::{DataExtensionsError, EdgeFieldDeclaration};
use nodespace_core::models::schema::{EdgeField, EnumValue, SchemaFieldType};
use nodespace_core::{
    Node, NodeService, NodeUpdate, SqliteStore, ValidationError as NodeValidationError,
};
use nodespace_daemon::{
    build_shared_services, DaemonExtensions, DaemonExtensionsError, DatabaseManager,
    DatabaseRequiresExtensions, SharedServices,
};
use serde_json::json;
use tempfile::TempDir;

/// The extension id the fixture declares.
const FIXTURE_ID: &str = "fixture";

/// The fixture's subtype of `collection`.
const FIXTURE_TYPE: &str = "fixture-collection";

/// The behaviour of `fixture-collection extends collection`: a `max_members`
/// in its bucket must be at least 1.
struct FixtureCollectionBehavior;

impl NodeBehavior for FixtureCollectionBehavior {
    fn type_name(&self) -> &'static str {
        FIXTURE_TYPE
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        let max_members = node
            .properties
            .get(FIXTURE_TYPE)
            .and_then(|bucket| bucket.get("max_members"))
            .and_then(serde_json::Value::as_f64);
        match max_members {
            Some(n) if n < 1.0 => Err(NodeValidationError::InvalidProperties(
                "max_members must be at least 1".to_string(),
            )),
            _ => Ok(()),
        }
    }

    fn supports_markdown(&self) -> bool {
        false
    }
}

/// Held by every [`IsolatedDaemonHome`], so tests that share a process (a
/// plain `cargo test`) take turns with the environment instead of one test
/// restoring it while another still resolves the model path from it.
static ENVIRONMENT: Mutex<()> = Mutex::new(());

/// Points `NODESPACE_HOME` at a temporary directory and
/// `NODESPACED_MODEL_PATH` at a file that does not exist, and restores both
/// when dropped, a panic included. The environment is process-global: nextest
/// runs each test in its own process, and [`ENVIRONMENT`] serializes tests
/// that share one.
struct IsolatedDaemonHome {
    home: TempDir,
    saved: Vec<(&'static str, Option<OsString>)>,
    _environment: MutexGuard<'static, ()>,
}

impl IsolatedDaemonHome {
    fn new() -> Self {
        // A test that panicked while holding the lock restored the
        // environment as it unwound, so its poison carries no meaning here.
        let environment = ENVIRONMENT.lock().unwrap_or_else(PoisonError::into_inner);
        let home = TempDir::new().unwrap();
        let saved = ["NODESPACE_HOME", "NODESPACED_MODEL_PATH"]
            .map(|var| (var, std::env::var_os(var)))
            .into();
        std::env::set_var("NODESPACE_HOME", home.path());
        std::env::set_var(
            "NODESPACED_MODEL_PATH",
            home.path().join("no-model-here.gguf"),
        );
        Self {
            home,
            saved,
            _environment: environment,
        }
    }

    fn path(&self) -> &Path {
        self.home.path()
    }
}

impl Drop for IsolatedDaemonHome {
    fn drop(&mut self) {
        for (var, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
    }
}

/// The daemon's shared services, built as a composing build builds them.
/// The context carries the extensions every database is built from.
async fn shared_services(extensions: DaemonExtensions) -> SharedServices {
    extensions
        .check()
        .expect("the fixture's extensions are valid");
    let declared = extensions.supported_extensions().to_vec();
    let (shared, model_task) = build_shared_services(extensions)
        .await
        .expect("the daemon starts");
    assert!(
        model_task.is_none() && !shared.context.has_model,
        "no model is loaded"
    );
    assert_eq!(shared.context.extensions.supported_extensions(), declared);
    shared
}

/// A closed database at `dir/name/name.sqlite` whose settings node requires
/// `ids`, marked through the core library the way an extension marks one.
async fn database_requiring(dir: &Path, name: &str, ids: &[&str]) -> PathBuf {
    let db_dir = dir.join(name);
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join(format!("{name}.sqlite"));
    {
        let mut store = Arc::new(SqliteStore::new(path.clone()).await.unwrap());
        let node_service = NodeService::new(&mut store).await.unwrap();
        let settings = node_service
            .get_node("database-settings-singleton")
            .await
            .unwrap()
            .unwrap();
        node_service
            .update_node(
                &settings.id,
                settings.version,
                NodeUpdate::new()
                    .with_properties(serde_json::json!({ "required_extensions": ids })),
            )
            .await
            .unwrap();
    }
    assert!(
        !PathBuf::from(format!("{}-wal", path.display())).exists(),
        "the fixture closed cleanly"
    );
    path
}

/// The unsupported ids a database open was refused for, or `None` when it
/// opened.
async fn refused_for(manager: &DatabaseManager, path: PathBuf) -> Option<Vec<String>> {
    let id = manager.register(path).await.unwrap().id;
    match manager.get_or_open(&id).await {
        Ok(_) => None,
        Err(err) => Some(
            DatabaseRequiresExtensions::find_in(&err)
                .unwrap_or_else(|| panic!("a required-extensions refusal, got {err:#}"))
                .unsupported
                .clone(),
        ),
    }
}

/// A database marked with an id the build declares opens; one that also
/// lists an id the build does not declare is refused for that id alone.
#[tokio::test]
async fn a_database_requiring_a_supported_extension_opens() {
    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none().supported_extension(FIXTURE_ID)).await;
    let marked = database_requiring(home.path(), "marked", &[FIXTURE_ID]).await;
    let mixed = database_requiring(home.path(), "mixed", &[FIXTURE_ID, "other"]).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    assert_eq!(refused_for(&manager, marked).await, None);
    assert_eq!(
        refused_for(&manager, mixed).await,
        Some(vec!["other".to_string()])
    );
}

/// Core's own daemon supports no extension: the same database is refused.
#[tokio::test]
async fn core_refuses_a_database_requiring_any_extension() {
    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none()).await;
    let marked = database_requiring(home.path(), "marked", &[FIXTURE_ID]).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    assert_eq!(
        refused_for(&manager, marked).await,
        Some(vec![FIXTURE_ID.to_string()])
    );
}

/// A malformed extension id stops the daemon before it builds anything, with
/// the error `check` reports for it.
#[tokio::test]
async fn an_invalid_extension_id_stops_startup() {
    let _home = IsolatedDaemonHome::new();
    let extensions = DaemonExtensions::none().supported_extension("Fixture");
    let expected = DaemonExtensionsError::InvalidExtensionId("Fixture".to_string());
    assert_eq!(extensions.check(), Err(expected.clone()));

    let err = build_shared_services(extensions)
        .await
        .err()
        .expect("startup is refused");

    assert_eq!(err.downcast_ref::<DaemonExtensionsError>(), Some(&expected));
}

/// A behaviour handed to `build_shared_services` is registered in every
/// database the daemon opens: each one's writes are validated by it.
#[tokio::test]
async fn a_subtype_behaviour_is_enforced_in_every_database() {
    let home = IsolatedDaemonHome::new();
    let extensions = DaemonExtensions::none().behavior(Arc::new(FixtureCollectionBehavior));
    assert_eq!(extensions.data().check(), Ok(()));
    assert!(extensions
        .data()
        .behavior_registry()
        .unwrap()
        .get(FIXTURE_TYPE)
        .is_some());
    let shared = shared_services(extensions).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    for name in ["first", "second"] {
        let path = database_requiring(home.path(), name, &[]).await;
        let id = manager.register(path).await.unwrap().id;
        let database = manager.get_or_open(&id).await.unwrap();
        let node_service = database.node_service_grpc.node_service();
        nodespace_core::schema::handle_create_schema(
            &node_service,
            json!({
                "name": FIXTURE_TYPE,
                "extends": "collection",
                "fields": [
                    { "name": "max_members", "type": "number", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .unwrap();

        let refused = node_service
            .create_node(Node::new(
                FIXTURE_TYPE.to_string(),
                "Team".to_string(),
                json!({ "max_members": 0 }),
            ))
            .await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("max_members must be at least 1")),
            "{name}: {refused:?}"
        );
        node_service
            .create_node(Node::new(
                FIXTURE_TYPE.to_string(),
                "Team".to_string(),
                json!({ "max_members": 2 }),
            ))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

/// A behaviour for a type core defines stops the daemon at startup: another
/// build may add types, never replace core's rules.
#[tokio::test]
async fn a_behaviour_for_a_core_type_stops_startup() {
    let _home = IsolatedDaemonHome::new();
    let err =
        build_shared_services(DaemonExtensions::none().behavior(Arc::new(CollectionNodeBehavior)))
            .await
            .err()
            .expect("startup is refused");

    assert_eq!(
        err.downcast_ref::<DaemonExtensionsError>(),
        Some(&DaemonExtensionsError::Data(DataExtensionsError::Behavior(
            BehaviorRegistrationError::CoreType("collection".to_string())
        )))
    );
}

/// The fixture's `permission` on `member_of`, in its bucket of the edge:
/// `admin`, `modify` or `read_only`.
fn permission_on(relationship: &str) -> EdgeFieldDeclaration {
    EdgeFieldDeclaration::new(
        relationship,
        FIXTURE_ID,
        vec![EdgeField {
            name: "permission".to_string(),
            field_type: SchemaFieldType::Enum,
            core_values: Some(
                ["admin", "modify", "read_only"]
                    .map(|v| EnumValue::new(v, v))
                    .to_vec(),
            ),
            indexed: None,
            required: Some(true),
            default: None,
            target_type: None,
            description: None,
        }],
    )
}

/// Edge fields handed to `build_shared_services` are validated in every
/// database the daemon opens, and stored in the extension's bucket.
#[tokio::test]
async fn an_edge_field_is_validated_in_every_database() {
    let home = IsolatedDaemonHome::new();
    let shared =
        shared_services(DaemonExtensions::none().edge_fields(permission_on("member_of"))).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    for name in ["first", "second"] {
        let path = database_requiring(home.path(), name, &[]).await;
        let id = manager.register(path).await.unwrap().id;
        let database = manager.get_or_open(&id).await.unwrap();
        let node_service = database.node_service_grpc.node_service();
        let collection = node_service
            .create_node(Node::new(
                "collection".to_string(),
                "Team".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        let member = node_service
            .create_node(Node::new(
                "text".to_string(),
                "Notes".to_string(),
                json!({}),
            ))
            .await
            .unwrap();

        let refused = node_service
            .create_relationship(
                &member,
                "member_of",
                &collection,
                json!({ "fixture": { "permission": "owner" } }),
            )
            .await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("one of: admin, modify, read_only")),
            "{name}: {:?}",
            refused.map(|_| ())
        );

        node_service
            .create_relationship(
                &member,
                "member_of",
                &collection,
                json!({ "fixture": { "permission": "modify" } }),
            )
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let stored = node_service
            .store()
            .get_relationship_record(&member, &collection, "member_of")
            .await
            .unwrap()
            .expect("the edge is stored");
        assert_eq!(
            stored.properties["fixture"],
            json!({ "permission": "modify" }),
            "{name}"
        );
    }
}

/// Fields on a relationship another build may not add to stop the daemon at
/// startup: a `has_child` edge is recreated when its node moves, so the
/// bucket would not survive.
#[tokio::test]
async fn an_edge_field_declaration_on_has_child_stops_startup() {
    let _home = IsolatedDaemonHome::new();
    let err =
        build_shared_services(DaemonExtensions::none().edge_fields(permission_on("has_child")))
            .await
            .err()
            .expect("startup is refused");

    assert!(
        matches!(
            err.downcast_ref::<DaemonExtensionsError>(),
            Some(DaemonExtensionsError::Data(DataExtensionsError::EdgeFields {
                relationship,
                extension_id,
                ..
            })) if relationship == "has_child" && extension_id == FIXTURE_ID
        ),
        "{err:#}"
    );
}

/// The client id the fixture reserves for its own writers.
const FIXTURE_WRITER: &str = "fixture-writer";

/// A `CreateNode` request presenting `client_id` as its `x-ns-client-id`.
fn create_as(client_id: &str) -> tonic::Request<nodespace_daemon::nodespace::CreateNodeRequest> {
    let mut request = tonic::Request::new(nodespace_daemon::nodespace::CreateNodeRequest {
        node_type: "text".into(),
        content: format!("written as {client_id}"),
        properties: "{}".into(),
        ..Default::default()
    });
    request
        .metadata_mut()
        .insert("x-ns-client-id", client_id.parse().unwrap());
    request
}

/// A client id the build reserves is refused, with `PERMISSION_DENIED`, on
/// the node API of every database the daemon opens; any other id is served.
/// The build's own writer still writes under it in process.
#[tokio::test]
async fn a_reserved_client_id_is_refused_in_every_database() {
    use nodespace_daemon::nodespace::node_service_server::NodeService as _;

    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none().reserved_client_id(FIXTURE_WRITER)).await;
    assert_eq!(
        shared.context.extensions.reserved_client_ids(),
        [FIXTURE_WRITER]
    );
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    for name in ["first", "second"] {
        let path = database_requiring(home.path(), name, &[]).await;
        let id = manager.register(path).await.unwrap().id;
        let database = manager.get_or_open(&id).await.unwrap();
        let grpc = &database.node_service_grpc;

        let refused = grpc
            .create_node(create_as(FIXTURE_WRITER))
            .await
            .unwrap_err();
        assert_eq!(
            refused.code(),
            tonic::Code::PermissionDenied,
            "{name}: {refused:?}"
        );
        grpc.create_node(create_as("window"))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));

        grpc.node_service()
            .with_client(FIXTURE_WRITER)
            .create_node(Node::new(
                "text".to_string(),
                "in process".to_string(),
                json!({}),
            ))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

/// Core's own daemon reserves no client id: the same request is served.
#[tokio::test]
async fn core_reserves_no_client_id() {
    use nodespace_daemon::nodespace::node_service_server::NodeService as _;

    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none()).await;
    assert!(shared.context.extensions.reserved_client_ids().is_empty());
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();
    let path = database_requiring(home.path(), "core", &[]).await;
    let id = manager.register(path).await.unwrap().id;
    let database = manager.get_or_open(&id).await.unwrap();

    database
        .node_service_grpc
        .create_node(create_as(FIXTURE_WRITER))
        .await
        .unwrap();
}

/// A reserved client id no request could present stops the daemon at
/// startup, with the error `check` reports for it.
#[tokio::test]
async fn an_invalid_reserved_client_id_stops_startup() {
    let _home = IsolatedDaemonHome::new();
    let extensions = DaemonExtensions::none().reserved_client_id(" padded ");
    let expected = DaemonExtensionsError::InvalidReservedClientId(" padded ".to_string());
    assert_eq!(extensions.check(), Err(expected.clone()));

    let err = build_shared_services(extensions)
        .await
        .err()
        .expect("startup is refused");

    assert_eq!(err.downcast_ref::<DaemonExtensionsError>(), Some(&expected));
}

/// A request's client id never switches playbook triggers off: a reactive
/// play fires for a node created over the node API whatever id the request
/// presents, as it does for one with none. The id core once reserved for
/// this is included, built from fragments so the value is not spelled out.
#[tokio::test]
async fn a_client_id_never_skips_playbook_triggers() {
    use nodespace_daemon::nodespace::node_service_server::NodeService as _;

    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none()).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();
    let path = database_requiring(home.path(), "plays", &[]).await;
    let id = manager.register(path).await.unwrap().id;
    let database = manager.get_or_open(&id).await.unwrap();
    let node_service = database.node_service_grpc.node_service();
    nodespace_core::schema::handle_create_schema(
        &node_service,
        json!({
            "name": "fixture-task",
            "fields": [{ "name": "status", "type": "text", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .unwrap();
    node_service
        .create_node(Node::new(
            "play".to_string(),
            "close open fixture tasks".to_string(),
            json!({ "rules": [{
                "name": "close-open",
                "description": "Closes an open task",
                "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "fixture-task" } },
                "conditions": [{ "expr": "node.status == 'open'", "description": "The task is open" }],
                "actions": [{
                    "description": "Close it",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
                }]
            }] }),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let former_reserved = ["sync", "service"].join("-");
    for client_id in [
        None,
        Some("window"),
        Some("apply"),
        Some(former_reserved.as_str()),
    ] {
        let mut request = tonic::Request::new(nodespace_daemon::nodespace::CreateNodeRequest {
            node_type: "fixture-task".into(),
            content: format!("created as {client_id:?}"),
            properties: r#"{"status":"open"}"#.into(),
            ..Default::default()
        });
        if let Some(client_id) = client_id {
            request
                .metadata_mut()
                .insert("x-ns-client-id", client_id.parse().unwrap());
        }
        let created = database
            .node_service_grpc
            .create_node(request)
            .await
            .unwrap_or_else(|e| panic!("{client_id:?}: {e:?}"))
            .into_inner()
            .node_id;

        let mut closed = false;
        for _ in 0..200 {
            let node = node_service.get_node(&created).await.unwrap().unwrap();
            if node.properties["fixture-task"]["status"] == json!("done") {
                closed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(
            closed,
            "{client_id:?}: the play fired on the request's write"
        );
    }
}
