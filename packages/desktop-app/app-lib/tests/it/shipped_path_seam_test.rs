//! Shipped-path behaviours of the Tauri command layer that the retired
//! browser e2e tier used to cover, asserted here at the real seam: command
//! handler -> `GrpcClient` -> gRPC over UDS -> a real headless `nodespaced`.
//!
//! - what `create_node` / `move_node` / `move_children_to_parent` hand back
//!   about where the store placed an edge (the frontend applies it without
//!   waiting for an event, since its own relationship events are echo-suppressed),
//! - that `get_children_tree` serves typed nodes, and
//! - the watcher's `node:deleted` event and its recovery when it was started
//!   before the daemon was reachable.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nodespace_app_lib::commands::nodes::{
    create_node, delete_node, get_children_tree, move_children_to_parent, move_node,
    ChildMoveInput, CreateNodeInput, InsertPositionInput,
};
use nodespace_app_lib::services::GrpcClient;
use nodespace_app_lib::watcher;
use nodespace_app_test_support::{
    hold_connect_mutex_and_socket_env, SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT,
};
use serde_json::{json, Value};
use tauri::{Listener, Manager};
use tokio_util::sync::CancellationToken;

fn input(
    id: &str,
    node_type: &str,
    parent_id: Option<&str>,
    position: Option<InsertPositionInput>,
    properties: Value,
) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: node_type.to_string(),
        content: id.to_string(),
        parent_id: parent_id.map(str::to_string),
        insert_position: position,
        properties,
        collection_id: None,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Seeds a text node under `parent` (or at the root) and returns the order its
/// edge was given, when it has one.
async fn seed(
    state: &tauri::State<'_, GrpcClient>,
    id: &str,
    parent: Option<&str>,
    position: Option<InsertPositionInput>,
) -> Option<f64> {
    create_node(
        state.clone(),
        input(id, "text", parent, position, json!({})),
    )
    .await
    .expect("create_node failed")
    .placement
    .map(|p| p.order)
}

/// `move_children_to_parent` returns one store order per transferred edge, in
/// the order the children were given, and the keys keep that order.
#[tokio::test]
async fn move_children_to_parent_returns_the_transferred_edges_orders() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let (old_parent, new_parent) = (new_id(), new_id());
    let (c1, c2, c3) = (new_id(), new_id(), new_id());
    seed(&state, &old_parent, None, None).await;
    seed(&state, &new_parent, None, None).await;
    for c in [&c1, &c2, &c3] {
        seed(&state, c, Some(&old_parent), Some(InsertPositionInput::End)).await;
    }

    let moved = move_children_to_parent(
        state.clone(),
        new_parent.clone(),
        [&c1, &c2, &c3]
            .iter()
            .map(|id| ChildMoveInput {
                node_id: (*id).clone(),
                version: 1,
            })
            .collect(),
    )
    .await
    .expect("move_children_to_parent failed");

    let ids: Vec<&str> = moved.orders.iter().map(|o| o.node_id.as_str()).collect();
    assert_eq!(
        ids,
        [c1.as_str(), c2.as_str(), c3.as_str()],
        "one order per transferred edge, in the order given"
    );
    assert!(
        moved.orders.windows(2).all(|w| w[0].order < w[1].order),
        "the store's keys keep the given order: {:?}",
        moved.orders
    );
    assert_eq!(moved.nodes.len(), 3);
    assert!(moved.nodes.iter().all(|n| n["version"] == json!(2)));
}

/// A tree read serves typed nodes: a person child exposes its typed field at
/// the top level and carries no storage bucket under `properties`.
#[tokio::test]
async fn get_children_tree_serves_typed_nodes() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let root = new_id();
    let person = new_id();
    seed(&state, &root, None, None).await;
    let mut person_input = input(
        &person,
        "person",
        Some(&root),
        Some(InsertPositionInput::End),
        json!({ "first_name": "Ada", "last_name": "Lovelace" }),
    );
    // A person takes its name from its fields, so it carries no content.
    person_input.content = String::new();
    create_node(state.clone(), person_input)
        .await
        .expect("create person failed");

    let tree = get_children_tree(state.clone(), root.clone())
        .await
        .expect("get_children_tree failed");
    assert_eq!(tree["id"], json!(root));
    let child = &tree["children"][0];
    assert_eq!(child["id"], json!(person));
    assert_eq!(child["nodeType"], json!("person"));
    assert_eq!(child["firstName"], json!("Ada"));
    assert_eq!(child["lastName"], json!("Lovelace"));
    assert_eq!(
        child["properties"],
        json!({}),
        "a tree node carries neither the storage bucket nor a copy of its typed fields"
    );
}

/// `create_node` under a parent returns the placement of the new edge: the
/// parent, a store order that respects the requested position, and no
/// re-spread when the keys had room.
#[tokio::test]
async fn create_node_under_a_parent_returns_its_placement_without_a_respread() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let parent = new_id();
    seed(&state, &parent, None, None).await;

    let mut placements = Vec::new();
    for position in [
        InsertPositionInput::End,
        InsertPositionInput::End,
        InsertPositionInput::Beginning,
    ] {
        let created = create_node(
            state.clone(),
            input(&new_id(), "text", Some(&parent), Some(position), json!({})),
        )
        .await
        .expect("create_node failed");
        placements.push(created.placement.expect("a child has a placement"));
    }

    for p in &placements {
        assert_eq!(p.parent_id, parent);
        assert!(
            p.respread.is_empty(),
            "no sibling is rewritten while the keys have room: {:?}",
            p.respread
        );
    }
    assert!(
        placements[0].order < placements[1].order,
        "End appends after the previous last child"
    );
    assert!(
        placements[2].order < placements[0].order,
        "Beginning sorts before the first child"
    );
}

/// `move_node` reports where the moved edge landed: the new parent, a key
/// that puts it where it was asked to go, and `None` for a move to the root.
#[tokio::test]
async fn move_node_returns_placement_that_matches_the_requested_position() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let (parent, target, first, second, mover) = (new_id(), new_id(), new_id(), new_id(), new_id());
    seed(&state, &parent, None, None).await;
    seed(&state, &target, None, None).await;
    let first_order = seed(
        &state,
        &first,
        Some(&target),
        Some(InsertPositionInput::End),
    )
    .await
    .unwrap();
    let second_order = seed(
        &state,
        &second,
        Some(&target),
        Some(InsertPositionInput::End),
    )
    .await
    .unwrap();
    seed(
        &state,
        &mover,
        Some(&parent),
        Some(InsertPositionInput::End),
    )
    .await;

    // Between two siblings: after `first`.
    let between = move_node(
        state.clone(),
        mover.clone(),
        1,
        Some(target.clone()),
        Some(InsertPositionInput::After {
            sibling_id: first.clone(),
        }),
    )
    .await
    .expect("move after a sibling failed");
    let p = between
        .placement
        .expect("a move under a parent has a placement");
    assert_eq!(p.parent_id, target);
    assert!(p.respread.is_empty());
    assert!(
        first_order < p.order && p.order < second_order,
        "after `first` sorts between the siblings: {first_order} < {} < {second_order}",
        p.order
    );
    assert_eq!(between.node["version"], json!(2));

    // To the front of the same parent.
    let front = move_node(
        state.clone(),
        mover.clone(),
        2,
        Some(target.clone()),
        Some(InsertPositionInput::Beginning),
    )
    .await
    .expect("move to beginning failed");
    let p = front.placement.expect("placement");
    assert_eq!(p.parent_id, target);
    assert!(
        p.order < first_order,
        "Beginning sorts before every sibling"
    );

    // To the root: no parent edge, so no placement.
    let to_root = move_node(state.clone(), mover.clone(), 3, None, None)
        .await
        .expect("move to root failed");
    assert!(
        to_root.placement.is_none(),
        "a root node has no parent edge"
    );
}

/// `(event name, id, node type)` for every `node:*` event the window saw.
type Seen = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

fn record(handle: &tauri::AppHandle<tauri::test::MockRuntime>, names: &[&'static str]) -> Seen {
    let names: Vec<&'static str> = names.to_vec();
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    for name in names {
        let seen = seen.clone();
        handle.listen(name, move |event| {
            if let Ok(v) = serde_json::from_str::<Value>(event.payload()) {
                if let Some(id) = v["id"].as_str() {
                    seen.lock().unwrap().push((
                        name.to_string(),
                        id.to_string(),
                        v["nodeType"].as_str().map(str::to_string),
                    ));
                }
            }
        });
    }
    seen
}

fn saw(seen: &Seen, name: &str, id: &str) -> Option<(String, String, Option<String>)> {
    seen.lock()
        .unwrap()
        .iter()
        .find(|(n, i, _)| n == name && i == id)
        .cloned()
}

/// A delete made by another window reaches this window's watcher as
/// `node:deleted`, carrying the node's id and type (consumers apply type-aware
/// cleanup without fetching the already-deleted node).
#[tokio::test]
async fn a_deleted_node_reaches_the_watcher_as_node_deleted_with_its_type() {
    let daemon = SpawnedDaemon::spawn();
    let window = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let other = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = window.client_state();
    let handle = window.handle();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let seen = record(&handle, &["node:deleted"]);
    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle.clone(),
        (*state).clone(),
        cancel_token.child_token(),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let id = new_id();
    seed(&other.client_state(), &id, None, None).await;
    delete_node(other.client_state(), id.clone(), 1)
        .await
        .expect("delete_node failed");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let event = loop {
        if let Some(e) = saw(&seen, "node:deleted", &id) {
            break e;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for node:deleted; saw {:?}",
            seen.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        event.2.as_deref(),
        Some("text"),
        "node:deleted carries the type"
    );

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}

/// The app starts its watcher before the daemon has bound its socket. Writes
/// made once the daemon is up must still reach it: the watcher keeps
/// reconnecting through the not-ready window instead of giving up.
#[tokio::test]
async fn a_watcher_started_before_the_daemon_is_ready_receives_later_writes() {
    let daemon = SpawnedDaemon::spawn();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;
    let client = GrpcClient::connect_lazy();
    let app = tauri::test::mock_app();
    app.manage(client);
    let state = app.state::<GrpcClient>();
    let handle = app.handle().clone();

    let seen = record(&handle, &["node:created"]);
    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle.clone(),
        (*state).clone(),
        cancel_token.child_token(),
    ));

    let status = nodespace_app_lib::daemon_setup::wait_for_daemon(
        &daemon.socket_path,
        DAEMON_CONNECT_TIMEOUT,
    )
    .await;
    assert_eq!(
        status,
        nodespace_app_lib::daemon_setup::DaemonStatus::Healthy,
        "daemon never became healthy"
    );

    // A different client, so the watcher's own-window echo suppression is not
    // what is under test. Writes made before the watcher has reconnected are
    // not replayed, so keep writing until one is seen.
    let writer = tauri::test::mock_app();
    writer.manage(GrpcClient::connect_lazy());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut written = Vec::new();
    loop {
        let id = new_id();
        create_node(
            writer.state::<GrpcClient>(),
            input(&id, "text", None, None, json!({})),
        )
        .await
        .expect("create_node failed");
        written.push(id);
        if written
            .iter()
            .any(|i| saw(&seen, "node:created", i).is_some())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no write reached a watcher started before the daemon was ready; saw {:?}",
            seen.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}
