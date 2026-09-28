//! ADR-048 commits-per-edit assertion — validates the backend half of the
//! write-coalescing fix (frontend serialization lives in
//! `shared-node-store.svelte.ts`'s `SimplePersistenceCoordinator`; that half
//! already has frontend coverage). What belongs at the Tauri seam is proof
//! that the real `update_node` command, called the way a correctly
//! sequenced coalescing writer calls it, produces exactly one `UpdateNode`
//! commit (one version increment) per logical edit — and that OCC genuinely
//! rejects a stale version, so a coalescing regression that reads a version
//! before the prior write's confirmation lands would be caught here, not
//! just hoped away.

use nodespace_app_lib::commands::nodes::create_node;
use nodespace_app_lib::commands::nodes::{update_node, CreateNodeInput};
use nodespace_app_lib::types::NodeUpdate;
use nodespace_app_test_support::{SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT};
use serde_json::json;

fn text_input(id: &str, content: &str) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "text".to_string(),
        content: content.to_string(),
        parent_id: None,
        insert_position: None,
        properties: json!({}),
    }
}

/// Simulates typing a word as a coalescing writer would call it: one
/// `update_node` per pause, always reading the version from the PRIOR
/// commit's confirmed response (never from a stale local guess). Asserts
/// version increments by exactly one per call — one commit per coalesced
/// edit, not one per keystroke.
#[tokio::test]
async fn one_coalesced_edit_produces_exactly_one_version_increment() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&id, ""))
        .await
        .expect("create_node failed");

    // "Typing a word": five keystrokes coalesce into ONE commit under a
    // correctly sequenced writer, so only one update_node call is made here
    // — the assertion is that this single call moves the version by
    // exactly 1, not that five keystrokes produce five calls.
    let result = update_node(
        state.clone(),
        id.clone(),
        1,
        NodeUpdate {
            content: Some("hello".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("update_node failed");

    assert_eq!(
        result["version"],
        json!(2),
        "a single coalesced edit must move the version by exactly one"
    );
    assert_eq!(result["content"], json!("hello"));
}

/// A correctly sequenced writer for a SECOND coalesced edit reads the
/// version from the first edit's confirmed response (2), not from a stale
/// pre-confirmation guess (1) — this is the "latest-wins serial writer that
/// reads the version only after prior confirmation" the fix mandates.
/// Confirms that path also increments by exactly one, and that reusing the
/// now-stale version 1 is rejected — proving self-conflict is structurally
/// impossible only when the version is sourced correctly, not by accident.
#[tokio::test]
async fn second_coalesced_edit_uses_the_confirmed_version_not_a_stale_guess() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&id, ""))
        .await
        .expect("create_node failed");

    let first = update_node(
        state.clone(),
        id.clone(),
        1,
        NodeUpdate {
            content: Some("hello".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("first update_node failed");
    let confirmed_version = first["version"].as_i64().expect("version must be a number");
    assert_eq!(confirmed_version, 2);

    // Correct behavior: the second coalesced edit reads the CONFIRMED
    // version from the first commit's response.
    let second = update_node(
        state.clone(),
        id.clone(),
        confirmed_version,
        NodeUpdate {
            content: Some("hello world".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("second update_node (using confirmed version) failed");
    assert_eq!(
        second["version"],
        json!(3),
        "second coalesced edit must move the version by exactly one more"
    );

    // The regression this guards against: re-using the now-stale pre-confirmation
    // version (1) for a write against a node already at version 3 must be
    // rejected by OCC, not silently accepted — this is what makes
    // self-conflict structurally impossible rather than accidental.
    let stale_attempt = update_node(
        state.clone(),
        id.clone(),
        1,
        NodeUpdate {
            content: Some("stale write".to_string()),
            ..Default::default()
        },
    )
    .await;
    assert!(
        stale_attempt.is_err(),
        "a write against a stale pre-confirmation version must be OCC-rejected"
    );
    assert_eq!(stale_attempt.unwrap_err().code, "VERSION_CONFLICT");
}
