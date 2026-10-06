//! End-to-end integration test for the `nodespace` CLI.
//!
//! Spins an in-process `nodespaced` gRPC server up against a tempdir-backed
//! SQLite database, then drives the CLI's command handlers (via the library
//! surface) at it. This validates that the CLI's gRPC plumbing — connection,
//! request construction, response unwrapping, error mapping — works end to
//! end against the same service stack the real binary uses.
//!
//! We exercise the handlers directly rather than spawning the compiled
//! binary so test failures point at the code path under test rather than
//! at fork/exec or stdout-capture plumbing.
//!
//! Two harnesses back the tests:
//! - [`spawn_test_daemon`] serves a single, plain `NodeService` — enough for the
//!   node/mention/schema/search flows that don't touch the database registry.
//! - [`spawn_routing_daemon`] serves the full multi-database stack (ADR-053):
//!   `DbManagerLayer` + `DatabaseService` + `NodeService` over a `DatabaseManager`
//!   seeded with one default database, so `database` subcommands and
//!   `--database` routing can be exercised over the real transport.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::pty::PtySessionManager;
use nodespace_cli::journal::WriteJournal;
use nodespace_cli::{commands, connect, connect_database, DatabaseIdInterceptor, NodeClient};
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{
    ConflictsForNodeRequest, CreateDatabaseRequest, CreateNodeRequest, GetConflictRequest,
    GetNodeRequest, GetRelatedNodesRequest, GetSchemaDefinitionRequest, GetSkillRequest,
    ListDatabasesRequest, NodeSortOrder, QueryNodesSimpleRequest, RunSavedQueryRequest,
    SkillGuidanceRequest,
};
use nodespace_daemon::{
    DatabaseManager, DatabaseServiceImpl, DatabaseServiceServer, DbManagerLayer, NodeServiceImpl,
    NodeServiceServer, SharedContext,
};
use nodespace_nlp_engine::EmbeddingService;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::{oneshot, watch};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic::Code;

/// `node::run` with no write journal, as a command run outside a harness session.
async fn node_run(
    client: &mut NodeClient,
    action: commands::node::NodeAction,
    json: bool,
) -> anyhow::Result<()> {
    commands::node::run(client, action, json, &WriteJournal::at(None, "")).await
}

/// Spawn an in-process daemon over a temp-dir UDS and return the socket path.
pub(crate) async fn spawn_test_daemon() -> (PathBuf, oneshot::Sender<()>, TempDir) {
    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("test-daemon.sock");

    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("failed to open SqliteStore"),
    );
    let node_service = Arc::new(
        CoreNodeService::new(&mut store)
            .await
            .expect("failed to build NodeService"),
    );
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );

    let listener = UnixListener::bind(&sock_path).expect("failed to bind test UDS socket");
    let incoming = UnixListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect(&sock_path, DatabaseIdInterceptor::none())
            .await
            .is_ok()
        {
            return (sock_path, shutdown_tx, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// Like [`spawn_test_daemon`], but seeds the real production `skill` and
/// tool registries first and also returns the `NodeService` handle, so a test can
/// make a live edit to a seeded skill before driving the CLI at it over
/// gRPC — needed for `nodespace skill reset`, which has no other way to get
/// a modified `_seed.guidance_modified` flag onto a node.
pub(crate) async fn spawn_test_daemon_with_seeded_skills(
) -> (PathBuf, oneshot::Sender<()>, TempDir, Arc<CoreNodeService>) {
    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("test-daemon.sock");

    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("failed to open SqliteStore"),
    );
    let node_service = Arc::new(
        CoreNodeService::new(&mut store)
            .await
            .expect("failed to build NodeService"),
    );

    let groups: Vec<_> = nodespace_agent::skill_pipeline::seed_skill_nodes()
        .iter()
        .chain(nodespace_agent::skill_pipeline::seed_tool_nodes().iter())
        .map(|t| {
            nodespace_core::markdown::prepare_nodes_from_template(t).expect("template must parse")
        })
        .collect();
    node_service
        .seed_nodes_from_templates(groups)
        .await
        .expect("initial seed must succeed");

    let service = NodeServiceImpl::new(
        node_service.clone(),
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );

    let listener = UnixListener::bind(&sock_path).expect("failed to bind test UDS socket");
    let incoming = UnixListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect(&sock_path, DatabaseIdInterceptor::none())
            .await
            .is_ok()
        {
            return (sock_path, shutdown_tx, tempdir, node_service);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// A model-less build context — with `has_model = false` no embedding wiring
/// runs, so the dropped watch sender is harmless (it is never read).
pub(crate) fn routing_test_context() -> SharedContext {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        scheduler: Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: nodespace_daemon::SharedLocalAgent::new(),
        extensions: nodespace_daemon::DaemonExtensions::none(),
    }
}

/// Spawn an in-process daemon over a temp-dir UDS serving the full ADR-053
/// multi-database stack: the `DbManagerLayer` injects a `DatabaseManager` into
/// every request, `DatabaseService` manages the registry, and `NodeService`
/// routes by the `x-ns-database-id` header. The registry is seeded with a single
/// default database so header-less requests work and a second can be created.
pub(crate) async fn spawn_routing_daemon() -> (PathBuf, oneshot::Sender<()>, TempDir) {
    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("routing-daemon.sock");
    let registry_path = tempdir.path().join("databases.toml");
    let default_db = tempdir.path().join("default.db");

    let manager = Arc::new(
        DatabaseManager::load(registry_path, routing_test_context())
            .await
            .expect("failed to load DatabaseManager"),
    );
    let default_id = manager
        .ensure_default_registered("Default".into(), default_db)
        .await
        .expect("failed to register default database");
    // Open the default through the manager and serve that bundle's NodeService
    // (exactly as the daemon boot path does) so the manager's cache is the served
    // handle — no double-open.
    let default_bundle = manager
        .get_or_open(&default_id)
        .await
        .expect("failed to open default database");
    let node_default = default_bundle.node_service_grpc.clone();

    let listener = UnixListener::bind(&sock_path).expect("failed to bind routing UDS socket");
    let incoming = UnixListenerStream::new(listener);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let mgr = manager.clone();
    tokio::spawn(async move {
        Server::builder()
            .layer(DbManagerLayer::new(mgr.clone()))
            .add_service(DatabaseServiceServer::new(DatabaseServiceImpl::new(mgr)))
            .add_service(NodeServiceServer::new(node_default))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect_database(&sock_path).await.is_ok() {
            return (sock_path, shutdown_tx, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "routing daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// Route a fresh `NodeService` client to `id` via the resolved routing header.
async fn node_client_for(sock: &std::path::Path, id: &str) -> nodespace_cli::NodeClient {
    let interceptor = DatabaseIdInterceptor::for_id(id).expect("build interceptor");
    connect(sock, interceptor)
        .await
        .expect("connect routed node")
}

/// A templated type (`person`, `{first_name} {last_name}`) is created from its
/// fields alone; `--content` on it surfaces the core validation error.
#[tokio::test]
async fn create_templated_type_without_content_and_reject_content() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "person".into(),
            content: None,
            parent: None,
            properties: vec![
                ("first_name".into(), serde_json::json!("Rowan")),
                ("last_name".into(), serde_json::json!("Price")),
            ],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("a person is created from its template fields alone");

    let err = node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "person".into(),
            content: Some("Rowan".into()),
            parent: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect_err("content on a templated type must be rejected");
    assert!(
        format!("{err:#}").contains("person takes its name from first_name/last_name"),
        "error should name the template fields, got: {err:#}"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn create_get_update_children_delete_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "text".into(),
            content: Some("root via CLI".into()),
            parent: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create root");

    let mut raw_client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw client connect");

    let created = raw_client
        .create_node(nodespace_daemon::nodespace::CreateNodeRequest {
            node_type: "text".into(),
            content: "parent".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed parent")
        .into_inner();
    let parent_id = created.node_id;

    node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "text".into(),
            content: Some("child via CLI".into()),
            parent: Some(parent_id.clone()),
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
        }),
        false,
    )
    .await
    .expect("create child");

    node_run(
        &mut client,
        commands::node::NodeAction::Get(commands::node::GetArgs {
            id: parent_id.clone(),
        }),
        false,
    )
    .await
    .expect("get parent");

    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: parent_id.clone(),
            content: Some("parent updated via CLI".into()),
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("update parent");

    let fetched = raw_client
        .get_node(GetNodeRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("post-update fetch")
        .into_inner();
    assert_eq!(
        fetched.node_data.expect("node_data").content,
        "parent updated via CLI"
    );

    node_run(
        &mut client,
        commands::node::NodeAction::Children(commands::node::ChildrenArgs {
            id: parent_id.clone(),
        }),
        true,
    )
    .await
    .expect("list children");

    let children = raw_client
        .get_children(nodespace_daemon::nodespace::GetChildrenRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("children fetch")
        .into_inner();
    assert_eq!(
        children.count, 1,
        "expected exactly one child seeded via CLI"
    );
    assert_eq!(children.nodes.len(), 1, "nodes len must match count");
    assert_eq!(children.nodes[0].content, "child via CLI");

    let delete = |version, descendants| {
        commands::node::NodeAction::Delete(commands::node::DeleteArgs {
            id: parent_id.clone(),
            version,
            descendants,
            routing: Vec::new(),
        })
    };

    // The bare form only previews: the parent and its child survive it.
    node_run(&mut client, delete(None, None), false)
        .await
        .expect("preview delete");
    let current = raw_client
        .get_node(GetNodeRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("a preview must not delete")
        .into_inner()
        .node_data
        .expect("node_data");

    // A confirmation that no longer matches — here, a nested count the
    // parent does not have — deletes nothing.
    let err = node_run(&mut client, delete(Some(current.version), Some(0)), false)
        .await
        .expect_err("a stale confirmation must be refused");
    assert!(
        format!("{err:#}").contains("Nothing deleted"),
        "refusal must say nothing was deleted: {err:#}"
    );
    raw_client
        .get_node(GetNodeRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("a refused delete must leave the node in place");

    node_run(&mut client, delete(Some(current.version), Some(1)), false)
        .await
        .expect("confirmed delete");

    let err = raw_client
        .get_node(GetNodeRequest { node_id: parent_id })
        .await
        .expect_err("expected not_found after delete");
    assert_eq!(err.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn get_missing_node_surfaces_not_found() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = node_run(
        &mut client,
        commands::node::NodeAction::Get(commands::node::GetArgs {
            id: "does-not-exist".into(),
        }),
        false,
    )
    .await
    .expect_err("expected error");

    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn search_without_embedding_service_reports_unavailable() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::search::run(
        &mut client,
        commands::search::SearchArgs {
            query: "anything".into(),
            node_types: vec![],
            collection: None,
            collection_id: None,
            filters: None,
            threshold: 0.0,
            limit: 0,
            include_content: false,
        },
        true,
    )
    .await
    .expect_err("expected unavailable");

    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::Unavailable);

    let _ = shutdown.send(());
}

/// `nodespace skill guidance` is dispatched specially in `lib.rs::run` (the
/// one `skill` subcommand that opens a `NodeClient`, unlike
/// install/uninstall/status). This proves that wiring reaches the real
/// `NodeService.GetSkillGuidance` RPC end to end: a task is matched by
/// meaning, so without an embedding model the fetch reports unavailable
/// rather than an empty result. The success path is covered with the real
/// model by `skill_guidance_fetches_skills_and_schemas_end_to_end` below.
#[tokio::test]
async fn skill_guidance_without_embedding_service_reports_unavailable() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::skill::run_guidance(
        &mut client,
        commands::skill::GuidanceArgs {
            query: "write an ADR".into(),
            limit: 3,
        },
        true,
    )
    .await
    .expect_err("expected unavailable");

    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::Unavailable);

    let _ = shutdown.send(());
}

/// `nodespace skill guidance` with no task lists every skill by name and
/// description, with no instructions and no schemas. It ranks nothing, so it
/// answers with no embedding model: the fallback an agent has when a fetch
/// by task is unavailable.
#[tokio::test]
async fn skill_guidance_with_no_task_lists_every_skill_without_a_model() {
    let (sock, shutdown, _tempdir, _node_service) = spawn_test_daemon_with_seeded_skills().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    for query in ["", "*"] {
        let response = client
            .get_skill_guidance(SkillGuidanceRequest {
                query: query.to_string(),
                limit: 3,
            })
            .await
            .expect("a listing needs no embedding model")
            .into_inner();

        let mut expected: Vec<(&str, &str)> = nodespace_agent::skill_pipeline::SKILL_SEEDS
            .iter()
            .map(|seed| (seed.title, seed.use_for))
            .collect();
        expected.sort();
        let listed: Vec<(&str, &str)> = response
            .skills
            .iter()
            .map(|s| (s.name.as_str(), s.use_for.as_str()))
            .collect();
        assert_eq!(
            listed, expected,
            "every skill, by name, whatever `limit` says"
        );
        assert!(
            response.skills.iter().all(|s| s.instructions.is_empty()),
            "a listing carries no instructions"
        );
        assert!(
            response.skills.iter().all(|s| s.confidence.is_none()),
            "a listing ranks nothing"
        );
        assert!(response.schemas.is_empty(), "a listing carries no schemas");
    }

    // The same through the command handler, which must not fail either.
    commands::skill::run_guidance(
        &mut client,
        commands::skill::GuidanceArgs {
            query: String::new(),
            limit: 3,
        },
        true,
    )
    .await
    .expect("listing through the command handler");

    let _ = shutdown.send(());
}

/// `nodespace skill get` drives `NodeService.GetSkill` end to end, with no
/// embedding model: one skill by its exact name or its id, in the form a
/// match returns it, with the commands of the tools it lists. A name no skill
/// has is an error naming it.
#[tokio::test]
async fn skill_get_fetches_one_skill_by_name_or_id_with_its_tool_commands() {
    let (sock, shutdown, _tempdir, _node_service) = spawn_test_daemon_with_seeded_skills().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let seed = nodespace_agent::skill_pipeline::SKILL_SEEDS
        .iter()
        .find(|s| s.title == "Node Deletion")
        .expect("Node Deletion is seeded");

    for name_or_id in [seed.title, seed.id] {
        let response = client
            .get_skill(GetSkillRequest {
                name_or_id: name_or_id.to_string(),
            })
            .await
            .expect("a fetch by name needs no embedding model")
            .into_inner();

        assert_eq!(response.skills.len(), 1, "{name_or_id}");
        let skill = &response.skills[0];
        assert_eq!(skill.id, seed.id);
        assert_eq!(skill.name, seed.title);
        // An untouched built-in skill is served in its CLI form, as a match
        // serves it.
        assert_eq!(skill.instructions, seed.external_body());
        assert!(skill.confidence.is_none(), "a fetch by name ranks nothing");
        let commands: Vec<(&str, &str)> = skill
            .tool_commands
            .iter()
            .map(|c| (c.tool.as_str(), c.command.as_str()))
            .collect();
        assert_eq!(
            commands,
            [
                ("delete_node", "nodespace node delete"),
                ("get_node", "nodespace node get"),
                ("search_nodes", "nodespace query"),
                ("search_semantic", "nodespace search"),
            ],
            "the commands of the tools Node Deletion lists"
        );
        assert!(response.version.is_empty(), "the version is a listing's");
    }

    let status = client
        .get_skill(GetSkillRequest {
            name_or_id: "Writing a Sonnet".to_string(),
        })
        .await
        .expect_err("no skill has this name");
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(
        status.message().contains("\"Writing a Sonnet\""),
        "{status}"
    );

    // The same through the command handler.
    commands::skill::run_get(
        &mut client,
        commands::skill::GetArgs {
            name_or_id: seed.title.to_string(),
        },
        true,
    )
    .await
    .expect("a fetch by name through the command handler");
    let error = commands::skill::run_get(
        &mut client,
        commands::skill::GetArgs {
            name_or_id: "Writing a Sonnet".to_string(),
        },
        true,
    )
    .await
    .expect_err("an unknown name fails the command");
    assert!(
        error.to_string().contains("\"Writing a Sonnet\""),
        "{error}"
    );

    let _ = shutdown.send(());
}

/// A listing carries the skill list's version: the same from one listing to
/// the next, and another once a skill changes.
#[tokio::test]
async fn skill_listing_carries_a_version_that_changes_with_the_skills() {
    let (sock, shutdown, _tempdir, node_service) = spawn_test_daemon_with_seeded_skills().await;
    let client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let list_version = || {
        let mut client = client.clone();
        async move {
            client
                .get_skill_guidance(SkillGuidanceRequest {
                    query: String::new(),
                    limit: 3,
                })
                .await
                .expect("a listing needs no embedding model")
                .into_inner()
                .version
        }
    };

    let first = list_version().await;
    assert!(!first.is_empty(), "a listing carries a version");
    assert_eq!(list_version().await, first, "nothing changed");

    let skill =
        nodespace_core::models::SkillFields::new("How we write.", &[], 3).into_node("House Style");
    node_service
        .create_node(skill)
        .await
        .expect("the skill must create");

    assert_ne!(list_version().await, first, "a skill was added");

    let _ = shutdown.send(());
}

/// A domain of the workspace's own, built from the primitives a user has: a
/// `ticket` type that extends `task`, a `sprint` type, and a skill for each
/// linked to the types it is about.
async fn install_ticket_domain(node_service: &Arc<CoreNodeService>) {
    use nodespace_core::models::{SkillFields, SKILL_APPLIES_TO};
    use nodespace_core::schema::handle_create_schema;
    use nodespace_core::services::{CreateNodeParams, InsertPositionOwned};

    for params in [
        serde_json::json!({
            "name": "Sprint",
            "description": "A fixed period of work",
            "fields": [
                { "name": "start_date", "type": "date" },
                { "name": "end_date", "type": "date" }
            ]
        }),
        serde_json::json!({
            "name": "Ticket",
            "description": "A unit of work planned into a sprint",
            "extends": "task",
            "fields": [{ "name": "severity", "type": "number" }],
            "relationships": [{
                "name": "in_sprint",
                "direction": "out",
                "targetType": "sprint",
                "cardinality": "one",
                "reverseName": "tickets",
                "reverseCardinality": "many"
            }]
        }),
    ] {
        handle_create_schema(node_service, params)
            .await
            .expect("schema must create");
    }

    for (title, description, procedure, schemas) in [
        (
            "Creating a Ticket",
            "Create a ticket, a unit of work with a severity, and plan it into the current sprint.",
            "Create the ticket, then link it to its sprint with in_sprint.",
            &["ticket", "sprint"][..],
        ),
        (
            "Working with Sprints",
            "Start a sprint, add tickets to the current sprint, and review what remains in it.",
            "A sprint has a start date and an end date. Its tickets are the ones linked to it.",
            &["sprint", "ticket"][..],
        ),
    ] {
        let skill = SkillFields::new(description, &[], 3).into_node(title);
        let skill_id = skill.id.clone();
        node_service
            .create_node(skill)
            .await
            .expect("the skill must create");
        node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: procedure.to_string(),
                parent_id: Some(skill_id.clone()),
                position: InsertPositionOwned::End,
                properties: serde_json::json!({}),
                lifecycle_status: None,
            })
            .await
            .expect("the procedure must create");
        for schema_id in schemas {
            node_service
                .create_relationship(
                    &skill_id,
                    SKILL_APPLIES_TO,
                    schema_id,
                    serde_json::json!({}),
                )
                .await
                .expect("the skill must link to its type");
        }
    }
}

/// A daemon serving a database seeded as a real one is — built-in skills,
/// a ticket domain of its own — with a real embedding model behind it and every
/// queued root embedded. `None` when the model is not on disk.
async fn spawn_embedded_daemon() -> Option<(
    PathBuf,
    oneshot::Sender<()>,
    TempDir,
    Arc<CoreNodeService>,
    Arc<nodespace_core::services::NodeEmbeddingService>,
)> {
    use nodespace_core::services::{EmbeddingProcessor, NodeAccessor, NodeEmbeddingService};
    use nodespace_daemon::EmbeddingReady;

    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("test-daemon.sock");
    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("failed to open SqliteStore"),
    );
    let node_service = Arc::new(
        CoreNodeService::new(&mut store)
            .await
            .expect("failed to build NodeService"),
    );

    let mut nlp = EmbeddingService::new(nodespace_nlp_engine::EmbeddingConfig::default())
        .expect("config must validate");
    if nlp.initialize().is_err() || !nlp.is_initialized() {
        eprintln!("SKIP: nomic-embed-text-v1.5 model not found on disk");
        return None;
    }
    let node_accessor: Arc<dyn NodeAccessor> = node_service.clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        Arc::new(nlp),
        store.clone(),
        node_accessor,
        node_service.behaviors().clone(),
    ));

    let groups: Vec<_> = nodespace_agent::skill_pipeline::seed_skill_nodes()
        .iter()
        .chain(nodespace_agent::skill_pipeline::seed_tool_nodes().iter())
        .map(|t| {
            nodespace_core::markdown::prepare_nodes_from_template(t).expect("template must parse")
        })
        .collect();
    node_service
        .seed_nodes_from_templates(groups)
        .await
        .expect("initial seed must succeed");
    install_ticket_domain(&node_service).await;

    // Embed what the write paths queued, as the processor would.
    for id in store
        .get_stale_embedding_root_ids(None, 0, 3)
        .await
        .expect("the queue must read")
    {
        embedding_service
            .embed_root_node(&id)
            .await
            .unwrap_or_else(|e| panic!("queued root {id} must embed: {e}"));
    }

    let scheduler = Arc::new(nodespace_core::services::EmbeddingScheduler::new());
    let processor = Arc::new(
        EmbeddingProcessor::new(
            embedding_service.clone(),
            scheduler.clone(),
            "test-db".to_string(),
        )
        .expect("processor must build"),
    );
    let service = NodeServiceImpl::new(
        node_service.clone(),
        Arc::new(tokio::sync::RwLock::new(Some(EmbeddingReady {
            embedding_service: embedding_service.clone(),
            processor,
        }))),
        scheduler,
    );

    let listener = UnixListener::bind(&sock_path).expect("failed to bind test UDS socket");
    let incoming = UnixListenerStream::new(listener);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect(&sock_path, DatabaseIdInterceptor::none())
            .await
            .is_ok()
        {
            return Some((
                sock_path,
                shutdown_tx,
                tempdir,
                node_service,
                embedding_service,
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// The regression test for the fetch itself: `nodespace skill guidance
/// "<task>"`, over the real transport, against embedded skill nodes.
///
/// Before the fix this returned nothing for any task. The command rode the
/// generic node search, whose default scope drops every `skill` node.
///
/// Ignored by default — loads a real embedding model from the standard
/// NodeSpace catalog path. Run explicitly:
///
/// ```text
/// .tools/bin/cargo-nextest nextest run -p nodespace-cli --test it cli_integration::skill_guidance_fetches --run-ignored all
/// ```
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn skill_guidance_fetches_skills_and_schemas_end_to_end() {
    let Some((sock, shutdown, _tempdir, node_service, embedding_service)) =
        spawn_embedded_daemon().await
    else {
        return;
    };
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let fetch = |query: &str| SkillGuidanceRequest {
        query: query.to_string(),
        limit: 3,
    };

    // A task worded as a built-in skill describes itself returns that skill
    // first, with its procedure written in CLI commands.
    let deletion = client
        .get_skill_guidance(fetch(
            "Delete, remove, erase, purge, discard, trash, drop, or get rid of stored content.",
        ))
        .await
        .expect("a task must fetch")
        .into_inner();
    let first = deletion.skills.first().expect("a skill must match");
    assert_eq!(first.name, "Node Deletion");
    assert!(
        first.instructions.contains("`nodespace node delete <id>`"),
        "a built-in skill is served in CLI commands: {}",
        first.instructions
    );
    assert!(
        !first.instructions.contains("delete_node"),
        "a built-in skill served to an outside agent names no in-app tool: {}",
        first.instructions
    );
    assert!(first.confidence.is_some());

    // A task in an installed domain returns that domain's skills and the
    // schemas of the types it touches.
    let domain = client
        .get_skill_guidance(fetch("add a ticket to the current sprint"))
        .await
        .expect("a task must fetch")
        .into_inner();
    let skills: Vec<&str> = domain.skills.iter().map(|s| s.name.as_str()).collect();
    for expected in ["Creating a Ticket", "Working with Sprints"] {
        assert!(skills.contains(&expected), "{expected:?} not in {skills:?}");
    }
    let schemas: Vec<&str> = domain.schemas.iter().map(|s| s.id.as_str()).collect();
    for expected in ["ticket", "sprint"] {
        assert!(
            schemas.contains(&expected),
            "{expected:?} not in {schemas:?}"
        );
    }
    let ticket: serde_json::Value = serde_json::from_str(
        &domain
            .schemas
            .iter()
            .find(|s| s.id == "ticket")
            .expect("ticket schema")
            .definition,
    )
    .expect("a schema's definition is JSON");
    assert!(ticket["fields"].as_array().is_some_and(|f| !f.is_empty()));
    assert!(ticket["relationships"]
        .as_array()
        .is_some_and(|r| !r.is_empty()));

    // A built-in skill a user has edited is served as they left it.
    let deletion_id = first.id.clone();
    let child = node_service
        .get_children(&deletion_id)
        .await
        .expect("children")
        .into_iter()
        .next()
        .expect("seeded guidance must have a child");
    node_service
        .update_node(
            &child.id,
            child.version,
            nodespace_core::models::NodeUpdate::new()
                .with_content("Our team archives instead of deleting.".to_string()),
        )
        .await
        .expect("a user's edit to seeded guidance must be allowed");
    // The edit leaves the skill's embedding stale, and a stale embedding is
    // out of the index until it is rebuilt. The write marks it stale from a
    // spawned task, so rebuild and look again until that has landed.
    let mut edited_instructions = None;
    for _ in 0..40 {
        embedding_service
            .embed_root_node(&deletion_id)
            .await
            .expect("the edited skill must re-embed");
        let edited = client
            .get_skill_guidance(fetch("delete a node"))
            .await
            .expect("a task must fetch")
            .into_inner();
        if let Some(skill) = edited.skills.into_iter().find(|s| s.id == deletion_id) {
            edited_instructions = Some(skill.instructions);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let edited_instructions = edited_instructions.expect("Node Deletion must still match");
    assert!(
        edited_instructions.contains("Our team archives instead of deleting."),
        "an edited skill is served as stored: {edited_instructions}"
    );
    assert!(
        !edited_instructions.contains("`nodespace node delete <id>`"),
        "an edited skill is not replaced by the seed's text: {edited_instructions}"
    );

    // And the command handler prints all of it, in both modes.
    for json in [false, true] {
        commands::skill::run_guidance(
            &mut client,
            commands::skill::GuidanceArgs {
                query: "add a ticket to the current sprint".into(),
                limit: 3,
            },
            json,
        )
        .await
        .expect("the command handler must print a fetch");
    }

    // A skill a user wrote names a built-in tool, which an outside agent
    // cannot call. The fetch hands back the `nodespace` command for it, and
    // running that command does what the step asks.
    use nodespace_core::services::{CreateNodeParams, InsertPositionOwned};
    let decision_skill = nodespace_core::models::SkillFields::new(
        "Record a decision the team made and link it to the task it settles.",
        &[],
        3,
    )
    .into_node("Logging a Team Decision");
    let decision_skill_id = decision_skill.id.clone();
    node_service
        .create_node(decision_skill)
        .await
        .expect("the user's skill must create");
    node_service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Find the task with `search_nodes`, then link the decision to it with \
                      create_relationship. Never call update_nodes_from_markdown."
                .to_string(),
            parent_id: Some(decision_skill_id.clone()),
            position: InsertPositionOwned::End,
            properties: serde_json::json!({}),
            lifecycle_status: None,
        })
        .await
        .expect("the user's procedure must create");

    let mut written = None;
    for _ in 0..40 {
        embedding_service
            .embed_root_node(&decision_skill_id)
            .await
            .expect("the user's skill must embed");
        let fetched = client
            .get_skill_guidance(fetch(
                "record a decision the team made and link it to the task it settles",
            ))
            .await
            .expect("a task must fetch")
            .into_inner();
        if let Some(skill) = fetched
            .skills
            .into_iter()
            .find(|s| s.id == decision_skill_id)
        {
            written = Some(skill);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let written = written.expect("the user's skill must match its own description");
    assert!(
        written.instructions.contains("create_relationship"),
        "a user's skill is served as written: {}",
        written.instructions
    );
    let named: Vec<(&str, &str)> = written
        .tool_commands
        .iter()
        .map(|c| (c.tool.as_str(), c.command.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("create_relationship", "nodespace relationship create"),
            ("search_nodes", "nodespace query"),
        ],
        "the tools the body names, each with its command, and no tool it does not name"
    );

    // Run the command the fetch returned for `create_relationship`.
    let mut ends = Vec::new();
    for content in ["Decision: ship on Friday", "Task: prepare the release"] {
        let node = nodespace_core::models::Node::new(
            "text".to_string(),
            content.to_string(),
            serde_json::json!({}),
        );
        ends.push(node.id.clone());
        node_service
            .create_node(node)
            .await
            .expect("an end of the link must create");
    }
    let mut argv: Vec<&str> = written.tool_commands[0].command.split(' ').collect();
    argv.extend([
        "--from",
        ends[0].as_str(),
        "--type",
        "mentions",
        "--to",
        ends[1].as_str(),
    ]);
    use clap::Parser;
    let cli = nodespace_cli::Cli::try_parse_from(&argv)
        .unwrap_or_else(|e| panic!("the returned command must be one the CLI has: {e}"));
    let nodespace_cli::Command::Relationship { action } = cli.command else {
        panic!("the returned command is not the relationship command: {argv:?}");
    };
    commands::relationship::run(&mut client, action, true)
        .await
        .expect("the returned command must run");
    let linked = node_service
        .store()
        .get_edge_targets_by_source(std::slice::from_ref(&ends[0]), "mentions")
        .await
        .expect("the edges must read");
    assert_eq!(
        linked.get(&ends[0]),
        Some(&vec![ends[1].clone()]),
        "running the command made the link the step asks for"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn diagnostics_collect_reports_counts_and_recency() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;
    let mut node = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect node");
    let mut seed = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect seed");
    let mut db = connect_database(&sock).await.expect("connect database");

    let baseline = commands::diagnostics::collect(&mut node, &mut db, None)
        .await
        .expect("no database is refused");
    assert!(
        baseline.errors.is_empty(),
        "baseline collect must not produce errors: {:?}",
        baseline.errors
    );
    assert!(
        baseline.total_node_count.is_some(),
        "a successful query must report a count, not unknown"
    );
    // The registry has exactly the seeded default, and it is the target when no
    // database is selected.
    assert_eq!(baseline.databases.len(), 1, "one registered database");
    assert!(baseline.databases[0].is_default);
    assert_eq!(baseline.targeted_database_id, baseline.databases[0].id);

    let root = seed
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "root".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed root")
        .into_inner();

    let mut last_child_id = String::new();
    for label in ["child-1", "child-2"] {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last_child_id = seed
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: label.into(),
                parent_id: Some(root.node_id.clone()),
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .unwrap_or_else(|e| panic!("seed {label}: {e}"))
            .into_inner()
            .node_id;
    }

    let report = commands::diagnostics::collect(&mut node, &mut db, None)
        .await
        .expect("no database is refused");
    assert_eq!(
        report.total_node_count,
        baseline.total_node_count.map(|n| n + 3),
        "expected three additional nodes vs baseline"
    );
    assert_eq!(
        report.root_node_count,
        baseline.root_node_count.map(|n| n + 1),
        "expected one additional root node vs baseline"
    );
    assert!(
        report.database_size_bytes.unwrap_or(0) > 0,
        "targeted database file should have a nonzero size after writes"
    );
    // The daemon under test runs in-process, so this is the test binary's own
    // RSS. Assert only what holds for any live process — a reading exists and
    // is nonzero — never a specific figure.
    assert!(
        report.daemon_rss_bytes.unwrap_or(0) > 0,
        "a running daemon must report a nonzero RSS, got {:?}",
        report.daemon_rss_bytes
    );
    assert_eq!(
        report
            .recent_node_ids
            .as_ref()
            .and_then(|ids| ids.first())
            .map(String::as_str),
        Some(last_child_id.as_str())
    );
    assert!(
        report.errors.is_empty(),
        "happy-path collect must not surface errors: {:?}",
        report.errors
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn connect_refused_returns_friendly_error() {
    let err = connect(
        std::path::Path::new("/tmp/nodespace-no-such-daemon.sock"),
        DatabaseIdInterceptor::none(),
    )
    .await
    .expect_err("expected refusal");

    let msg = format!("{}", err);
    assert!(
        msg.contains("Could not connect to nodespaced"),
        "expected friendly error, got: {msg}"
    );
    assert!(
        msg.contains("Is the daemon running?"),
        "expected remediation hint, got: {msg}"
    );
}

#[tokio::test]
async fn node_query_by_type() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    raw.create_node(CreateNodeRequest {
        node_type: "task".into(),
        content: "do the thing".into(),
        parent_id: None,
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed task");

    raw.create_node(CreateNodeRequest {
        node_type: "text".into(),
        content: "some text".into(),
        parent_id: None,
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed text");

    node_run(
        &mut client,
        commands::node::NodeAction::Query(commands::node::QueryArgs {
            id: None,
            mentioned_by: None,
            content_contains: None,
            title_contains: None,
            node_type: Some("task".into()),
            limit: 0,
            offset: 0,
        }),
        true,
    )
    .await
    .expect("query by type");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_export_markdown() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let root = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "# Root Document".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed root")
        .into_inner();

    raw.create_node(CreateNodeRequest {
        node_type: "text".into(),
        content: "Child paragraph".into(),
        parent_id: Some(root.node_id.clone()),
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed child");

    node_run(
        &mut client,
        commands::node::NodeAction::Export(commands::node::ExportArgs {
            id: root.node_id.clone(),
            children: true,
            max_depth: 0,
            node_ids: false,
        }),
        true,
    )
    .await
    .expect("export markdown");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_batch_get_and_update() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let a = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "node-a".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed a")
        .into_inner()
        .node_id;

    let b = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "node-b".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed b")
        .into_inner()
        .node_id;

    // batch-get: both found, one missing
    node_run(
        &mut client,
        commands::node::NodeAction::BatchGet(commands::node::BatchGetArgs {
            ids: vec![a.clone(), b.clone(), "does-not-exist".into()],
        }),
        true,
    )
    .await
    .expect("batch-get");

    // batch-update (auto-version)
    let updates_json = serde_json::json!([
        {"node_id": a, "content": "node-a updated"},
        {"node_id": b, "content": "node-b updated"},
    ])
    .to_string();

    node_run(
        &mut client,
        commands::node::NodeAction::BatchUpdate(commands::node::BatchUpdateArgs {
            updates: updates_json,
        }),
        true,
    )
    .await
    .expect("batch-update");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn mention_create_query_delete() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let source = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "source node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed source")
        .into_inner()
        .node_id;

    let target = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "target node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed target")
        .into_inner()
        .node_id;

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Create(commands::mention::CreateMentionArgs {
            from: source.clone(),
            to: target.clone(),
        }),
        true,
    )
    .await
    .expect("create mention");

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Outgoing(commands::mention::MentionQueryArgs {
            id: source.clone(),
        }),
        true,
    )
    .await
    .expect("outgoing mentions");

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Incoming(commands::mention::MentionQueryArgs {
            id: target.clone(),
        }),
        true,
    )
    .await
    .expect("incoming mentions");

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Delete(commands::mention::DeleteMentionArgs {
            from: source.clone(),
            to: target.clone(),
        }),
        true,
    )
    .await
    .expect("delete mention");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_list_and_get() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // The test daemon has no custom schemas; list should return an empty result without error.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::List(commands::schema::SchemaListArgs {}),
        true,
    )
    .await
    .expect("schema list");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_create_and_update_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Invoice",
                    "fields": [
                        {"name": "amount", "type": "number"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    // Fetch the created schema back via the existing read path to confirm it landed.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Get(commands::schema::SchemaGetArgs {
            id: "invoice".into(),
        }),
        true,
    )
    .await
    .expect("schema get after create");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "schema_id": "invoice",
                    "add_fields": [
                        {"name": "currency", "type": "text"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema update");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_delete_requires_relationship_declarations_removed_first() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // A self-referential declaration is enough to trip the guard, and needs
    // only one schema to set up.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Memo",
                    "fields": [{"name": "body", "type": "text"}],
                    "relationships": [{
                        "name": "supersedes",
                        "targetType": "memo",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "superseded_by",
                        "reverseCardinality": "one"
                    }]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    // The declaration blocks the delete, and the rejection names the fix.
    let err = commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Delete(commands::schema::SchemaDeleteArgs {
            id: "memo".into(),
        }),
        true,
    )
    .await
    .expect_err("delete must be refused while a declaration remains");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("schema_has_declarations"),
        "rejection should name the guard: {msg}"
    );
    assert!(
        msg.contains("update_schema"),
        "rejection should name the fix: {msg}"
    );
    // The guidance tells agents to act on the count, so the count has to be
    // in the message. A self-reference is one stored edge, not two, even
    // though it touches this schema at both ends.
    assert!(
        msg.contains("1 relationship declaration(s)"),
        "rejection should name how many declarations remain: {msg}"
    );

    // Clearing the declaration unblocks it.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "schema_id": "memo",
                    "remove_relationships": ["supersedes"]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema update removing the declaration");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Delete(commands::schema::SchemaDeleteArgs {
            id: "memo".into(),
        }),
        true,
    )
    .await
    .expect("schema delete after clearing declarations");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Get(commands::schema::SchemaGetArgs { id: "memo".into() }),
        true,
    )
    .await
    .expect_err("the deleted schema must no longer resolve");

    let _ = shutdown.send(());
}

/// `schema delete` is one step, so it must not reach an ordinary node — that
/// would be a way around `node delete`'s preview (ADR-080).
#[tokio::test]
async fn schema_delete_refuses_a_node_that_is_not_a_schema() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let node_id = client
        .create_node(nodespace_daemon::nodespace::CreateNodeRequest {
            node_type: "text".into(),
            content: "not a schema".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner()
        .node_id;

    let err = commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Delete(commands::schema::SchemaDeleteArgs {
            id: node_id.clone(),
        }),
        true,
    )
    .await
    .expect_err("schema delete must refuse a text node");
    assert!(
        format!("{err:#}").contains("not a schema"),
        "refusal should say why: {err:#}"
    );
    client
        .get_node(GetNodeRequest { node_id })
        .await
        .expect("the refused node must still exist");

    let _ = shutdown.send(());
}

/// The two delete flags only make sense together: either alone is a parse
/// error, not a half-confirmed delete.
#[test]
fn node_delete_confirmation_flags_are_required_together() {
    use clap::Parser;
    let parse = |args: &[&str]| nodespace_cli::Cli::try_parse_from(args);

    assert!(parse(&["nodespace", "node", "delete", "abc"]).is_ok());
    assert!(parse(&[
        "nodespace",
        "node",
        "delete",
        "abc",
        "--version",
        "3",
        "--descendants",
        "0"
    ])
    .is_ok());
    assert!(parse(&["nodespace", "node", "delete", "abc", "--version", "3"]).is_err());
    assert!(parse(&["nodespace", "node", "delete", "abc", "--descendants", "0"]).is_err());
}

/// `node move` takes one destination and one position at a time.
#[test]
fn node_move_destination_and_position_flags_are_exclusive() {
    use clap::Parser;
    let parse = |args: &[&str]| nodespace_cli::Cli::try_parse_from(args);

    assert!(parse(&["nodespace", "node", "move", "a", "--parent", "p"]).is_ok());
    assert!(parse(&["nodespace", "node", "move", "a", "--root"]).is_ok());
    assert!(parse(&["nodespace", "node", "move", "a", "--after", "b"]).is_ok());
    assert!(parse(&["nodespace", "node", "move", "a", "--parent", "p", "--root"]).is_err());
    assert!(parse(&["nodespace", "node", "move", "a", "--first", "--after", "b"]).is_err());
    // A root has no sibling order, so `--root` takes no position.
    assert!(parse(&["nodespace", "node", "move", "a", "--root", "--first"]).is_err());
    assert!(parse(&["nodespace", "node", "move", "a", "--root", "--after", "b"]).is_err());
    // An empty ID (an unset shell variable) is not a request for the root.
    assert!(parse(&["nodespace", "node", "move", "a", "--parent", ""]).is_err());
    assert!(parse(&["nodespace", "node", "move", "a", "--after", ""]).is_err());
}

/// Create a text node through the raw client and return its ID.
async fn seed_text_node(raw: &mut NodeClient, content: &str, parent: Option<&str>) -> String {
    raw.create_node(CreateNodeRequest {
        node_type: "text".into(),
        content: content.into(),
        parent_id: parent.map(str::to_string),
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed text node")
    .into_inner()
    .node_id
}

/// The IDs of `parent`'s children, in sibling order.
async fn child_ids(raw: &mut NodeClient, parent: &str) -> Vec<String> {
    raw.get_children(nodespace_daemon::nodespace::GetChildrenRequest {
        node_id: parent.to_string(),
    })
    .await
    .expect("get children")
    .into_inner()
    .nodes
    .into_iter()
    .map(|n| n.id)
    .collect()
}

/// The version `id` is at now.
async fn node_version(raw: &mut NodeClient, id: &str) -> i64 {
    raw.get_node(GetNodeRequest {
        node_id: id.to_string(),
    })
    .await
    .expect("get node")
    .into_inner()
    .node_data
    .expect("node_data")
    .version
}

fn move_args(id: &str) -> commands::node::MoveArgs {
    commands::node::MoveArgs {
        id: id.to_string(),
        parent: None,
        root: false,
        first: false,
        after: None,
        version: None,
    }
}

async fn run_move(client: &mut NodeClient, args: commands::node::MoveArgs) -> anyhow::Result<()> {
    node_run(client, commands::node::NodeAction::Move(args), true).await
}

#[tokio::test]
async fn node_move_reparents_a_node_at_the_requested_position() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let old_parent = seed_text_node(&mut raw, "old parent", None).await;
    let new_parent = seed_text_node(&mut raw, "new parent", None).await;
    let a = seed_text_node(&mut raw, "a", Some(&new_parent)).await;
    let b = seed_text_node(&mut raw, "b", Some(&new_parent)).await;
    let last = seed_text_node(&mut raw, "last", Some(&old_parent)).await;
    let first = seed_text_node(&mut raw, "first", Some(&old_parent)).await;
    let middle = seed_text_node(&mut raw, "middle", Some(&old_parent)).await;

    // No position: last under the new parent.
    run_move(
        &mut client,
        commands::node::MoveArgs {
            parent: Some(new_parent.clone()),
            ..move_args(&last)
        },
    )
    .await
    .expect("move to the end");
    run_move(
        &mut client,
        commands::node::MoveArgs {
            parent: Some(new_parent.clone()),
            first: true,
            ..move_args(&first)
        },
    )
    .await
    .expect("move to the beginning");
    run_move(
        &mut client,
        commands::node::MoveArgs {
            parent: Some(new_parent.clone()),
            after: Some(a.clone()),
            ..move_args(&middle)
        },
    )
    .await
    .expect("move after a sibling");

    assert_eq!(
        child_ids(&mut raw, &new_parent).await,
        [&first, &a, &middle, &b, &last].map(String::clone)
    );
    assert!(child_ids(&mut raw, &old_parent).await.is_empty());

    // `--root` detaches the node from its parent.
    run_move(
        &mut client,
        commands::node::MoveArgs {
            root: true,
            ..move_args(&middle)
        },
    )
    .await
    .expect("move to root");
    assert_eq!(
        child_ids(&mut raw, &new_parent).await,
        [&first, &a, &b, &last].map(String::clone)
    );

    shutdown.send(()).ok();
}

#[tokio::test]
async fn node_move_without_a_parent_reorders_among_siblings() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let parent = seed_text_node(&mut raw, "parent", None).await;
    let a = seed_text_node(&mut raw, "a", Some(&parent)).await;
    let b = seed_text_node(&mut raw, "b", Some(&parent)).await;
    let c = seed_text_node(&mut raw, "c", Some(&parent)).await;

    run_move(
        &mut client,
        commands::node::MoveArgs {
            first: true,
            ..move_args(&c)
        },
    )
    .await
    .expect("reorder to the beginning");
    assert_eq!(
        child_ids(&mut raw, &parent).await,
        [&c, &a, &b].map(String::clone)
    );

    // After a sibling that is not the last one, so "last" is a wrong answer.
    let version_before = node_version(&mut raw, &c).await;
    run_move(
        &mut client,
        commands::node::MoveArgs {
            after: Some(a.clone()),
            ..move_args(&c)
        },
    )
    .await
    .expect("reorder after a sibling");
    assert_eq!(
        child_ids(&mut raw, &parent).await,
        [&a, &c, &b].map(String::clone)
    );
    assert!(node_version(&mut raw, &c).await > version_before);

    // A root has no siblings to be ordered among.
    run_move(
        &mut client,
        commands::node::MoveArgs {
            first: true,
            ..move_args(&parent)
        },
    )
    .await
    .expect_err("reordering a root must be refused");

    // Neither a destination nor a position: nothing to do, so it is refused
    // before any RPC.
    let err = run_move(&mut client, move_args(&c))
        .await
        .expect_err("a move with no destination or position must fail");
    assert!(err.to_string().contains("--parent"), "got: {err}");

    shutdown.send(()).ok();
}

#[tokio::test]
async fn node_move_at_a_stale_version_writes_nothing() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let old_parent = seed_text_node(&mut raw, "old parent", None).await;
    let new_parent = seed_text_node(&mut raw, "new parent", None).await;
    let node = seed_text_node(&mut raw, "node", Some(&old_parent)).await;
    let sibling = seed_text_node(&mut raw, "sibling", Some(&old_parent)).await;
    let version = node_version(&mut raw, &node).await;

    for args in [
        commands::node::MoveArgs {
            parent: Some(new_parent.clone()),
            version: Some(version + 1),
            ..move_args(&node)
        },
        commands::node::MoveArgs {
            after: Some(sibling.clone()),
            version: Some(version + 1),
            ..move_args(&node)
        },
    ] {
        let err = run_move(&mut client, args)
            .await
            .expect_err("a stale version must be refused");
        assert!(
            err.to_string().contains("has changed since it was read"),
            "got: {err:#}"
        );
    }
    assert_eq!(
        child_ids(&mut raw, &old_parent).await,
        [&node, &sibling].map(String::clone)
    );

    // The version it is at moves it.
    run_move(
        &mut client,
        commands::node::MoveArgs {
            parent: Some(new_parent.clone()),
            version: Some(version),
            ..move_args(&node)
        },
    )
    .await
    .expect("move at the current version");
    assert_eq!(child_ids(&mut raw, &new_parent).await, [node]);

    shutdown.send(()).ok();
}

/// `--after` names a child of the parent the node ends up under. Anything
/// else is refused, not quietly read as "last".
#[tokio::test]
async fn node_move_after_a_node_that_is_not_a_sibling_writes_nothing() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let old_parent = seed_text_node(&mut raw, "old parent", None).await;
    let new_parent = seed_text_node(&mut raw, "new parent", None).await;
    let first = seed_text_node(&mut raw, "first", Some(&old_parent)).await;
    let node = seed_text_node(&mut raw, "node", Some(&old_parent)).await;
    let last = seed_text_node(&mut raw, "last", Some(&old_parent)).await;
    let elsewhere = seed_text_node(&mut raw, "elsewhere", Some(&new_parent)).await;
    let version = node_version(&mut raw, &node).await;

    let missing = "no-such-node".to_string();
    for (parent, after, refusal) in [
        // Move: a child of the parent the node is leaving, a node that does
        // not exist, and the node itself, under a new parent and under the
        // one it already has.
        (Some(&new_parent), &first, "is not a child of"),
        (Some(&new_parent), &missing, "is not a child of"),
        (Some(&new_parent), &node, "after itself"),
        (Some(&old_parent), &node, "after itself"),
        // Reorder: a child of another parent, a node that does not exist,
        // and the node itself.
        (None, &elsewhere, "is not a child of"),
        (None, &missing, "does not exist"),
        (None, &node, "after itself"),
    ] {
        let err = run_move(
            &mut client,
            commands::node::MoveArgs {
                parent: parent.cloned(),
                after: Some(after.clone()),
                ..move_args(&node)
            },
        )
        .await
        .expect_err("a position after a non-sibling must be refused");
        let message = format!("{err:#}");
        assert!(message.contains(refusal), "got: {message}");
    }

    assert_eq!(
        child_ids(&mut raw, &old_parent).await,
        [&first, &node, &last].map(String::clone)
    );
    assert_eq!(child_ids(&mut raw, &new_parent).await, [elsewhere]);
    assert_eq!(node_version(&mut raw, &node).await, version);

    shutdown.send(()).ok();
}

/// A second `has_child` edge is refused with the command that moves the node,
/// and that command, run as printed, does it.
#[tokio::test]
async fn second_parent_edge_is_refused_with_the_move_command() {
    use clap::Parser;

    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let old_parent = seed_text_node(&mut raw, "old parent", None).await;
    let new_parent = seed_text_node(&mut raw, "new parent", None).await;
    let node = seed_text_node(&mut raw, "node", Some(&old_parent)).await;

    let err = commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: new_parent.clone(),
            relationship_name: "has_child".into(),
            to: node.clone(),
            edge_data: None,
        }),
        true,
    )
    .await
    .expect_err("a second parent edge must be refused");
    let message = format!("{err:#}");
    let command = format!("nodespace node move {node} --parent {new_parent}");
    assert!(message.contains(&command), "got: {message}");
    assert_eq!(
        child_ids(&mut raw, &old_parent).await,
        std::slice::from_ref(&node)
    );

    let cli = nodespace_cli::Cli::try_parse_from(command.split(' ')).expect("the command parses");
    let nodespace_cli::Command::Node { action } = cli.command else {
        panic!("the refusal's command is not a node command");
    };
    node_run(&mut client, action, true)
        .await
        .expect("the refusal's command moves the node");
    assert_eq!(child_ids(&mut raw, &new_parent).await, [node]);

    shutdown.send(()).ok();
}

/// Seed two `person` nodes sharing the same (case-insensitively) unique
/// `email`, which trips the create-path `detect_unique_field_collisions`
/// hook and journals an open `UniqueFieldCollision` record naming both. The
/// `person` type's `unique_case_insensitive` email field is a system-seeded
/// schema (`NodeService::new` seeds it), so no explicit `create_schema` call
/// is needed — mirrors `person_duplicate_convergence_test.rs`'s fixture.
///
/// `email_local_part` must be distinct per call within a test: the field is
/// case-insensitively unique across every active `person` node in the store,
/// so reusing a value across two calls would make the second pair collide
/// with the first pair's nodes too, not just with each other.
async fn seed_colliding_people(raw: &mut NodeClient, email_local_part: &str) -> (String, String) {
    let email = format!("{email_local_part}@example.com");
    let alice = raw
        .create_node(CreateNodeRequest {
            node_type: "person".into(),
            content: String::new(),
            parent_id: None,
            properties: serde_json::json!({"person": {"email": email}}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed alice")
        .into_inner()
        .node_id;

    let bob = raw
        .create_node(CreateNodeRequest {
            node_type: "person".into(),
            content: String::new(),
            parent_id: None,
            properties: serde_json::json!({"person": {"email": email.to_uppercase()}}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed bob (colliding email)")
        .into_inner()
        .node_id;

    (alice, bob)
}

#[tokio::test]
async fn conflicts_list_show_and_dismiss_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let (alice, _bob) = seed_colliding_people(&mut raw, "list-show-dismiss").await;

    // list --node finds the journaled collision.
    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::List(commands::conflicts::ListArgs {
            status: None,
            kind: None,
            node: Some(alice.clone()),
            limit: None,
        }),
        true,
    )
    .await
    .expect("conflicts list --node");

    let conflicts = raw
        .conflicts_for_node(ConflictsForNodeRequest {
            node_id: alice.clone(),
        })
        .await
        .expect("raw conflicts_for_node")
        .into_inner()
        .conflicts;
    let open = conflicts
        .iter()
        .find(|c| c.kind == "unique_field_collision" && c.status == "open")
        .expect("the colliding email must have journaled a conflict");
    let conflict_id = open.id.clone();

    // show fetches the same record by its own id.
    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Show(commands::conflicts::ShowArgs {
            conflict_id: conflict_id.clone(),
        }),
        true,
    )
    .await
    .expect("conflicts show");

    // dismiss resolves it as Dismissed.
    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Dismiss(commands::conflicts::DismissArgs {
            conflict_id: conflict_id.clone(),
        }),
        true,
    )
    .await
    .expect("conflicts dismiss");

    let after = raw
        .get_conflict(GetConflictRequest {
            conflict_id: conflict_id.clone(),
        })
        .await
        .expect("raw get_conflict after dismiss")
        .into_inner()
        .conflict
        .expect("record must still exist");
    assert_eq!(after.status, "dismissed");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn conflicts_show_missing_id_surfaces_a_clear_error() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Show(commands::conflicts::ShowArgs {
            conflict_id: "does-not-exist".into(),
        }),
        false,
    )
    .await
    .expect_err("expected error for an unknown conflict id");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("does-not-exist"),
        "error should name the missing id: {msg}"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn conflicts_adopt_and_merge_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // adopt, on its own pair.
    let (alice, _bob) = seed_colliding_people(&mut raw, "adopt-pair").await;
    let conflict = raw
        .conflicts_for_node(ConflictsForNodeRequest {
            node_id: alice.clone(),
        })
        .await
        .expect("raw conflicts_for_node")
        .into_inner()
        .conflicts
        .into_iter()
        .find(|c| c.kind == "unique_field_collision" && c.status == "open")
        .expect("journaled collision");

    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Adopt(commands::conflicts::AdoptArgs {
            conflict_id: conflict.id.clone(),
            keep: alice.clone(),
        }),
        true,
    )
    .await
    .expect("conflicts adopt");

    let after = raw
        .get_conflict(GetConflictRequest {
            conflict_id: conflict.id,
        })
        .await
        .expect("raw get_conflict after adopt")
        .into_inner()
        .conflict
        .expect("record must still exist");
    assert_eq!(after.status, "resolved");

    // merge, on a fresh pair with --conflict-id inferring the loser.
    let (carol, dave) = seed_colliding_people(&mut raw, "merge-pair").await;
    let merge_conflict = raw
        .conflicts_for_node(ConflictsForNodeRequest {
            node_id: carol.clone(),
        })
        .await
        .expect("raw conflicts_for_node")
        .into_inner()
        .conflicts
        .into_iter()
        .find(|c| c.kind == "unique_field_collision" && c.status == "open")
        .expect("journaled collision");

    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Merge(commands::conflicts::MergeArgs {
            survivor: carol.clone(),
            loser: None,
            conflict_id: Some(merge_conflict.id.clone()),
        }),
        true,
    )
    .await
    .expect("conflicts merge (loser inferred from conflict_id)");

    let loser_after = raw
        .get_node(GetNodeRequest {
            node_id: dave.clone(),
        })
        .await
        .expect("raw get_node on merged-away loser")
        .into_inner()
        .node_data
        .expect("loser row must still exist, archived");
    assert_eq!(loser_after.lifecycle_status, "archived");

    let closed = raw
        .get_conflict(GetConflictRequest {
            conflict_id: merge_conflict.id,
        })
        .await
        .expect("raw get_conflict after merge")
        .into_inner()
        .conflict
        .expect("record must still exist");
    assert_eq!(closed.status, "resolved");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_create_rejects_malformed_params() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some("not json".into()),
            params_file: None,
        }),
        true,
    )
    .await
    .expect_err("malformed params_json should error");
    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("params_json"),
        "expected status message to name the offending field, got: {}",
        status.message()
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn execute_query_filters_by_property() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    raw.create_node(CreateNodeRequest {
        node_type: "task".into(),
        content: "task one".into(),
        parent_id: None,
        properties: serde_json::json!({"status": "open"}).to_string(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed open task");

    raw.create_node(CreateNodeRequest {
        node_type: "task".into(),
        content: "task two".into(),
        parent_id: None,
        properties: serde_json::json!({"status": "done"}).to_string(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed done task");

    commands::query::run(
        &mut client,
        commands::query::QueryArgs {
            command: None,
            target_type: Some("task".into()),
            filters: Some(
                serde_json::json!([
                    {"type": "property", "operator": "equals", "property": "status", "value": "open"}
                ])
                .to_string(),
            ),
            sorting: None,
            limit: 0,
        },
        true,
    )
    .await
    .expect("execute query");

    let _ = shutdown.send(());
}

/// `query run` takes a saved query's id or title and optional narrowing;
/// without the subcommand, `query` still needs `--type`.
#[test]
fn query_run_parses_beside_the_inline_query() {
    use clap::Parser;
    let parse = |args: &[&str]| nodespace_cli::Cli::try_parse_from(args);

    assert!(parse(&["nodespace", "query", "--type", "task"]).is_ok());
    assert!(parse(&["nodespace", "query"]).is_err());
    assert!(parse(&["nodespace", "query", "run"]).is_err());
    assert!(parse(&["nodespace", "query", "run", "Ready tasks"]).is_ok());
    assert!(parse(&["nodespace", "--json", "query", "run", "Ready tasks"]).is_ok());
    assert!(parse(&[
        "nodespace",
        "query",
        "run",
        "Ready tasks",
        "--filters",
        "[]",
        "--limit",
        "1"
    ])
    .is_ok());
    // A saved query carries its own type and sorting.
    assert!(parse(&["nodespace", "query", "run", "Ready tasks", "--type", "task"]).is_err());
    assert!(parse(&[
        "nodespace",
        "query",
        "run",
        "Ready tasks",
        "--sorting",
        "[]"
    ])
    .is_err());
}

/// `query run` returns what the saved query matches, by id and by title,
/// narrowed by run-time filters, and says which when a title names no query
/// or several.
#[tokio::test]
async fn query_run_executes_a_saved_query_by_id_or_title() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let create = |node_type: &str, content: &str, properties: serde_json::Value| {
        let request = CreateNodeRequest {
            node_type: node_type.into(),
            content: content.into(),
            parent_id: None,
            properties: properties.to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        };
        let mut raw = raw.clone();
        async move {
            raw.create_node(request)
                .await
                .expect("create node")
                .into_inner()
                .node_id
        }
    };
    create(
        "task",
        "open low",
        serde_json::json!({"status": "open", "priority": "low"}),
    )
    .await;
    create(
        "task",
        "open high",
        serde_json::json!({"status": "open", "priority": "high"}),
    )
    .await;
    create("task", "done", serde_json::json!({"status": "done"})).await;
    let not_done = serde_json::json!({
        "target_type": "task",
        "filters": [{
            "type": "property", "operator": "equals", "property": "status",
            "value": "done", "negate": true
        }],
        "sorting": [{"field": "priority", "direction": "asc"}]
    });
    let query_id = create("query", "Not done", not_done.clone()).await;
    create("query", "Twin", not_done.clone()).await;
    create("query", "twin", not_done).await;

    let contents = |query: &str, filters: Option<serde_json::Value>, limit: u32| {
        let request = RunSavedQueryRequest {
            query: query.into(),
            filters_json: filters.map(|f| f.to_string()),
            limit,
            with_context: false,
        };
        let mut raw = raw.clone();
        async move {
            raw.run_saved_query(request).await.map(|response| {
                response
                    .into_inner()
                    .nodes
                    .into_iter()
                    .map(|node| node.content)
                    .collect::<Vec<_>>()
            })
        }
    };

    assert_eq!(
        contents(&query_id, None, 0).await.expect("run by id"),
        ["open high", "open low"]
    );
    assert_eq!(
        contents("not DONE", None, 0).await.expect("run by title"),
        ["open high", "open low"]
    );
    assert_eq!(
        contents("Not done", None, 1)
            .await
            .expect("run with a limit"),
        ["open high"]
    );
    let narrowed = serde_json::json!([
        {"property": "priority", "operator": "equals", "value": "high", "negate": true}
    ]);
    assert_eq!(
        contents("Not done", Some(narrowed), 0)
            .await
            .expect("narrowed run"),
        ["open low"]
    );

    let missing = contents("Nowhere", None, 0)
        .await
        .expect_err("no such query");
    assert_eq!(missing.code(), Code::NotFound);
    assert!(
        missing
            .message()
            .contains("no saved query has the id or title 'Nowhere'"),
        "{}",
        missing.message()
    );
    let ambiguous = contents("Twin", None, 0).await.expect_err("two queries");
    assert_eq!(ambiguous.code(), Code::InvalidArgument);
    assert!(
        ambiguous
            .message()
            .contains("2 saved queries are titled 'Twin'"),
        "{}",
        ambiguous.message()
    );
    let bad_filter = contents(
        "Not done",
        Some(serde_json::json!([{"property": "nope.deeper", "operator": "exists"}])),
        0,
    )
    .await
    .expect_err("undeclared path");
    assert_eq!(bad_filter.code(), Code::InvalidArgument);
    assert!(
        bad_filter.message().contains("nope"),
        "{}",
        bad_filter.message()
    );

    for json in [true, false] {
        commands::query::run(
            &mut client,
            commands::query::QueryArgs {
                command: Some(commands::query::QueryCommand::Run(
                    commands::query::RunArgs {
                        query: "Not done".into(),
                        filters: None,
                        limit: 0,
                        with_context: false,
                    },
                )),
                target_type: None,
                filters: None,
                sorting: None,
                limit: 0,
            },
            json,
        )
        .await
        .expect("query run");
    }

    let _ = shutdown.send(());
}

/// `node context` takes a node id and any number of dotted paths.
#[test]
fn node_context_parses_dotted_paths() {
    use clap::Parser;
    let parse = |args: &[&str]| nodespace_cli::Cli::try_parse_from(args);

    assert!(parse(&["nodespace", "node", "context", "n1"]).is_ok());
    assert!(parse(&[
        "nodespace",
        "node",
        "context",
        "n1",
        "--path",
        "project",
        "--path",
        "child_of*.project"
    ])
    .is_ok());
    assert!(parse(&["nodespace", "node", "context"]).is_err());
    assert!(parse(&["nodespace", "node", "context", "n1", "--path", "a..b"]).is_err());
    assert!(parse(&[
        "nodespace",
        "relationship",
        "delete",
        "--from",
        "a",
        "--type",
        "attached_to",
        "--to",
        "b"
    ])
    .is_ok());
    assert!(parse(&["nodespace", "relationship", "delete", "--from", "a"]).is_err());
}

/// A skill attached with `relationship create` comes back from `node context`
/// for the node it is attached to and for a node whose path reaches that
/// node, and from `query run` for a saved query it is attached to; after
/// `relationship delete` it does not.
#[tokio::test]
async fn attached_skills_come_back_with_a_node_and_a_query_run_until_detached() {
    use nodespace_daemon::nodespace::GetNodeContextRequest;

    let (sock, shutdown, _tempdir, node_service) = spawn_test_daemon_with_seeded_skills().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let create = |node_type: &str, content: &str, properties: serde_json::Value| {
        let node = nodespace_core::models::Node::new(
            node_type.to_string(),
            content.to_string(),
            properties,
        );
        let node_service = node_service.clone();
        async move { node_service.create_node(node).await.expect("create node") }
    };
    let child = |parent: String, node_type: &'static str, content: &'static str| {
        let node_service = node_service.clone();
        let create = create(node_type, content, serde_json::json!({}));
        async move {
            let id = create.await;
            node_service
                .create_relationship(&parent, "has_child", &id, serde_json::json!({}))
                .await
                .expect("place the child");
        }
    };

    let project = create("project", "Apollo", serde_json::json!({})).await;
    let task = create("task", "Write the spec", serde_json::json!({})).await;
    node_service
        .create_relationship(&project, "tasks", &task, serde_json::json!({}))
        .await
        .expect("link the task to its project");
    child(task.clone(), "checkbox", "- [ ] Draft it").await;
    let queue = create(
        "query",
        "Open tasks",
        serde_json::json!({ "target_type": "task", "filters": [] }),
    )
    .await;

    let skill = |name: &'static str, step: &'static str| {
        let node =
            nodespace_core::models::SkillFields::new("When this applies.", &[], 2).into_node(name);
        let node_service = node_service.clone();
        async move {
            let id = node_service.create_node(node).await.expect("the skill");
            (id, step)
        }
    };
    let (standards, step) = skill("Standards", "Read the project with get_node first.").await;
    child(standards.clone(), "text", step).await;
    let (procedure, step) = skill("Implementing", "Tick each item as you go.").await;
    child(procedure.clone(), "text", step).await;

    for (skill, target) in [(&standards, &project), (&procedure, &queue)] {
        commands::relationship::run(
            &mut client,
            commands::relationship::RelationshipAction::Create(
                commands::relationship::CreateArgs {
                    from: skill.clone(),
                    relationship_name: "attached_to".into(),
                    to: target.clone(),
                    edge_data: None,
                },
            ),
            false,
        )
        .await
        .expect("attach the skill");
    }

    let read = |node_id: &str, paths: serde_json::Value| {
        let request = GetNodeContextRequest {
            node_id: node_id.into(),
            paths_json: Some(paths.to_string()),
            version_only: false,
        };
        let mut raw = raw.clone();
        async move { raw.get_node_context(request).await.map(|r| r.into_inner()) }
    };

    // The task matches the queue, so it carries the queue's procedure; with
    // the path to its project, it carries the project's skill too. `task`
    // ships with that path as one of its context paths, so it is followed
    // once, in its place among them.
    let task_paths = ["spec", "plan", "decisions", "spec.decisions", "project"];
    let context = read(&task, serde_json::json!([["project"]]))
        .await
        .expect("read the task");
    let node = context.node.expect("the node");
    assert_eq!(node.node.expect("node data").id, task);
    assert_eq!(node.checkboxes.len(), 1);
    let followed: Vec<&str> = context.paths.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(followed, task_paths);
    assert_eq!(
        context.paths[4].nodes[0].node.as_ref().expect("reached").id,
        project
    );
    assert_eq!(context.skills.len(), 2);
    let through_queue = &context.skills[0];
    assert_eq!(through_queue.skill.as_ref().unwrap().name, "Implementing");
    assert!(through_queue.attached_to.is_empty());
    assert_eq!(through_queue.matched_queries.len(), 1);
    assert_eq!(through_queue.matched_queries[0].id, queue);
    assert_eq!(through_queue.matched_queries[0].title, "Open tasks");
    let attached = &context.skills[1];
    assert_eq!(attached.attached_to, std::slice::from_ref(&project));
    assert!(attached.matched_queries.is_empty());
    let fetched = attached.skill.as_ref().expect("the skill");
    assert_eq!(fetched.name, "Standards");
    assert!(
        fetched
            .instructions
            .contains("Read the project with get_node first."),
        "{}",
        fetched.instructions
    );
    // As a skill fetch returns it: with the command of each tool it names.
    assert!(
        fetched
            .tool_commands
            .iter()
            .any(|entry| entry.tool == "get_node" && entry.command == "nodespace node get"),
        "{:?}",
        fetched.tool_commands
    );

    // A read that names no path follows the type's context paths: the same
    // read as the one that named the path, so the same version.
    let unasked = read(&task, serde_json::json!([])).await.expect("read");
    assert_eq!(unasked.paths.len(), task_paths.len());
    assert_eq!(unasked.skills.len(), 2);
    assert_eq!(unasked.version, context.version);

    // A name the task's type does not declare is refused, naming the path.
    let refused = read(&task, serde_json::json!([["project", "sponsor"]]))
        .await
        .expect_err("an undeclared name");
    assert_eq!(refused.code(), Code::InvalidArgument);
    assert!(
        refused.message().contains("path 'project.sponsor'")
            && refused.message().contains("'sponsor' is not declared"),
        "{}",
        refused.message()
    );

    // The queue hands over its procedure, once, beside its nodes.
    let run = raw
        .clone()
        .run_saved_query(RunSavedQueryRequest {
            query: "Open tasks".into(),
            filters_json: None,
            limit: 0,
            with_context: false,
        })
        .await
        .expect("run the queue")
        .into_inner();
    assert_eq!(run.count, 1);
    assert!(run.items.is_empty() && !run.limit_reached);
    assert_eq!(run.skills.len(), 1);
    assert_eq!(run.skills[0].skill.as_ref().unwrap().name, "Implementing");
    assert_eq!(run.skills[0].attached_to, std::slice::from_ref(&queue));

    // The task schema declares another path as context, so a read that
    // names no path follows it too. A path written in its dotted form is the
    // one the schema read prints.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({ "schema_id": "task", "add_context_paths": ["project.tasks"] })
                    .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("declare the context path");
    let refused = commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({ "schema_id": "task", "add_context_paths": ["sponsor"] })
                    .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect_err("a context path the type does not declare");
    assert!(
        format!("{refused:#}").contains("Context path 'sponsor'"),
        "the refusal must name the path: {refused:#}"
    );
    let schema: nodespace_core::models::SchemaNode = serde_json::from_str(
        &raw.clone()
            .get_schema_definition(nodespace_daemon::nodespace::GetSchemaDefinitionRequest {
                schema_id: "task".into(),
            })
            .await
            .expect("read the task schema")
            .into_inner()
            .schema_json,
    )
    .expect("a typed schema");
    assert_eq!(
        nodespace_cli::output::schema_to_json(&schema)["context_paths"],
        serde_json::json!([
            "spec",
            "plan",
            "decisions",
            "spec.decisions",
            "project",
            "project.tasks"
        ])
    );

    let by_default = read(&task, serde_json::json!([])).await.expect("read");
    assert_eq!(by_default.paths.len(), 6);
    assert_eq!(by_default.paths[4].path, "project");
    assert_eq!(by_default.paths[5].path, "project.tasks");
    assert_eq!(
        by_default.paths[5].nodes[0]
            .node
            .as_ref()
            .expect("reached")
            .id,
        task
    );
    assert_eq!(by_default.skills.len(), 2);
    // The same read as one that names the path, so the same version.
    let named = read(&task, serde_json::json!([["project", "tasks"]]))
        .await
        .expect("read");
    assert_eq!(by_default.version, named.version);
    let version_only = raw
        .clone()
        .get_node_context(GetNodeContextRequest {
            node_id: task.clone(),
            paths_json: None,
            version_only: true,
        })
        .await
        .expect("read the version")
        .into_inner();
    assert_eq!(version_only.version, by_default.version);
    assert!(version_only.node.is_none() && version_only.skills.is_empty());

    // Run with context: the item with what governs it, and each skill once.
    let run = raw
        .clone()
        .run_saved_query(RunSavedQueryRequest {
            query: "Open tasks".into(),
            filters_json: None,
            limit: 0,
            with_context: true,
        })
        .await
        .expect("run the queue with context")
        .into_inner();
    assert_eq!(run.count, 1);
    assert!(run.nodes.is_empty());
    let item = &run.items[0];
    assert_eq!(item.node.as_ref().unwrap().node.as_ref().unwrap().id, task);
    assert_eq!(item.node.as_ref().unwrap().checkboxes.len(), 1);
    assert_eq!(item.paths[4].path, "project");
    assert_eq!(item.version, by_default.version);
    let item_skills: Vec<&str> = item.skills.iter().map(|s| s.skill_id.as_str()).collect();
    assert_eq!(item_skills, [procedure.as_str(), standards.as_str()]);
    assert_eq!(item.skills[0].matched_queries[0].id, queue);
    assert_eq!(item.skills[1].attached_to, std::slice::from_ref(&project));
    let run_skills: Vec<&str> = run
        .skills
        .iter()
        .map(|s| s.skill.as_ref().unwrap().name.as_str())
        .collect();
    assert_eq!(run_skills, ["Implementing", "Standards"]);

    for json in [true, false] {
        node_run(
            &mut client,
            commands::node::NodeAction::Context(commands::node::ContextArgs {
                id: task.clone(),
                paths: vec!["project".parse().unwrap(), "has_child".parse().unwrap()],
                version_only: false,
            }),
            json,
        )
        .await
        .expect("node context");
        node_run(
            &mut client,
            commands::node::NodeAction::Context(commands::node::ContextArgs {
                id: task.clone(),
                paths: Vec::new(),
                version_only: true,
            }),
            json,
        )
        .await
        .expect("node context --version-only");
        for with_context in [false, true] {
            commands::query::run(
                &mut client,
                commands::query::QueryArgs {
                    command: Some(commands::query::QueryCommand::Run(
                        commands::query::RunArgs {
                            query: "Open tasks".into(),
                            filters: None,
                            limit: 0,
                            with_context,
                        },
                    )),
                    target_type: None,
                    filters: None,
                    sorting: None,
                    limit: 0,
                },
                json,
            )
            .await
            .expect("query run");
        }
    }
    let unknown = node_run(
        &mut client,
        commands::node::NodeAction::Context(commands::node::ContextArgs {
            id: task.clone(),
            paths: vec!["sponsor".parse().unwrap()],
            version_only: false,
        }),
        false,
    )
    .await
    .expect_err("an undeclared path fails the command");
    assert!(
        unknown.to_string().contains("path 'sponsor'"),
        "the error must name the path: {unknown}"
    );

    // Detached, the skill is gone from the next read of that node.
    for json in [true, false] {
        commands::relationship::run(
            &mut client,
            commands::relationship::RelationshipAction::Delete(
                commands::relationship::DeleteArgs {
                    from: standards.clone(),
                    relationship_name: "attached_to".into(),
                    to: project.clone(),
                },
            ),
            json,
        )
        .await
        .expect("detach the skill (the second delete finds nothing and still succeeds)");
    }
    let context = read(&task, serde_json::json!([["project"]]))
        .await
        .expect("read the task again");
    assert_eq!(context.paths[4].nodes.len(), 1);
    let names: Vec<&str> = context
        .skills
        .iter()
        .map(|s| s.skill.as_ref().unwrap().name.as_str())
        .collect();
    assert_eq!(names, ["Implementing"]);
    assert_ne!(context.version, by_default.version);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn relationship_create_and_get() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // Relationships must be schema-defined (find-then-edit guidance: the
    // relationship name must already exist on the source node's schema).
    // Create a "ticket" schema with a "blocks" -> "ticket" relationship, then
    // two ticket instances to relate.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Ticket",
                    "fields": [{"name": "title", "type": "text"}],
                    "relationships": [
                        {"name": "blocks", "targetType": "ticket", "direction": "out", "cardinality": "many", "reverseName": "blocked_by", "reverseCardinality": "many"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("create ticket schema");

    let source = raw
        .create_node(CreateNodeRequest {
            node_type: "ticket".into(),
            content: "source".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed source")
        .into_inner()
        .node_id;

    let target = raw
        .create_node(CreateNodeRequest {
            node_type: "ticket".into(),
            content: "target".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed target")
        .into_inner()
        .node_id;

    commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: source.clone(),
            relationship_name: "blocks".into(),
            to: target.clone(),
            edge_data: None,
        }),
        true,
    )
    .await
    .expect("create relationship");

    commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Get(commands::relationship::GetArgs {
            id: source.clone(),
            relationship_name: "blocks".into(),
            direction: commands::relationship::Direction::Out,
        }),
        true,
    )
    .await
    .expect("get related nodes");

    let _ = shutdown.send(());
}

/// `relationship get` is a documented read path that does NOT go through
/// `output.rs`, so it needs its own guard: the daemon builds
/// `related_nodes_json` itself, and a plain `to_value(Node)` there would
/// serialize properties exactly as stored — namespaced under the schema id.
#[tokio::test]
async fn relationship_get_emits_flat_properties() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Ticket",
                    "fields": [{"name": "severity", "type": "text"}],
                    "relationships": [
                        {"name": "blocks", "targetType": "ticket", "direction": "out", "cardinality": "many", "reverseName": "blocked_by", "reverseCardinality": "many"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("create ticket schema");

    let make_ticket = |content: &str, severity: &str| {
        let mut raw = raw.clone();
        let req = CreateNodeRequest {
            node_type: "ticket".into(),
            content: content.into(),
            parent_id: None,
            properties: serde_json::json!({"severity": severity}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        };
        async move {
            raw.create_node(req)
                .await
                .expect("seed ticket")
                .into_inner()
                .node_id
        }
    };

    let source = make_ticket("source", "low").await;
    let target = make_ticket("target", "high").await;

    commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: source.clone(),
            relationship_name: "blocks".into(),
            to: target.clone(),
            edge_data: None,
        }),
        true,
    )
    .await
    .expect("create relationship");

    let response = raw
        .get_related_nodes(GetRelatedNodesRequest {
            node_id: source.clone(),
            relationship_name: "blocks".into(),
            direction: "out".into(),
        })
        .await
        .expect("get related nodes")
        .into_inner();

    let related: serde_json::Value =
        serde_json::from_str(&response.related_nodes_json).expect("parse related_nodes_json");
    let first = &related[0];

    assert_eq!(
        first["properties"]["severity"], "high",
        "related nodes must carry flat properties like every other read path"
    );
    assert!(
        first["properties"].get("ticket").is_none(),
        "the schema id must not be observable in `relationship get` output"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn relationship_create_rejects_relationship_undefined_on_source_schema() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // Plain "text" nodes have no schema-defined relationships at all, so
    // any non-built-in relationship name must be rejected (find-then-edit
    // guidance: the relationship must already exist on the source's schema).
    let source = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "source".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed source")
        .into_inner()
        .node_id;

    let target = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "target".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed target")
        .into_inner()
        .node_id;

    let err = commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: source,
            relationship_name: "not_a_defined_relationship".into(),
            to: target,
            edge_data: None,
        }),
        true,
    )
    .await
    .expect_err("relationship not defined on schema should error");
    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert!(
        status.message().contains("not_a_defined_relationship")
            || status.message().contains("not defined"),
        "expected error to name the undefined relationship, got: {}",
        status.message()
    );

    let _ = shutdown.send(());
}

// `nodespace relationship get --direction` is a clap ValueEnum (only "out"/"in"
// are constructible), so an invalid direction can no longer reach the CLI's
// GetArgs at all — clap itself rejects it before this test's code would run.
// The daemon-side validation this used to exercise is still real (any other
// gRPC client, not just this CLI, can send an arbitrary string on the wire),
// so this drives GetRelatedNodesRequest directly against the raw client.
#[tokio::test]
async fn get_related_nodes_rpc_rejects_invalid_direction() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let status = raw
        .get_related_nodes(GetRelatedNodesRequest {
            node_id: "some-id".into(),
            relationship_name: "blocks".into(),
            direction: "sideways".into(),
        })
        .await
        .expect_err("invalid direction should error");
    assert_eq!(status.code(), Code::InvalidArgument);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_update_sets_properties_and_preserves_content() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "original content".into(),
            parent_id: None,
            properties: serde_json::json!({"custom:existing": "keep-me"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner()
        .node_id;

    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: id.clone(),
            content: None,
            properties: vec![("custom:added".into(), serde_json::json!("value"))],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("update properties only");

    let node = raw
        .get_node(GetNodeRequest {
            node_id: id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    assert_eq!(
        node.content, "original content",
        "content must be preserved when only properties are set"
    );
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    // Typed properties are namespaced under the node's type key on the wire
    // (properties.<node_type>.<field>), per the typed-value shape produced by
    // crate::models::node_to_typed_value.
    assert_eq!(props["text"]["custom:added"], "value");
    assert_eq!(
        props["text"]["custom:existing"], "keep-me",
        "existing properties must be deep-merged, not replaced"
    );

    let _ = shutdown.send(());
}

/// The CLI must never expose the storage-layer property nesting.
///
/// A consumer that parsed `--json` and read `.properties.status` got `None`
/// against the nested storage shape and could report "status unknown" about
/// real data. These assert both halves of the contract at once: storage stays
/// namespaced under the type key, while what the CLI emits is flat.
#[tokio::test]
async fn cli_json_output_is_flat_while_storage_stays_nested() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // --- a core type -----------------------------------------------------
    let task_id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "Buy groceries".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create task")
        .into_inner()
        .node_id;

    let task = raw
        .get_node(GetNodeRequest {
            node_id: task_id.clone(),
        })
        .await
        .expect("get task")
        .into_inner()
        .node_data
        .expect("node_data");

    // Storage is namespaced — unchanged by this fix.
    let stored: serde_json::Value =
        serde_json::from_str(&task.properties).expect("parse stored properties");
    assert_eq!(
        stored["task"]["status"], "open",
        "storage must stay nested under the type key"
    );

    // The CLI surface is flat.
    let emitted = nodespace_cli::output::node_to_json(&task);
    assert_eq!(
        emitted["properties"]["status"], "open",
        "`jq '.properties.status'` must work with no second parse"
    );
    assert!(
        emitted["properties"].get("task").is_none(),
        "the schema id must not be observable in CLI output"
    );
    assert!(
        emitted["properties"].get("_schema_version").is_none(),
        "`_`-prefixed internals must not reach a CLI consumer"
    );

    // --- a user-defined type ---------------------------------------------
    commands::schema::run(
        &mut raw.clone(),
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Venue",
                    "fields": [{"name": "capacity", "type": "number"}]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    let venue_id = raw
        .create_node(CreateNodeRequest {
            node_type: "venue".into(),
            content: "Grand Hall".into(),
            parent_id: None,
            properties: serde_json::json!({"capacity": 250}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create venue")
        .into_inner()
        .node_id;

    let venue = raw
        .get_node(GetNodeRequest { node_id: venue_id })
        .await
        .expect("get venue")
        .into_inner()
        .node_data
        .expect("node_data");

    let emitted = nodespace_cli::output::node_to_json(&venue);
    assert_eq!(
        emitted["properties"]["capacity"], 250,
        "a user-defined type flattens by the same rule as a core type"
    );
    assert!(
        emitted["properties"].get("venue").is_none(),
        "the schema id must not be observable for user-defined types either"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_update_rejects_empty_args() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: "irrelevant".into(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect_err("update with no content and no properties should error");
    assert!(err.to_string().contains("--content"));

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_set_status_updates_status_property() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "a task".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed task")
        .into_inner()
        .node_id;

    node_run(
        &mut client,
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id: id.clone(),
            status: "done".into(),
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("set status");

    let node = raw
        .get_node(GetNodeRequest {
            node_id: id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    assert_eq!(props["task"]["status"], "done");

    let _ = shutdown.send(());
}

#[test]
fn node_update_and_set_status_take_a_dry_run_flag() {
    use clap::Parser;
    let parse = |args: &[&str]| nodespace_cli::Cli::try_parse_from(args);

    assert!(parse(&[
        "nodespace",
        "node",
        "set-status",
        "t",
        "in_progress",
        "--dry-run"
    ])
    .is_ok());
    assert!(parse(&[
        "nodespace",
        "node",
        "set-status",
        "t",
        "in_progress",
        "--dry-run",
        "--version",
        "3"
    ])
    .is_ok());
    assert!(parse(&[
        "nodespace",
        "node",
        "update",
        "t",
        "--property",
        "a=1",
        "--dry-run"
    ])
    .is_ok());
    assert!(parse(&[
        "nodespace",
        "node",
        "update",
        "t",
        "--content",
        "x",
        "--dry-run"
    ])
    .is_ok());
    // A collection change runs no rule, so a dry run of one is refused.
    for collection_flag in ["--collection", "--collection-id", "--remove-collection-id"] {
        assert!(
            parse(&[
                "nodespace",
                "node",
                "update",
                "t",
                "--dry-run",
                collection_flag,
                "c"
            ])
            .is_err(),
            "{collection_flag}"
        );
    }
}

/// `--dry-run` on `node update` and `node set-status` asks the daemon what
/// the rules would say and writes nothing: the node keeps its fields and its
/// version, and a stale `--version` is refused as on a write.
#[tokio::test]
async fn a_dry_run_update_or_set_status_writes_nothing() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "a task".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed task")
        .into_inner()
        .node_id;
    let stored = |raw: &mut NodeClient| {
        let id = id.clone();
        let mut raw = raw.clone();
        async move {
            raw.get_node(GetNodeRequest { node_id: id })
                .await
                .expect("get")
                .into_inner()
                .node_data
                .expect("node_data")
        }
    };
    let before = stored(&mut raw).await;

    for json in [false, true] {
        node_run(
            &mut client,
            commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
                id: id.clone(),
                status: "in_progress".into(),
                version: Some(before.version),
                dry_run: true,
            }),
            json,
        )
        .await
        .expect("a dry run answers whichever way the rules go");
        node_run(
            &mut client,
            commands::node::NodeAction::Update(commands::node::UpdateArgs {
                properties_json: None,
                id: id.clone(),
                content: Some("renamed".into()),
                properties: vec![("status".into(), serde_json::json!("done"))],
                collections: vec![],
                collection_ids: vec![],
                remove_collection_ids: vec![],
                version: None,
                dry_run: true,
            }),
            json,
        )
        .await
        .expect("a dry run answers whichever way the rules go");
    }
    let after = stored(&mut raw).await;
    assert_eq!(after.version, before.version, "no version changes");
    assert_eq!(after.content, "a task");
    assert_eq!(after.properties, before.properties);

    // A stale version is refused, as on the write the dry run stands for.
    let err = node_run(
        &mut client,
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id: id.clone(),
            status: "in_progress".into(),
            version: Some(before.version + 5),
            dry_run: true,
        }),
        false,
    )
    .await
    .expect_err("the version named is not the node's");
    assert!(
        format!("{err:#}").contains("Nothing was written"),
        "{err:#}"
    );

    let _ = shutdown.send(());
}

/// `node update` and `node set-status` change a node only when it is still at
/// the version the caller names. A stale version is refused with a message
/// naming the node, the version given and the version it is at now, and
/// nothing is written.
#[tokio::test]
async fn update_and_set_status_write_only_at_the_version_named() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "a task".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed task")
        .into_inner()
        .node_id;

    let set_status = |status: &str, version| {
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id: id.clone(),
            status: status.into(),
            version,
            dry_run: false,
        })
    };
    let update = |content: &str, version| {
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: id.clone(),
            content: Some(content.into()),
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version,
            dry_run: false,
        })
    };
    let assert_refused = |err: anyhow::Error, given: i64, current: i64| {
        let message = format!("{err:#}");
        for part in [
            id.clone(),
            format!("version {given} was given"),
            format!("now at version {current}"),
            "Nothing was written".to_string(),
        ] {
            assert!(message.contains(&part), "missing {part:?}: {message}");
        }
        assert!(
            !message.contains("RPC failed"),
            "a conflict is not a generic RPC failure: {message}"
        );
    };

    // The first session to start the task with the version it read wins.
    node_run(&mut client, set_status("in_progress", Some(1)), false)
        .await
        .expect("the version read is still current");

    // A second session that read the same version is refused, in both
    // output modes.
    for json in [false, true] {
        let err = node_run(&mut client, set_status("done", Some(1)), json)
            .await
            .expect_err("version 1 is stale");
        assert_refused(err, 1, 2);
    }
    let err = node_run(&mut client, update("taken over", Some(1)), false)
        .await
        .expect_err("version 1 is stale");
    assert_refused(err, 1, 2);

    let node = raw
        .get_node(GetNodeRequest {
            node_id: id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    assert_eq!(node.version, 2, "a refused write changes nothing");
    assert_eq!(node.content, "a task");
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    assert_eq!(props["task"]["status"], "in_progress");

    // Naming the current version lands; naming none applies to what is there.
    node_run(&mut client, update("renamed", Some(2)), false)
        .await
        .expect("version 2 is current");
    node_run(&mut client, set_status("done", None), false)
        .await
        .expect("no version: as before");

    // A version named on an update that only changes collections is held to
    // too, and a refused one joins nothing.
    let join = |version| {
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: id.clone(),
            content: None,
            properties: vec![],
            collections: vec!["claimed".into()],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version,
            dry_run: false,
        })
    };
    let err = node_run(&mut client, join(Some(1)), false)
        .await
        .expect_err("version 1 is stale");
    assert_refused(err, 1, 4);
    let collection_request = || nodespace_daemon::nodespace::QueryNodesSimpleRequest {
        include_archived: false,
        id: None,
        mentioned_by: None,
        content_contains: None,
        title_contains: None,
        node_type: Some("collection".into()),
        limit: 0,
        offset: 0,
        order_by: nodespace_daemon::nodespace::NodeSortOrder::Unspecified as i32,
    };
    let collections = raw
        .query_nodes_simple(collection_request())
        .await
        .expect("query collections")
        .into_inner();
    assert!(
        collections.nodes.iter().all(|c| c.content != "claimed"),
        "a refused update must not create or join a collection"
    );
    node_run(&mut client, join(Some(4)), false)
        .await
        .expect("version 4 is current");
    let collections = raw
        .query_nodes_simple(collection_request())
        .await
        .expect("query collections")
        .into_inner();
    assert!(
        collections.nodes.iter().any(|c| c.content == "claimed"),
        "the update at the current version joins the collection"
    );

    // A node too large for the conflict header to carry is still reported
    // as a conflict, with its versions, across the socket.
    let large = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "日本語のノート ".repeat(4_000),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed large node")
        .into_inner()
        .node_id;
    let err = node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: large.clone(),
            content: Some("short".into()),
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: Some(7),
            dry_run: false,
        }),
        false,
    )
    .await
    .expect_err("version 7 was never this node's");
    let message = format!("{err:#}");
    assert!(
        message.contains(&large)
            && message.contains("version 7 was given")
            && message.contains("now at version 1"),
        "{message}"
    );

    // The structured form `--json` prints carries the same three facts.
    let status = raw
        .update_node(nodespace_daemon::nodespace::UpdateNodeRequest {
            node_id: id.clone(),
            version: Some(2),
            node_type: None,
            content: Some("stale".into()),
            properties: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
            typed_client: false,
        })
        .await
        .expect_err("version 2 is stale");
    let conflict = commands::node::VersionConflict::from_status(&status)
        .expect("the daemon reports a version conflict");
    let printed = conflict.to_json();
    assert_eq!(printed["error"], "version_conflict");
    assert_eq!(printed["node_id"], id.as_str());
    assert_eq!(printed["given_version"], 2);
    assert_eq!(printed["current_version"], 4);
    assert_eq!(printed["message"], conflict.message());

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_set_status_rejects_invalid_status() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // A real node: with no CLI-side vocabulary check left, the RPC actually
    // reaches the daemon, which resolves the node before validating the
    // property — a nonexistent id would fail with NotFound first and never
    // exercise the status check this test is for.
    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "a task".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed task")
        .into_inner()
        .node_id;

    // No CLI-side vocabulary check remains, so this exercises the daemon's
    // live-schema validation (the update pipeline's enum check) via the
    // RPC — same error-mapping path as `schema_create_rejects_malformed_params`.
    let err = node_run(
        &mut client,
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id,
            status: "not-a-real-status".into(),
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect_err("invalid status should error");
    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("not-a-real-status"),
        "expected status message to name the offending value, got: {}",
        status.message()
    );
    assert!(
        status.message().contains("open"),
        "expected status message to list the valid values, got: {}",
        status.message()
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_set_status_accepts_schema_extended_status() {
    // Reproduces the documented schema-extension flow: `schema update` with
    // `add_field_values` adds `backlog` to `task.status` (which is declared
    // `extensible: true`), and `set-status` must accept it since the daemon's
    // live vocabulary — not a hardcoded CLI list — is the source of truth.
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "schema_id": "task",
                    "add_field_values": [{
                        "field": "status",
                        "values": [{"value": "backlog", "label": "Backlog"}]
                    }]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("extending task.status with 'backlog' should succeed");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "a task".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed task")
        .into_inner()
        .node_id;

    node_run(
        &mut client,
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id: id.clone(),
            status: "backlog".into(),
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("set-status should accept the schema-extended value");

    let node = raw
        .get_node(GetNodeRequest {
            node_id: id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    assert_eq!(props["task"]["status"], "backlog");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn database_registry_round_trip() {
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    // list: succeeds and reports the seeded default.
    commands::database::run(&mut db, commands::database::DatabaseAction::List, true)
        .await
        .expect("database list");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.databases.len(), 1);
    let default_id = listed.default_database_id.clone();
    assert!(!default_id.is_empty());

    // create: registers a second database.
    let second_path = tempdir.path().join("second.db").display().to_string();
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Create(commands::database::CreateArgs {
            name: "Second".into(),
            path: Some(second_path),
        }),
        true,
    )
    .await
    .expect("database create");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.databases.len(), 2);
    let second_id = listed
        .databases
        .iter()
        .find(|d| d.name == "Second")
        .expect("second registered")
        .id
        .clone();
    // Create makes the database file before registering it — it exists on disk
    // as soon as the command returns, without any request having routed to it.
    assert!(
        tempdir.path().join("second.db").exists(),
        "create must create the database file"
    );

    // Route a write to the second database, proving the freshly created
    // database serves requests immediately.
    let mut node_second = node_client_for(&sock, &second_id).await;
    node_second
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "open the second database".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("open second database");

    // rename: resolves by name and relabels.
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Rename(commands::database::RenameArgs {
            database: "Second".into(),
            new_name: "Renamed".into(),
        }),
        true,
    )
    .await
    .expect("database rename");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    let renamed = listed
        .databases
        .iter()
        .find(|d| d.id == second_id)
        .expect("still registered by id");
    assert_eq!(renamed.name, "Renamed");

    // use: sets the daemon-wide default to the renamed database.
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Use(commands::database::UseArgs {
            database: "Renamed".into(),
        }),
        true,
    )
    .await
    .expect("database use");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.default_database_id, second_id);

    // Put the default back so removing the (now non-default) original succeeds.
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Use(commands::database::UseArgs {
            database: default_id.clone(),
        }),
        true,
    )
    .await
    .expect("database use back to default");

    // remove: unregisters by id without deleting the file.
    let second_file = tempdir.path().join("second.db");
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Remove(commands::database::RemoveArgs {
            database: second_id.clone(),
        }),
        true,
    )
    .await
    .expect("database remove");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.databases.len(), 1);
    assert!(
        listed.databases.iter().all(|d| d.id != second_id),
        "removed database must be gone from the registry"
    );
    assert!(
        second_file.exists(),
        "remove must not delete the underlying database file"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn select_database_by_name_resolves_to_id() {
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    let created = db
        .create(CreateDatabaseRequest {
            name: "Workspace".into(),
            path: Some(tempdir.path().join("workspace.db").display().to_string()),
        })
        .await
        .expect("create database")
        .into_inner();

    // A name resolves to the matching id...
    let resolved = commands::database::resolve_database_id_by_selection(&mut db, "Workspace")
        .await
        .expect("resolve by name");
    assert_eq!(resolved, created.id);

    // ...and the id resolves to itself.
    let resolved_by_id = commands::database::resolve_database_id_by_selection(&mut db, &created.id)
        .await
        .expect("resolve by id");
    assert_eq!(resolved_by_id, created.id);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn unregistered_database_selection_errors() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    let err = commands::database::resolve_database_id_by_selection(&mut db, "no-such-database")
        .await
        .expect_err("unregistered selection must error");
    assert!(
        err.to_string().contains("no database named or with id"),
        "expected a clear not-registered error, got: {err}"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn ambiguous_name_selection_errors_but_id_still_resolves() {
    // The daemon does not enforce unique names, so two databases can share a
    // name. Selecting by that name must fail with an ambiguity error; selecting
    // by id must still resolve (id match wins over any name match).
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    let first = db
        .create(CreateDatabaseRequest {
            name: "work".into(),
            path: Some(tempdir.path().join("work-a.db").display().to_string()),
        })
        .await
        .expect("create first work")
        .into_inner();
    db.create(CreateDatabaseRequest {
        name: "work".into(),
        path: Some(tempdir.path().join("work-b.db").display().to_string()),
    })
    .await
    .expect("create second work");

    let err = commands::database::resolve_database_id_by_selection(&mut db, "work")
        .await
        .expect_err("a name shared by two databases must be ambiguous");
    let msg = err.to_string();
    assert!(
        msg.contains("ambiguous"),
        "expected an ambiguity error, got: {msg}"
    );
    // Both colliding ids should be listed so the user can disambiguate.
    assert!(
        msg.contains(&first.id),
        "ambiguity error should list the colliding ids, got: {msg}"
    );

    // Selecting by id sidesteps the ambiguity.
    let resolved = commands::database::resolve_database_id_by_selection(&mut db, &first.id)
        .await
        .expect("id resolves unambiguously even with a duplicate name");
    assert_eq!(resolved, first.id);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn database_routing_isolates_writes() {
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    // Register a second database.
    db.create(CreateDatabaseRequest {
        name: "Second".into(),
        path: Some(tempdir.path().join("second.db").display().to_string()),
    })
    .await
    .expect("create second")
    .into_inner();

    // Resolve its id by name and route a NodeService client to it.
    let second_id = commands::database::resolve_database_id_by_selection(&mut db, "Second")
        .await
        .expect("resolve second");
    let mut node_second = node_client_for(&sock, &second_id).await;

    // Create a node routed to the second database via the node command handler.
    node_run(
        &mut node_second,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "text".into(),
            content: Some("isolated-to-second".into()),
            parent: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create in second");

    let query = QueryNodesSimpleRequest {
        include_archived: false,
        id: None,
        mentioned_by: None,
        content_contains: Some("isolated-to-second".into()),
        title_contains: None,
        node_type: None,
        limit: 0,
        offset: 0,
        order_by: NodeSortOrder::Unspecified as i32,
    };

    // Visible from the second database...
    let in_second = node_second
        .query_nodes_simple(query.clone())
        .await
        .expect("query second")
        .into_inner();
    assert_eq!(
        in_second.nodes.len(),
        1,
        "the write must be visible in the database it was routed to"
    );

    // ...and invisible from the default (header-less) database — no cross-database bleed.
    let mut node_default = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect default");
    let in_default = node_default
        .query_nodes_simple(query)
        .await
        .expect("query default")
        .into_inner();
    assert!(
        in_default.nodes.is_empty(),
        "the write must not leak into the default database"
    );

    let _ = shutdown.send(());
}

/// tonic's default decode limit is 4 MiB, which several list-shaped RPCs in
/// this contract can legitimately exceed on a real database: `QueryNodesSimple`
/// returns every matching node's full record in a single message. This seeds a
/// response comfortably past that default and asserts both halves of the
/// contract — a client left on the default limit fails with `OutOfRange`
/// (proving the payload really does cross the boundary, so the test can't go
/// vacuous), while the client the CLI actually builds reads it in full.
#[tokio::test]
async fn query_nodes_simple_handles_response_over_the_default_grpc_limit() {
    const DEFAULT_TONIC_DECODE_LIMIT: usize = 4 * 1024 * 1024;
    const NODE_COUNT: usize = 90;
    const CONTENT_BYTES: usize = 60_000;

    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut seed = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect seed");

    // 90 × 60 KB ≈ 5.1 MB of content alone — past 4 MiB even before proto
    // framing and the rest of each record.
    let query = QueryNodesSimpleRequest {
        include_archived: false,
        id: None,
        mentioned_by: None,
        content_contains: None,
        title_contains: None,
        node_type: None,
        // Requesting far more than MAX_ROW_LIMIT is
        // deliberate: the server clamps the row count, not this test's
        // point (a large-CONTENT response still exceeds tonic's default
        // decode limit well under that row cap — see NODE_COUNT/CONTENT_BYTES
        // above).
        limit: 100_000,
        offset: 0,
        order_by: NodeSortOrder::Unspecified as i32,
    };

    // The daemon bootstraps its own nodes (schemas and friends), so count what
    // is already there rather than assuming an empty database.
    let baseline = seed
        .query_nodes_simple(query.clone())
        .await
        .expect("baseline query")
        .into_inner()
        .nodes
        .len();

    let filler = "x".repeat(CONTENT_BYTES);
    for _ in 0..NODE_COUNT {
        seed.create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: filler.clone(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed large node");
    }

    // A client on tonic's default decode limit cannot read this response.
    let mut default_limit_client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect default-limit")
        .max_decoding_message_size(DEFAULT_TONIC_DECODE_LIMIT);
    let status = default_limit_client
        .query_nodes_simple(query.clone())
        .await
        .expect_err("a >4 MiB response must exceed tonic's default decode limit");
    assert_eq!(
        status.code(),
        Code::OutOfRange,
        "expected the decode-limit failure this test exists to prevent, got: {status}"
    );

    // The client the CLI builds reads the same response in full.
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let response = client
        .query_nodes_simple(query)
        .await
        .expect("configured client must read a multi-megabyte response")
        .into_inner();
    assert_eq!(
        response.nodes.len(),
        baseline + NODE_COUNT,
        "every seeded node must come back"
    );

    let _ = shutdown.send(());
}

/// When the node query fails, diagnostics must report the count as unknown and
/// exit non-zero — never present a failed query as a zero-node database.
#[tokio::test]
async fn diagnostics_reports_unknown_counts_when_the_node_query_fails() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;
    let mut seed = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect seed");
    let mut db = connect_database(&sock).await.expect("connect database");

    // Seed real nodes so a reported count of 0 would be provably wrong.
    for label in ["alpha", "beta", "gamma"] {
        seed.create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: label.into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .unwrap_or_else(|e| panic!("seed {label}: {e}"));
    }

    // Clamping the decode limit reproduces what an oversized response does to a
    // real client, without having to seed one here.
    let mut starved = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect starved")
        .max_decoding_message_size(1);

    let report = commands::diagnostics::collect(&mut starved, &mut db, None)
        .await
        .expect("no database is refused");

    assert!(
        report.total_node_count.is_none(),
        "a failed node query must report an unknown count, not a number: {:?}",
        report.total_node_count
    );
    assert!(
        report.recent_node_ids.is_none(),
        "recency is derived from the failed query and must be unknown too"
    );
    // The clamped client starves the memory RPC as well. The figure must go
    // unknown rather than fall back to a 0 that would read as "the daemon uses
    // no memory".
    assert!(
        report.daemon_rss_bytes.is_none(),
        "a failed memory query must report unknown, not a number: {:?}",
        report.daemon_rss_bytes
    );
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("GetDaemonMemory failed")),
        "the memory RPC failure must be surfaced: {:?}",
        report.errors
    );
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("QueryNodesSimple failed")),
        "the underlying failure must be surfaced: {:?}",
        report.errors
    );
    // The registry enumeration rides an unclamped client, so the report is
    // partial rather than empty — that is exactly the state that must not read
    // as a success.
    assert!(
        !report.databases.is_empty(),
        "registry enumeration should still succeed"
    );

    let err = commands::diagnostics::run(
        &mut starved,
        &mut db,
        None,
        commands::diagnostics::DiagnosticsArgs {},
        false,
    )
    .await
    .expect_err("diagnostics must exit non-zero when a query failed");
    assert!(
        err.to_string().contains("diagnostics incomplete"),
        "expected a loud failure, got: {err}"
    );

    let _ = shutdown.send(());
}

/// `node create --collection <path>` files the node in one call, auto-creating
/// every missing path segment, and is repeatable so a node can join several
/// collections without a follow-up write.
#[tokio::test]
async fn node_create_collection_paths_are_repeatable_and_auto_create() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "text".into(),
            content: Some("collected via CLI".into()),
            parent: None,
            properties: vec![],
            // Neither path exists yet: `docs:rust` is nested, so `docs` and
            // `rust` are both created and wired member_of.
            collections: vec!["docs:rust".into(), "reference".into()],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create with collections");

    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let found = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: false,
            node_type: Some("text".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("query")
        .into_inner();
    let node_id = found
        .nodes
        .iter()
        .find(|n| n.content == "collected via CLI")
        .expect("created node")
        .id
        .clone();

    let memberships = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(
        memberships.collection_ids.len(),
        2,
        "one create call must produce both memberships: {:?}",
        memberships.collection_ids
    );

    // The nested path's intermediate segment exists as its own collection, so
    // `docs:rust` resolved as a hierarchy rather than as a flat label.
    let collections = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: false,
            node_type: Some("collection".into()),
            limit: 20,
            ..Default::default()
        })
        .await
        .expect("query collections")
        .into_inner();
    let names: Vec<&str> = collections
        .nodes
        .iter()
        .map(|n| n.content.as_str())
        .collect();
    for expected in ["docs", "rust", "reference"] {
        assert!(
            names.contains(&expected),
            "missing auto-created collection '{expected}' in {names:?}"
        );
    }

    let _ = shutdown.send(());
}

/// `node update --collection <path>` adds membership after the fact, and
/// `--remove-collection-id` takes it away again.
#[tokio::test]
async fn node_update_collection_adds_and_removes_membership() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let created = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "late joiner".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner();
    let node_id = created.node_id;

    // --collection alone is enough to make an update meaningful: no --content
    // or --property is required.
    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec!["archive:2026".into()],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("update with collection");

    let after_add = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(after_add.collection_ids.len(), 1);
    let leaf_id = after_add.collection_ids[0].clone();

    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![leaf_id.clone()],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("update removing collection");

    let after_remove = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest { node_id })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert!(
        after_remove.collection_ids.is_empty(),
        "membership should be gone: {:?}",
        after_remove.collection_ids
    );

    let _ = shutdown.send(());
}

/// Removal takes collection IDs, not paths — the asymmetry with
/// `add_to_collections` (which takes paths) is why the field is named
/// `remove_from_collection_ids`.
///
/// This pins the consequence, not just the name. Removal detaches an existing
/// `member_of` edge, so a path passed where an id belongs matches no edge and
/// `delete_relationship` reports success: the call returns Ok and the
/// membership survives. That is a silent data-integrity failure rather than an
/// error, so a rename back to a symmetric `remove_from_collections` — which
/// would make passing a path look correct — must fail here.
#[tokio::test]
async fn removing_by_path_instead_of_id_does_not_silently_drop_membership() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let created = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "membership survives a bad removal".into(),
            parent_id: None,
            properties: String::new(),
            collections: vec!["ops:oncall".into()],
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner();
    let node_id = created.node_id;

    let before = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(before.collection_ids.len(), 1, "fixture must be a member");

    // Pass the PATH where an id belongs — the mistake the old symmetric field
    // name invited. It removes nothing.
    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec!["ops:oncall".into()],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("the call itself succeeds — that is precisely the hazard");

    let after = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(
        after.collection_ids, before.collection_ids,
        "a path is not an id: nothing should have been removed"
    );

    // The id form is what actually detaches the edge.
    let leaf_id = before.collection_ids[0].clone();
    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![leaf_id],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("removal by id");

    let removed = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest { node_id })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert!(
        removed.collection_ids.is_empty(),
        "removal by id must actually detach: {:?}",
        removed.collection_ids
    );

    let _ = shutdown.send(());
}

/// A collection that cannot be resolved must fail the command rather than
/// leaving the caller with a silently uncollected node.
#[tokio::test]
async fn node_create_unresolvable_collection_is_an_error() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "text".into(),
            content: Some("should fail".into()),
            parent: None,
            properties: vec![],
            // An empty path has no segments to resolve.
            collections: vec!["".into()],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect_err("an unresolvable collection path must surface as an error");
    assert!(
        err.to_string().contains("CreateNode RPC failed"),
        "unexpected error: {err}"
    );

    let _ = shutdown.send(());
}

/// A schema field that is `required` with no `default` can only be satisfied
/// at create time (validation runs on create; `update` cannot run before the
/// node exists). Before `--property` existed on `node create`, such a field
/// made the type entirely uninstantiable from the CLI. Confirms both halves:
/// creating without `--property` still fails with the validation error
/// (existing behavior, unchanged), and supplying the field via `--property`
/// now succeeds and the value round-trips into storage.
#[tokio::test]
async fn node_create_required_field_without_default_needs_property_flag() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Customer",
                    "fields": [
                        {"name": "company_name", "type": "text", "required": true}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    // Without --property the required field cannot be supplied at all:
    // creation must still fail with the daemon's validation error, exactly
    // as before this flag existed.
    let err = node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "customer".into(),
            content: Some("Northwind Labs".into()),
            parent: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect_err("required field with no default must fail without --property");
    let err_chain = format!("{err:?}");
    assert!(
        err_chain.contains("Required field 'company_name' is missing"),
        "unexpected error: {err_chain}"
    );

    // With --property, the same create now succeeds.
    node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "customer".into(),
            content: Some("Northwind Labs".into()),
            parent: None,
            properties: vec![("company_name".into(), serde_json::json!("Northwind Labs"))],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create with required property supplied via --property");

    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");
    let found = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: false,
            node_type: Some("customer".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("query")
        .into_inner();
    assert_eq!(found.nodes.len(), 1, "exactly one customer node expected");
    let node = raw
        .get_node(GetNodeRequest {
            node_id: found.nodes[0].id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    assert_eq!(props["customer"]["company_name"], "Northwind Labs");

    let _ = shutdown.send(());
}

/// A link field is written with `--property` as its `{title, url}` object,
/// cleared with `null`, and read back as that object. A bare URL is refused
/// with the validation message, and `schema get` reports the type as `link`.
#[tokio::test]
async fn node_property_sets_reads_and_clears_a_link() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Vendor",
                    "fields": [{"name": "website", "type": "link"}]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");
    let schema = raw
        .get_schema_definition(GetSchemaDefinitionRequest {
            schema_id: "vendor".into(),
        })
        .await
        .expect("schema get")
        .into_inner();
    let schema: serde_json::Value =
        serde_json::from_str(&schema.schema_json).expect("parse schema");
    assert_eq!(schema["fields"][0]["type"], "link");

    let create = |properties| {
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            node_type: "vendor".into(),
            content: Some("Acme".into()),
            parent: None,
            properties,
            properties_json: None,
            collections: vec![],
            collection_ids: vec![],
        })
    };
    let err = node_run(
        &mut client,
        create(vec![(
            "website".into(),
            serde_json::json!("https://acme.example"),
        )]),
        true,
    )
    .await
    .expect_err("a bare URL is not a link");
    assert!(
        format!("{err:?}").contains("Link field 'website'"),
        "unexpected error: {err:?}"
    );

    let link = serde_json::json!({"title": "Acme", "url": "https://acme.example"});
    node_run(
        &mut client,
        create(vec![("website".into(), link.clone())]),
        true,
    )
    .await
    .expect("create with a link");

    let found = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: false,
            node_type: Some("vendor".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("query")
        .into_inner();
    assert_eq!(found.nodes.len(), 1, "exactly one vendor expected");
    let id = found.nodes[0].id.clone();
    // What `--json` prints: the flat properties, the link as its object.
    assert_eq!(
        nodespace_cli::output::node_to_json(&found.nodes[0])["properties"]["website"],
        link
    );

    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: id.clone(),
            content: None,
            properties: vec![("website".into(), serde_json::Value::Null)],
            properties_json: None,
            version: None,
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("clear the link");
    let node = raw
        .get_node(GetNodeRequest { node_id: id })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    let website = nodespace_cli::output::node_to_json(&node)["properties"]
        .get("website")
        .cloned();
    assert!(website.as_ref().is_none_or(|v| v.is_null()), "{website:?}");

    let _ = shutdown.send(());
}

/// `--property` on `node create` is repeatable, mirroring `node update`, so
/// several required fields can all be satisfied in one call.
#[tokio::test]
async fn node_create_multiple_property_flags_set_multiple_fields() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Invoice",
                    "fields": [
                        {"name": "invoice_number", "type": "text", "required": true},
                        {"name": "amount", "type": "number", "required": true}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    node_run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "invoice".into(),
            content: Some("INV-1001".into()),
            parent: None,
            properties: vec![
                ("invoice_number".into(), serde_json::json!("INV-1001")),
                ("amount".into(), serde_json::json!(500)),
            ],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create with both required properties supplied via repeated --property");

    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");
    let found = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: false,
            node_type: Some("invoice".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("query")
        .into_inner();
    assert_eq!(found.nodes.len(), 1, "exactly one invoice node expected");
    let node = raw
        .get_node(GetNodeRequest {
            node_id: found.nodes[0].id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    assert_eq!(props["invoice"]["invoice_number"], "INV-1001");
    assert_eq!(props["invoice"]["amount"], 500);

    let _ = shutdown.send(());
}

/// `--collection` and `--collection-id` are mutually exclusive at the parser,
/// matching how `search` already handles the same pair.
#[test]
fn collection_path_and_id_flags_are_mutually_exclusive() {
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct CreateHarness {
        #[command(flatten)]
        args: commands::node::CreateArgs,
    }

    #[derive(Parser, Debug)]
    struct UpdateHarness {
        #[command(flatten)]
        args: commands::node::UpdateArgs,
    }

    let err = CreateHarness::try_parse_from([
        "create",
        "--type",
        "text",
        "--content",
        "x",
        "--collection",
        "docs",
        "--collection-id",
        "abc",
    ])
    .expect_err("create must reject both collection forms at once");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);

    let err = UpdateHarness::try_parse_from([
        "update",
        "node-1",
        "--collection",
        "docs",
        "--collection-id",
        "abc",
    ])
    .expect_err("update must reject both collection forms at once");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);

    // Each form on its own parses, and both are repeatable.
    let ok = CreateHarness::try_parse_from([
        "create",
        "--type",
        "text",
        "--content",
        "x",
        "--collection",
        "docs:rust",
        "--collection",
        "reference",
    ])
    .expect("repeated --collection must parse");
    assert_eq!(ok.args.collections, vec!["docs:rust", "reference"]);
}

/// `--property` on `node create` is repeatable and value-parsed by
/// `parse_property` the same way `node update`'s already is — this exercises
/// the real clap arg parser (not a hand-built `CreateArgs` literal), so a
/// regression in the `#[arg(...)]` wiring itself (not just in `create()`'s
/// use of the parsed result) would be caught here.
#[test]
fn create_property_flag_is_repeatable_and_value_parsed() {
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct CreateHarness {
        #[command(flatten)]
        args: commands::node::CreateArgs,
    }

    let ok = CreateHarness::try_parse_from([
        "create",
        "--type",
        "customer",
        "--content",
        "Northwind Labs",
        "--property",
        "company_name=Northwind Labs",
        "--property",
        "employee_count=42",
    ])
    .expect("repeated --property must parse");
    assert_eq!(
        ok.args.properties,
        vec![
            (
                "company_name".to_string(),
                serde_json::json!("Northwind Labs")
            ),
            ("employee_count".to_string(), serde_json::json!(42)),
        ]
    );
}

/// `nodespace skill reset <key>` with none of `--guidance`/`--config`/`--all`
/// must be a parse-time usage error, not a silent full reset (ADR-072: reset
/// is the one destructive path in the system, so a bare invocation must
/// never guess a scope).
#[test]
fn skill_reset_requires_an_explicit_scope_flag() {
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct ResetHarness {
        #[command(flatten)]
        args: commands::skill::ResetArgs,
    }

    let err = ResetHarness::try_parse_from(["reset", "Research & Search"])
        .expect_err("bare `skill reset <key>` with no scope flag must be rejected");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);

    // Each scope flag on its own parses.
    for flag in ["--guidance", "--config", "--all"] {
        let ok = ResetHarness::try_parse_from(["reset", "Research & Search", flag])
            .unwrap_or_else(|e| panic!("{flag} alone must parse: {e}"));
        assert_eq!(ok.args.key, "Research & Search");
    }

    // --yes doesn't count as a scope flag on its own.
    let err = ResetHarness::try_parse_from(["reset", "Research & Search", "--yes"])
        .expect_err("--yes alone must not satisfy the scope requirement");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);

    // Combining --guidance and --config must parse and be equivalent to
    // --all, not conflict -- the ArgGroup allows multiple selections
    // (`.multiple(true)`), matching the documented "equivalent to passing
    // both flags" behavior for --all.
    let ok = ResetHarness::try_parse_from(["reset", "Research & Search", "--guidance", "--config"])
        .expect("--guidance and --config together must parse, not conflict");
    assert!(ok.args.guidance && ok.args.config);
}

/// End-to-end: `run_reset` against a real gRPC daemon, with `--yes` so it
/// never blocks on stdin. Proves the CLI -> ResetSeedNode RPC -> core
/// `reset_seed_node` path discards a live user edit to a seeded skill's
/// guidance and restores the current compiled template, using the real
/// production skill registry (not a synthetic template).
#[tokio::test]
async fn skill_reset_discards_modified_guidance_end_to_end() {
    const RESEARCH_AND_SEARCH: &str = "Research & Search";

    let (sock, shutdown, _tempdir, node_service) = spawn_test_daemon_with_seeded_skills().await;

    let skills = node_service
        .query_nodes_by_type("skill", true)
        .await
        .expect("query skills");
    let root = skills
        .iter()
        .find(|n| n.content == RESEARCH_AND_SEARCH)
        .expect("Research & Search must be seeded");
    let children = node_service
        .get_children(&root.id)
        .await
        .expect("get children");
    let target = children.first().expect("seeded guidance must have a child");

    node_service
        .update_node(
            &target.id,
            target.version,
            nodespace_core::models::NodeUpdate::new()
                .with_content("User's own guidance override.".to_string()),
        )
        .await
        .expect("user edit must succeed");

    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::skill::run_reset(
        &mut client,
        commands::skill::ResetArgs {
            key: RESEARCH_AND_SEARCH.to_string(),
            guidance: true,
            config: false,
            all: false,
            yes: true,
        },
        true,
    )
    .await
    .expect("run_reset must succeed");

    let children_after = node_service
        .get_children(&root.id)
        .await
        .expect("get children after reset");
    assert!(
        children_after
            .iter()
            .all(|n| n.content != "User's own guidance override."),
        "reset over gRPC must discard the user's edit"
    );

    let _ = shutdown.send(());
}

/// End-to-end: a shipped change to a skill the user edited is listed by
/// `nodespace seed pending`'s RPC, kept by `seed keep`, and replaced only by
/// `seed take` (ADR-094 §8). The daemon resolves the node to the real
/// compiled seed table, so `take` restores the production body.
#[tokio::test]
async fn seed_pending_update_is_listed_kept_and_taken_end_to_end() {
    const RESEARCH_AND_SEARCH: &str = "Research & Search";
    const USER_BODY: &str = "User's own guidance override.";

    let (sock, shutdown, _tempdir, node_service) = spawn_test_daemon_with_seeded_skills().await;
    let template = nodespace_agent::skill_pipeline::seed_skill_nodes()
        .into_iter()
        .find(|t| t.title == RESEARCH_AND_SEARCH)
        .expect("Research & Search is a seeded skill");
    let skill_id = template.id.clone();

    // The user rewrites the first line of the body.
    let children = node_service.get_children(&skill_id).await.expect("body");
    let target = children.first().expect("seeded guidance must have a child");
    node_service
        .update_node(
            &target.id,
            target.version,
            nodespace_core::models::NodeUpdate::new().with_content(USER_BODY.to_string()),
        )
        .await
        .expect("user edit must succeed");

    // A release ships a different body: what the next open reconciles.
    let reseed = |suffix: &str| {
        let mut changed = template.clone();
        changed.markdown_content.push_str(suffix);
        nodespace_core::markdown::prepare_nodes_from_template(&changed).expect("template expands")
    };
    node_service
        .seed_nodes_from_templates(vec![reseed("\n\nA line a later release added.")])
        .await
        .expect("reseed");

    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let pending = client
        .list_pending_seed_updates(nodespace_daemon::nodespace::ListPendingSeedUpdatesRequest {})
        .await
        .expect("list")
        .into_inner()
        .updates;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_id, skill_id);
    assert_eq!(pending[0].node_type, "skill");
    assert_eq!(pending[0].title, RESEARCH_AND_SEARCH);
    assert_eq!(pending[0].aspect, "guidance");

    let detail = client
        .get_pending_seed_update(nodespace_daemon::nodespace::PendingSeedUpdateRef {
            node_id: skill_id.clone(),
            aspect: "guidance".to_string(),
        })
        .await
        .expect("show")
        .into_inner();
    assert!(detail.yours.contains(USER_BODY), "{}", detail.yours);
    assert!(!detail.shipped.contains(USER_BODY), "{}", detail.shipped);
    assert!(!detail.shipped.is_empty());

    let item = |aspect: commands::seed::AspectArgs| commands::seed::ItemArgs {
        item: RESEARCH_AND_SEARCH.to_string(),
        aspect,
    };

    // Keep: the edit stays and nothing is pending.
    commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Keep(item(commands::seed::AspectArgs::default())),
        true,
    )
    .await
    .expect("keep must succeed");
    let body = |nodes: Vec<nodespace_core::models::Node>| -> Vec<String> {
        nodes.into_iter().map(|n| n.content).collect()
    };
    let after_keep = body(node_service.get_children(&skill_id).await.expect("body"));
    assert!(after_keep.iter().any(|line| line == USER_BODY));
    assert!(node_service
        .list_pending_seed_updates()
        .await
        .expect("list")
        .is_empty());
    // With nothing pending there is nothing to take.
    let err = commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Take(commands::seed::TakeArgs {
            item: item(commands::seed::AspectArgs::default()),
            yes: true,
        }),
        true,
    )
    .await
    .expect_err("take with nothing pending must fail");
    assert!(err.to_string().contains("No shipped update is pending"));

    // The shipped body changes again; this time the user takes it.
    node_service
        .seed_nodes_from_templates(vec![reseed("\n\nA line a still later release added.")])
        .await
        .expect("reseed");
    commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Take(commands::seed::TakeArgs {
            item: item(commands::seed::AspectArgs {
                guidance: true,
                ..Default::default()
            }),
            yes: true,
        }),
        true,
    )
    .await
    .expect("take must succeed");
    let after_take = body(node_service.get_children(&skill_id).await.expect("body"));
    assert!(
        after_take.iter().all(|line| line != USER_BODY),
        "take must replace the user's body"
    );
    assert!(node_service
        .list_pending_seed_updates()
        .await
        .expect("list")
        .is_empty());

    let _ = shutdown.send(());
}

/// End-to-end: a shipped change to the context paths of a built-in type the
/// user changed them on is listed, shown, kept by `seed keep` and replaced
/// only by `seed take --context-paths` (ADR-094 §8). The daemon resolves the
/// type to this build's definition, so `take` restores the paths it ships.
#[tokio::test]
async fn seed_pending_context_paths_update_is_listed_kept_and_taken_end_to_end() {
    use nodespace_core::models::core_schemas::get_core_schemas;

    let (sock, shutdown, _tempdir, node_service) = spawn_test_daemon_with_seeded_skills().await;
    let shipped = ["spec", "plan", "decisions", "spec.decisions", "project"];
    let theirs = ["spec", "decisions", "spec.decisions", "project"];
    let stored = || async {
        node_service
            .get_schema_node("task")
            .await
            .expect("read task")
            .expect("task is seeded")
            .context_paths
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    };

    // The user removes a path, then a build shipping other paths opens the
    // database: what its reconciliation does.
    nodespace_core::schema::handle_update_schema(
        &node_service,
        serde_json::json!({ "schema_id": "task", "remove_context_paths": ["plan"] }),
    )
    .await
    .expect("user edit must succeed");
    let other_build = |paths: &[&str]| {
        let mut schemas = get_core_schemas();
        schemas
            .iter_mut()
            .find(|schema| schema.envelope.id == "task")
            .expect("task is a core schema")
            .context_paths = paths.iter().map(|path| path.parse().unwrap()).collect();
        schemas
    };
    node_service
        .reconcile_core_context_paths(&other_build(&["project"]))
        .await
        .expect("reconcile");

    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let pending = client
        .list_pending_seed_updates(nodespace_daemon::nodespace::ListPendingSeedUpdatesRequest {})
        .await
        .expect("list")
        .into_inner()
        .updates;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_id, "task");
    assert_eq!(pending[0].node_type, "schema");
    assert_eq!(pending[0].aspect, "context_paths");
    assert!(pending[0].shipped_available);

    let detail = client
        .get_pending_seed_update(nodespace_daemon::nodespace::PendingSeedUpdateRef {
            node_id: "task".to_string(),
            aspect: "context_paths".to_string(),
        })
        .await
        .expect("show")
        .into_inner();
    assert_eq!(detail.shipped, shipped.join("\n"));
    assert_eq!(detail.yours, theirs.join("\n"));

    let item = |aspect: commands::seed::AspectArgs| commands::seed::ItemArgs {
        item: "task".to_string(),
        aspect,
    };
    commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Show(item(commands::seed::AspectArgs::default())),
        true,
    )
    .await
    .expect("show must succeed");

    // Keep: their paths stay and nothing is pending.
    commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Keep(item(commands::seed::AspectArgs::default())),
        true,
    )
    .await
    .expect("keep must succeed");
    assert_eq!(stored().await, theirs);
    assert!(node_service
        .list_pending_seed_updates()
        .await
        .expect("list")
        .is_empty());
    let err = commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Take(commands::seed::TakeArgs {
            item: item(commands::seed::AspectArgs::default()),
            yes: true,
        }),
        true,
    )
    .await
    .expect_err("take with nothing pending must fail");
    assert!(err.to_string().contains("No shipped update is pending"));

    // What ships changes again; this time the user takes it.
    node_service
        .reconcile_core_context_paths(&get_core_schemas())
        .await
        .expect("reconcile");
    commands::seed::run(
        &mut client,
        commands::seed::SeedAction::Take(commands::seed::TakeArgs {
            item: item(commands::seed::AspectArgs {
                context_paths: true,
                ..Default::default()
            }),
            yes: true,
        }),
        true,
    )
    .await
    .expect("take must succeed");
    assert_eq!(stored().await, shipped);
    assert!(node_service
        .list_pending_seed_updates()
        .await
        .expect("list")
        .is_empty());

    let _ = shutdown.send(());
}

/// A reset scope flag against a seed key that doesn't exist must report
/// "not found" rather than erroring or silently succeeding.
#[tokio::test]
async fn skill_reset_reports_not_found_for_an_unknown_key() {
    let (sock, shutdown, _tempdir, _node_service) = spawn_test_daemon_with_seeded_skills().await;

    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::skill::run_reset(
        &mut client,
        commands::skill::ResetArgs {
            key: "Not A Real Skill".to_string(),
            guidance: false,
            config: false,
            all: true,
            yes: true,
        },
        true,
    )
    .await
    .expect("run_reset must not error on an unknown key -- it should report not-found");

    let _ = shutdown.send(());
}

/// Like [`spawn_test_daemon`], but wires a real `PlaybookLifecycleManager`
/// into the served `NodeServiceImpl` (via `with_playbook_lifecycle`), so
/// `nodespace playbook get-workflow-state` has something to evaluate against
/// over the real gRPC transport. Also returns the raw `CoreNodeService` and
/// the lifecycle handle so a test can create a play node and activate it
/// directly, mirroring how `PlaybookEngine::handle_play_created` would.
pub(crate) async fn spawn_test_daemon_with_playbook() -> (
    PathBuf,
    oneshot::Sender<()>,
    TempDir,
    Arc<CoreNodeService>,
    Arc<std::sync::RwLock<nodespace_core::playbook::PlaybookLifecycleManager>>,
) {
    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("test-daemon.sock");

    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("failed to open SqliteStore"),
    );
    let node_service = Arc::new(
        CoreNodeService::new(&mut store)
            .await
            .expect("failed to build NodeService"),
    );
    let lifecycle = Arc::new(std::sync::RwLock::new(
        nodespace_core::playbook::PlaybookLifecycleManager::new(),
    ));
    let service = NodeServiceImpl::new(
        node_service.clone(),
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    )
    .with_playbook_lifecycle(lifecycle.clone());

    let listener = UnixListener::bind(&sock_path).expect("failed to bind test UDS socket");
    let incoming = UnixListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect(&sock_path, DatabaseIdInterceptor::none())
            .await
            .is_ok()
        {
            return (sock_path, shutdown_tx, tempdir, node_service, lifecycle);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// `nodespace playbook list` reduces to `QueryNodesSimple` (per ADR-035
/// capability parity) — this proves that reduction actually reaches the real
/// daemon and returns the play nodes it should, not just that the CLI command
/// builds a well-formed request.
#[tokio::test]
async fn playbook_list_round_trip() {
    let (sock, shutdown, _tempdir, node_service, _lifecycle) =
        spawn_test_daemon_with_playbook().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // No plays yet: list must return empty without error.
    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::List(commands::playbook::PlaybookListArgs {
            include_archived: false,
        }),
        true,
    )
    .await
    .expect("playbook list (empty)");

    let play = nodespace_core::models::Node::new(
        "play".to_string(),
        "Test Play".to_string(),
        serde_json::json!({ "rules": [] }),
    );
    let play_id = node_service
        .create_node(play)
        .await
        .expect("create play node");

    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::List(commands::playbook::PlaybookListArgs {
            include_archived: false,
        }),
        true,
    )
    .await
    .expect("playbook list (one play)");

    // A disabled Play is switched off, not archived: it stays in the list.
    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::Disable(commands::playbook::PlaybookIdArgs {
            play_id: play_id.clone(),
        }),
        true,
    )
    .await
    .expect("playbook disable");

    let list_plays = |include_archived: bool| QueryNodesSimpleRequest {
        include_archived,
        id: None,
        mentioned_by: None,
        content_contains: None,
        title_contains: None,
        node_type: Some("play".to_string()),
        limit: 0,
        offset: 0,
        order_by: 0,
    };
    let listed = client
        .query_nodes_simple(list_plays(false))
        .await
        .expect("QueryNodesSimple")
        .into_inner();
    let disabled = listed
        .nodes
        .iter()
        .find(|n| n.id == play_id)
        .expect("a disabled Play is still listed");
    assert_eq!(
        commands::playbook::PlayState::of(&nodespace_cli::output::node_to_json(disabled)),
        commands::playbook::PlayState::Off
    );

    // A suspension the engine recorded is reported as that state, with the
    // reason in the Play's properties.
    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::Enable(commands::playbook::PlaybookIdArgs {
            play_id: play_id.clone(),
        }),
        true,
    )
    .await
    .expect("playbook enable");
    node_service
        .record_play_suspension(
            &play_id,
            nodespace_core::models::PlaySuspensionReason::ActionFailed,
            "boom",
        )
        .await
        .expect("record suspension");
    let listed = client
        .query_nodes_simple(list_plays(false))
        .await
        .expect("QueryNodesSimple")
        .into_inner();
    let suspended = nodespace_cli::output::node_to_json(
        listed
            .nodes
            .iter()
            .find(|n| n.id == play_id)
            .expect("a suspended Play is still listed"),
    );
    assert_eq!(
        commands::playbook::PlayState::of(&suspended),
        commands::playbook::PlayState::Suspended
    );
    assert_eq!(suspended["properties"]["suspended_reason"], "action_failed");
    assert_eq!(suspended["properties"]["suspended_message"], "boom");

    for include_archived in [false, true] {
        commands::playbook::run(
            &mut client,
            commands::playbook::PlaybookAction::List(commands::playbook::PlaybookListArgs {
                include_archived,
            }),
            include_archived,
        )
        .await
        .expect("playbook list");
    }

    let _ = shutdown.send(());
}

/// `nodespace playbook enable`/`disable` write the Play's `enabled` field
/// through the typed play update over the real gRPC transport. They never
/// touch `lifecycle_status`, and `enable` clears a suspension.
#[tokio::test]
async fn playbook_enable_disable_round_trip() {
    let (sock, shutdown, _tempdir, node_service, _lifecycle) =
        spawn_test_daemon_with_playbook().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let play = nodespace_core::models::Node::new(
        "play".to_string(),
        "Test Play".to_string(),
        serde_json::json!({ "rules": [] }),
    );
    let play_id = node_service
        .create_node(play)
        .await
        .expect("create play node");

    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::Disable(commands::playbook::PlaybookIdArgs {
            play_id: play_id.clone(),
        }),
        true,
    )
    .await
    .expect("playbook disable");

    let disabled = node_service
        .get_node(&play_id)
        .await
        .expect("get_node")
        .expect("play node still exists");
    assert_eq!(disabled.properties["play"]["enabled"], false);
    assert_eq!(disabled.lifecycle_status, "active");

    node_service
        .record_play_suspension(
            &play_id,
            nodespace_core::models::PlaySuspensionReason::CycleLimit,
            "boom",
        )
        .await
        .expect("record suspension");

    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::Enable(commands::playbook::PlaybookIdArgs {
            play_id: play_id.clone(),
        }),
        true,
    )
    .await
    .expect("playbook enable");

    let enabled = node_service
        .get_node(&play_id)
        .await
        .expect("get_node")
        .expect("play node still exists");
    assert_eq!(enabled.properties["play"]["enabled"], true);
    assert_eq!(enabled.lifecycle_status, "active");
    assert!(
        enabled.properties["play"]["suspended_reason"].is_null()
            && enabled.properties["play"]["suspended_at"].is_null(),
        "enable clears a suspension: {}",
        enabled.properties
    );

    // Enabling an already-enabled Play is not an error.
    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::Enable(commands::playbook::PlaybookIdArgs {
            play_id: play_id.clone(),
        }),
        false,
    )
    .await
    .expect("playbook enable (already on)");

    let _ = shutdown.send(());
}

/// A Play's rules are written through `node update --property rules=...` and
/// read back in the described shape (ADR-090 §1): the rule, each condition
/// and each action carry a description, and a write that changes a condition
/// under its stored description is refused with a message that names it.
#[tokio::test]
async fn play_rules_are_written_and_read_with_their_descriptions() {
    let (sock, shutdown, _tempdir, node_service, _lifecycle) =
        spawn_test_daemon_with_playbook().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let rules = |expr: &str, description: &str| {
        serde_json::json!([{
            "name": "greet",
            "description": "Greet a new note",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "text" } },
            "conditions": [{ "expr": expr, "description": description }],
            "actions": [{
                "action_type": "update_node",
                "description": "Mark the note as seen",
                "params": { "node_id": "{trigger.node.id}", "properties": { "seen": true } }
            }]
        }])
    };
    let play_id = node_service
        .create_node(nodespace_core::models::Node::new(
            "play".to_string(),
            "Greeter".to_string(),
            serde_json::json!({ "rules": rules("node.content == 'hello'", "The note says hello") }),
        ))
        .await
        .expect("create play node");

    let update = |rules: serde_json::Value| {
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: play_id.clone(),
            content: None,
            properties: vec![("rules".to_string(), rules)],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: None,
            dry_run: false,
        })
    };

    let stale = node_run(
        &mut client,
        update(rules("node.content == 'hi'", "The note says hello")),
        true,
    )
    .await
    .expect_err("a changed expression under its stored description is refused");
    let message = format!("{stale:#}");
    assert!(
        message.contains(
            "rule `greet`, condition 1: its expression changed and its description didn't"
        ),
        "{message}"
    );

    let bare = node_run(
        &mut client,
        update(serde_json::json!([{
            "name": "greet",
            "description": "Greet a new note",
            "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "text" } },
            "conditions": ["node.content == 'hi'"]
        }])),
        true,
    )
    .await
    .expect_err("a bare-string condition is refused");
    let message = format!("{bare:#}");
    assert!(
        message.contains("rule[0] ('greet'): conditions[0]"),
        "{message}"
    );

    node_run(
        &mut client,
        update(rules("node.content == 'hi'", "The note says hi")),
        true,
    )
    .await
    .expect("a changed expression with a new description saves");

    let listed = client
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: false,
            id: Some(play_id.clone()),
            mentioned_by: None,
            content_contains: None,
            title_contains: None,
            node_type: Some("play".to_string()),
            limit: 0,
            offset: 0,
            order_by: 0,
        })
        .await
        .expect("QueryNodesSimple")
        .into_inner();
    let printed = nodespace_cli::output::node_to_json(&listed.nodes[0]);
    let rule = &printed["properties"]["rules"][0];
    assert_eq!(rule["description"], "Greet a new note");
    assert_eq!(
        rule["conditions"][0],
        serde_json::json!({ "expr": "node.content == 'hi'", "description": "The note says hi" })
    );
    assert_eq!(rule["actions"][0]["description"], "Mark the note as seen");

    let _ = shutdown.send(());
}

/// `nodespace playbook get-workflow-state` over the real gRPC transport: a
/// play activated directly on the served lifecycle manager is found and
/// evaluated against a real node.
#[tokio::test]
async fn playbook_get_workflow_state_round_trip() {
    let (sock, shutdown, _tempdir, node_service, lifecycle) =
        spawn_test_daemon_with_playbook().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    {
        let mut lm = lifecycle.write().unwrap();
        let play = nodespace_core::models::Node::new(
            "play".to_string(),
            "Test Play".to_string(),
            serde_json::json!({
                "play": {
                    "rules": [{
                        "name": "r1",
                        "description": "Test rule",
                        "trigger": { "type": "graph_event", "on": "node_created", "select": { "target_type": "text" } },
                        "conditions": [{ "expr": "node.content == 'hello'", "description": "Test condition" }],
                        "actions": []
                    }]
                }
            }),
        );
        lm.activate_play(&play).expect("activate play");
    }

    let node = nodespace_core::models::Node::new(
        "text".to_string(),
        "hello".to_string(),
        serde_json::json!({}),
    );
    let node_id = node_service
        .create_node(node)
        .await
        .expect("create text node");

    commands::playbook::run(
        &mut client,
        commands::playbook::PlaybookAction::GetWorkflowState(
            commands::playbook::GetWorkflowStateArgs {
                node_id: node_id.clone(),
            },
        ),
        true,
    )
    .await
    .expect("playbook get-workflow-state");

    let _ = shutdown.send(());
}

/// With no `--database`, routing must resolve the daemon's default to a
/// concrete id and stamp it — not send an unstamped request and let the daemon
/// pick.
///
/// Both reach the same database today, so this is not about which rows come
/// back. It is about whether the CLI can *say* which database it read. An
/// unstamped request is resolved daemon-side against whatever
/// `registry.default_database` holds when it arrives, so the CLI never learns
/// the target and cannot report it. A write that went elsewhere — an agent turn
/// runs against its own event watcher's database, not the default — then reads
/// back as a well-formed empty result, indistinguishable from a write that
/// never happened. That is how a schema written by an agent can appear to
/// vanish from a CLI search.
#[tokio::test]
async fn routing_with_no_selection_pins_the_resolved_default_database_id() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;

    let mut db = connect_database(&sock).await.expect("connect database");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("list databases")
        .into_inner();
    let default_id = listed
        .databases
        .iter()
        .find(|d| d.is_default)
        .map(|d| d.id.clone())
        .expect("the harness seeds a default database");

    let (_interceptor, resolved) = nodespace_cli::resolve_routing(&sock, None)
        .await
        .expect("resolve routing with no selection");

    assert_eq!(
        resolved.as_deref(),
        Some(default_id.as_str()),
        "no --database must resolve to the default's concrete id, so the target \
         is a fact the CLI knows rather than one the daemon decides per request"
    );

    // An explicit selection of that same database must land on the same id —
    // the two paths agree rather than one of them being special.
    let (_interceptor, explicit) = nodespace_cli::resolve_routing(&sock, Some(&default_id))
        .await
        .expect("resolve routing with explicit selection");
    assert_eq!(
        explicit.as_deref(),
        Some(default_id.as_str()),
        "selecting the default by id must resolve to the same id as selecting nothing"
    );

    let _ = shutdown.send(());
}

/// `node create --type collection` makes the collection the name identifies:
/// the same id an import or the app's create derives, with the description in
/// the collection bucket. `node update --property description=…` changes it.
#[tokio::test]
async fn node_create_and_update_set_a_collection_description() {
    use nodespace_core::services::collection_service::deterministic_collection_id;

    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let create = |name: &str| {
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "collection".into(),
            content: Some(name.into()),
            parent: None,
            properties: vec![("description".into(), serde_json::json!("Accounts we bill"))],
            collections: vec![],
            collection_ids: vec![],
        })
    };
    node_run(&mut client, create("Clients"), true)
        .await
        .expect("create collection");

    let id = deterministic_collection_id("Clients");
    let stored_description = |raw: &mut NodeClient| {
        let mut raw = raw.clone();
        let id = id.clone();
        async move {
            let node = raw
                .get_node(GetNodeRequest { node_id: id })
                .await
                .expect("the collection exists at its deterministic id")
                .into_inner()
                .node_data
                .expect("node_data");
            let props: serde_json::Value =
                serde_json::from_str(&node.properties).expect("parse properties");
            assert!(
                props.get("description").is_none(),
                "no flat description key: {props}"
            );
            props["collection"]["description"].clone()
        }
    };
    assert_eq!(stored_description(&mut raw).await, "Accounts we bill");

    // A second create of the same name is the same collection, so it is
    // refused rather than stored as a duplicate under another id.
    let err = node_run(&mut client, create("clients"), true)
        .await
        .expect_err("a duplicate collection name is refused");
    assert!(
        format!("{err:#}").contains("Already exists"),
        "expected an already-exists error, got: {err:#}"
    );

    node_run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            properties_json: None,
            id: id.clone(),
            content: None,
            properties: vec![(
                "description".into(),
                serde_json::json!("Accounts we bill, one page each"),
            )],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
            version: None,
            dry_run: false,
        }),
        true,
    )
    .await
    .expect("update description");
    assert_eq!(
        stored_description(&mut raw).await,
        "Accounts we bill, one page each"
    );

    let _ = shutdown.send(());
}

/// The `(node_id, version)` pairs a journal holds, oldest first.
fn journal_entries(path: &std::path::Path) -> Vec<(String, i64)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
            assert_eq!(entry["database"], "db-1");
            (
                entry["node_id"].as_str().expect("node_id").to_string(),
                entry["version"].as_i64().expect("version"),
            )
        })
        .collect()
}

fn update_args(id: &str, content: &str) -> commands::node::UpdateArgs {
    commands::node::UpdateArgs {
        id: id.to_string(),
        content: Some(content.to_string()),
        properties: vec![],
        properties_json: None,
        collections: vec![],
        collection_ids: vec![],
        remove_collection_ids: vec![],
        version: None,
        dry_run: false,
    }
}

/// The version `id` was last journaled at.
fn journaled_version(path: &std::path::Path, id: &str) -> Option<i64> {
    journal_entries(path)
        .iter()
        .rev()
        .find(|(entry, _)| entry == id)
        .map(|(_, version)| *version)
}

#[tokio::test]
async fn every_node_write_is_journaled_at_the_version_it_leaves_the_node() {
    let (sock, shutdown, tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");
    let path = tempdir.path().join("journal.jsonl");
    let journal = WriteJournal::at(Some(path.clone()), "db-1");

    let parent = seed_text_node(&mut raw, "parent", None).await;
    let other = seed_text_node(&mut raw, "other", None).await;
    let node = seed_text_node(&mut raw, "node", Some(&parent)).await;

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(update_args(&node, "edited")),
        true,
        &journal,
    )
    .await
    .expect("update");
    assert_eq!(
        journaled_version(&path, &node),
        Some(node_version(&mut raw, &node).await)
    );

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Move(commands::node::MoveArgs {
            parent: Some(other.clone()),
            ..move_args(&node)
        }),
        true,
        &journal,
    )
    .await
    .expect("move");
    assert_eq!(
        journaled_version(&path, &node),
        Some(node_version(&mut raw, &node).await)
    );

    commands::node::run(
        &mut client,
        commands::node::NodeAction::BatchUpdate(commands::node::BatchUpdateArgs {
            updates: serde_json::json!([{ "node_id": node, "content": "again" }]).to_string(),
        }),
        true,
        &journal,
    )
    .await
    .expect("batch-update");
    assert_eq!(
        journaled_version(&path, &node),
        Some(node_version(&mut raw, &node).await)
    );

    let before = journal_entries(&path).len();
    commands::node::run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            properties_json: None,
            node_type: "text".into(),
            content: Some("made".into()),
            parent: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
        &journal,
    )
    .await
    .expect("create");
    assert_eq!(journal_entries(&path).len(), before + 1);

    // A write the daemon refuses changes nothing, so it records nothing.
    let before = journal_entries(&path).len();
    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            version: Some(1_000),
            ..update_args(&other, "refused")
        }),
        true,
        &journal,
    )
    .await
    .expect_err("a stale version is refused");
    assert_eq!(journal_entries(&path).len(), before);

    // A delete records nothing: the node is gone, and a watcher is told so by
    // the not-found read, not by a version.
    let delete = |version, descendants| {
        commands::node::NodeAction::Delete(commands::node::DeleteArgs {
            id: node.clone(),
            version,
            descendants,
            routing: Vec::new(),
        })
    };
    commands::node::run(&mut client, delete(None, None), true, &journal)
        .await
        .expect("preview");
    assert_eq!(journal_entries(&path).len(), before);
    let version = node_version(&mut raw, &node).await;
    commands::node::run(&mut client, delete(Some(version), Some(0)), true, &journal)
        .await
        .expect("delete");
    assert_eq!(journal_entries(&path).len(), before);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn a_context_read_of_a_missing_node_is_an_error_naming_it() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = node_run(
        &mut client,
        commands::node::NodeAction::Context(commands::node::ContextArgs {
            id: "00000000-0000-4000-8000-000000000000".into(),
            paths: Vec::new(),
            version_only: true,
        }),
        true,
    )
    .await
    .expect_err("a missing node is an error");

    assert_eq!(
        format!("{err:#}"),
        "Not found: 00000000-0000-4000-8000-000000000000"
    );

    let _ = shutdown.send(());
}
