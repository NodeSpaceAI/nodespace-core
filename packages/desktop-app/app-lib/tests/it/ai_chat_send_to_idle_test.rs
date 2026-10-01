//! ADR-048 priority flow 4 — ai-chat send → idle with a real (not mocked)
//! model, through the real Tauri command layer against a real headless
//! daemon.
//!
//! There is no dedicated "send chat message" Tauri command or gRPC RPC.
//! Per `scripts/aichat.ts`'s doc comment (the existing CLI harness for this
//! same mechanism): the daemon's event watcher runs an inference turn when
//! an `ai-chat-native` node has `turn_status: "processing"` AND a trailing
//! `role: "user"` message. So a turn is: create/update the node with that
//! shape, then poll `get_node` until `turn_status` returns to `"idle"` with a
//! new assistant message appended.
//!
//! No lightweight stub/test-double model backend exists anywhere in this
//! codebase (confirmed: `ChatInferenceEngine`'s only implementors are the
//! real `LlamaChatInferenceEngine` and `OpenAiCompatInferenceEngine`; the daemon's
//! only placeholder, `NoOpInferenceEngine`, always errors rather than
//! completing a turn, so it cannot drive this flow at all). Adding one would
//! mean carving a test-only code path into the production model-selection
//! logic in `packages/daemon/src/services/local_agent_service.rs` — a
//! change to a real inference subsystem's behavior, not a test concern.
//! Instead, this uses `gemma-4-e4b-q4km` — the ADR-056-locked native model,
//! already downloaded under `~/.nodespace/models/` on this machine. It is a
//! real inference call, not instant, so this test's timeout is generous
//! (well beyond CRUD test scale) and it is the slowest test in this suite by
//! design.

use std::time::Duration;

use nodespace_app_lib::commands::nodes::{create_node, get_node, update_node, CreateNodeInput};
use nodespace_app_lib::types::NodeUpdate;
use nodespace_app_test_support::{
    model_file_available, SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT,
};
use nodespace_proto::nodespace::EnsureModelReadyRequest;
use serde_json::json;
use tokio_stream::StreamExt;

const MODEL_ID: &str = "gemma-4-e4b-q4km";
// Filename mapped from MODEL_ID by the CATALOG entry in
// packages/agent/src/local_agent/model_manager.rs — kept in sync manually
// since this test has no dependency on that crate. If this test starts
// failing the `model_file_available` skip-check spuriously, confirm that
// mapping hasn't changed.
const MODEL_FILENAME: &str = "gemma-4-E4B-it-Q4_K_M.gguf";

fn ai_chat_input(
    id: &str,
    provider_model: &str,
    turn_status: &str,
    messages: serde_json::Value,
) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "ai-chat-native".to_string(),
        content: "Test chat".to_string(),
        parent_id: None,
        insert_position: None,
        properties: json!({
            "agent": "nodespace",
            "provider": "native",
            "model": provider_model,
            "turn_status": turn_status,
            "messages": messages
        }),
    }
}

async fn poll_until_idle_with_new_assistant_reply(
    state: &tauri::State<'_, nodespace_app_lib::services::GrpcClient>,
    id: &str,
    prior_assistant_count: usize,
    timeout: Duration,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let node = get_node(state.clone(), id.to_string())
            .await
            .expect("get_node failed")
            .expect("node must exist");

        let turn_status = node["turnStatus"].as_str().unwrap_or_default();
        let messages = node["messages"].as_array().cloned().unwrap_or_default();
        let assistant_count = messages
            .iter()
            .filter(|m| m["role"] == json!("assistant"))
            .count();

        if turn_status == "idle" && assistant_count > prior_assistant_count {
            return node;
        }

        if tokio::time::Instant::now() >= deadline {
            panic!(
                "ai-chat turn did not reach idle-with-new-reply within {timeout:?}; \
                 last turn_status={turn_status:?}, assistant_count={assistant_count}; \
                 node={node}"
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
async fn ai_chat_send_reaches_idle_with_no_stuck_processing_state() {
    // No provisioning step downloads this model in bun install/test-gate.ts
    // today. Skip cleanly with a clear message rather than silently kicking
    // off a live multi-gigabyte HuggingFace fetch and hanging the pre-push
    // gate on a machine that hasn't run `model load` for it before.
    if !model_file_available(MODEL_FILENAME) {
        eprintln!(
            "SKIPPED: {MODEL_ID} not found under ~/.nodespace/models/{MODEL_FILENAME} — \
             run `target/debug/nodespace model load {MODEL_ID}` once to provision it, \
             then re-run this test."
        );
        return;
    }

    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    // chat_model_load's gRPC call only updates the model manager's
    // bookkeeping — it does NOT swap in a real inference engine (that swap,
    // `LocalAgentServiceImpl::replace_engine`, only happens inside
    // `load_model_and_collect_events`, reached solely via the
    // `EnsureModelReady` streaming RPC). Drive that RPC directly so the
    // daemon's ai-chat turn actually has a working engine to run against.
    let mut local_agent_client = state.local_agent_client().await;
    let mut ready_stream = local_agent_client
        .ensure_model_ready(EnsureModelReadyRequest {
            model_id: MODEL_ID.to_string(),
        })
        .await
        .expect("EnsureModelReady RPC failed")
        .into_inner();
    while let Some(event) = ready_stream.next().await {
        let event = event.expect("EnsureModelReady stream yielded a transport error");
        if event.event_type == "error" {
            panic!(
                "EnsureModelReady reached an error state: {:?}",
                event.error_message
            );
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    create_node(
        state.clone(),
        ai_chat_input(&id, MODEL_ID, "idle", json!([])),
    )
    .await
    .expect("create ai-chat node failed");

    // "Send a message": append a user message and flip turn_status to
    // processing — the exact mechanism scripts/aichat.ts's cmdSend
    // documents, and the exact key shape the real frontend's handleSend
    // actually sends: the declared snake_case `turn_status`. This matters
    // more than it looks: a chat has no dedicated typed write command like
    // task does, so the generic updateNode path carries the frontend's own
    // keys, and only the schema's declared names are read back
    // (`AiChatNativeNode::from_node`). The chat's bucket is closed, so a
    // camelCase `turnStatus` is refused rather than stored unread.
    let after_send = update_node(
        state.clone(),
        id.clone(),
        1,
        NodeUpdate {
            properties: Some(json!({
                "provider": "native",
                "model": MODEL_ID,
                "turn_status": "processing",
                "messages": [
                    { "role": "user", "content": "Reply with exactly one word: OK" }
                ]
            })),
            ..Default::default()
        },
    )
    .await
    .expect("update_node (send message) failed");
    assert_eq!(
        after_send["turnStatus"],
        json!("processing"),
        "update_node's own response must reflect the processing turnStatus just written: {after_send:?}"
    );

    // 300s, not the 180s this used to be: this is a real inference call
    // competing for the same CPU as every other daemon-spawning test in this
    // suite (see DAEMON_CONNECT_TIMEOUT's comment) — under representative
    // background load a token-by-token completion can take meaningfully
    // longer than on an idle machine. poll_until_idle_with_new_assistant_reply
    // returns as soon as the turn completes, so this only raises the ceiling
    // for a genuinely slow/stuck run, not the typical wall-clock cost.
    let final_node =
        poll_until_idle_with_new_assistant_reply(&state, &id, 0, Duration::from_secs(300)).await;

    assert_eq!(final_node["turnStatus"], json!("idle"));
    let messages = final_node["messages"]
        .as_array()
        .expect("messages must be an array");
    let assistant_reply = messages
        .iter()
        .rev()
        .find(|m| m["role"] == json!("assistant"))
        .expect("an assistant reply must be present");
    assert!(
        !assistant_reply["content"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "assistant reply content must not be empty"
    );
}
