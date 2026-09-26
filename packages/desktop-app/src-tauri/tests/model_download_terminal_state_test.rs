//! ADR-048 priority flow 6 — model-download terminal state, including
//! cancel, against a real headless daemon.
//!
//! `chat_model_download` (the `#[tauri::command]`) takes `app: AppHandle`
//! hardcoded to the real `Wry` runtime — like every other `AppHandle`-taking
//! command in this codebase, it is not generic over `Runtime`, so it cannot
//! be called through `tauri::test`'s `MockRuntime` (confirmed: this is a
//! structural constraint of the command signature, not a gap in the test
//! harness). `chat_model_download`'s body is a thin proxy — open the
//! `DownloadModel` gRPC stream and forward each `ModelLoadProgressEvent` to
//! an `app.emit` call — so the download-terminal-state contract that
//! actually matters (does the daemon's stream reach a clean ready/error
//! terminal state, does cancel leave nothing lingering) is exercised here
//! directly against `LocalAgentServiceClient`, the same client type
//! `GrpcClient::local_agent_client()` hands the command. The cancel/list/
//! load/unload commands take only `State<GrpcClient>` (no `AppHandle`), so
//! those run through the real `#[tauri::command]` functions as usual.
//!
//! Uses `gemma-4-e4b-q4km` — the default model, so already present under
//! `~/.nodespace/models/` on a machine that has run the app — so `download`
//! exercises the real "already downloaded, ready immediately" terminal path
//! without a network fetch. This deliberately does not test a fresh
//! multi-gigabyte download (too slow/flaky for a test gate); the terminal-
//! state contract is the same regardless of how long the `downloading`
//! phase runs.

use nodespace_app_lib::commands::chat_models::{
    chat_model_cancel_download, chat_model_list, chat_model_load, chat_model_unload,
};
use nodespace_app_test_support::{
    model_file_available, SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT,
};
use nodespace_proto::nodespace::DownloadModelRequest;
use serde_json::json;
use tokio_stream::StreamExt;

const MODEL_ID: &str = "gemma-4-e4b-q4km";
// Filename mapped from MODEL_ID by the CATALOG entry in
// packages/agent/src/local_agent/model_manager.rs — kept in sync manually
// since this test has no dependency on that crate.
const MODEL_FILENAME: &str = "gemma-4-E4B-it-Q4_K_M.gguf";

/// No provisioning step downloads this model in bun install/test-gate.ts
/// today. Tests that need it already present skip cleanly with a clear
/// message rather than silently kicking off a live multi-gigabyte
/// HuggingFace fetch and hanging the pre-push gate on a machine that hasn't
/// run `model load` for it before.
macro_rules! skip_if_model_absent {
    () => {
        if !model_file_available(MODEL_FILENAME) {
            eprintln!(
                "SKIPPED: {MODEL_ID} not found under ~/.nodespace/models/{MODEL_FILENAME} — \
                 run `target/debug/nodespace model load {MODEL_ID}` once to provision it, \
                 then re-run this test."
            );
            return;
        }
    };
}

/// Drains the real `DownloadModel` stream (same RPC `chat_model_download`
/// proxies) and returns the sequence of `event_type`s observed.
async fn drain_download_events(harness: &TauriTestApp, model_id: &str) -> Vec<String> {
    let mut client = harness.client_state().local_agent_client().await;
    let mut stream = client
        .download_model(DownloadModelRequest {
            model_id: model_id.to_string(),
        })
        .await
        .expect("DownloadModel RPC failed")
        .into_inner();

    let mut event_types = Vec::new();
    while let Some(event) = stream.next().await {
        let event = event.expect("DownloadModel stream yielded a transport error");
        event_types.push(event.event_type.clone());
        if event.event_type == "error" {
            panic!(
                "download stream reached an error terminal state: {:?}",
                event.error_message
            );
        }
    }
    event_types
}

#[tokio::test]
async fn download_of_an_already_present_model_reaches_ready_with_no_error() {
    skip_if_model_absent!();
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;

    let event_types = drain_download_events(&harness, MODEL_ID).await;

    assert!(
        event_types.last().map(String::as_str) == Some("ready"),
        "download stream must end in a ready terminal state, got: {event_types:?}"
    );
    assert!(
        !event_types.iter().any(|t| t == "error"),
        "no error events may occur for an already-downloaded model: {event_types:?}"
    );
}

#[tokio::test]
async fn chat_model_list_still_succeeds_after_a_download() {
    skip_if_model_absent!();
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    drain_download_events(&harness, MODEL_ID).await;

    let models = chat_model_list(state, None)
        .await
        .expect("chat_model_list failed");
    assert!(
        models.iter().any(|m| m["id"] == MODEL_ID),
        "downloaded model must appear in the catalog list"
    );
}

#[tokio::test]
async fn cancel_of_a_download_with_no_stray_downloading_state_left_behind() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    // Cancelling a model that isn't mid-download must not error and must
    // leave no trace — the "no lingering downloading state" half of the
    // acceptance criterion, exercised on the path that's deterministic in a
    // test (an in-flight multi-gigabyte download is not).
    chat_model_cancel_download(MODEL_ID.to_string(), state.clone())
        .await
        .expect("cancel of a non-in-progress download must be a clean no-op, not an error");

    let models = chat_model_list(state, None)
        .await
        .expect("chat_model_list failed after cancel");
    for m in &models {
        if m["id"] == json!(MODEL_ID) {
            let status_str = serde_json::to_string(&m["status"]).unwrap_or_default();
            assert!(
                !status_str.to_lowercase().contains("downloading"),
                "no model may be left in a Downloading status after cancel: {status_str}"
            );
        }
    }
}

#[tokio::test]
async fn load_then_unload_reaches_a_clean_terminal_state() {
    skip_if_model_absent!();
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    drain_download_events(&harness, MODEL_ID).await;

    chat_model_load(MODEL_ID.to_string(), state.clone())
        .await
        .expect("chat_model_load failed for an already-downloaded model");

    chat_model_unload(state)
        .await
        .expect("chat_model_unload must cleanly succeed after a load");
}
