//! Tauri commands for ACP (Agent Communication Protocol) operations.
//!
//! Bridges the Svelte frontend to [`AcpClientService`] and
//! [`SystemAgentRegistry`] via Tauri IPC. Session state changes are
//! forwarded to the frontend through Tauri event channels.
//!
//! Issue #1008

use crate::acp::registry::SystemAgentRegistry;
use crate::acp::session::AcpClientService;
use crate::agent_types::{
    events, AcpAgentInfo, AcpError, AcpMessage, AcpSessionState, AgentRegistry,
};
use crate::commands::nodes::CommandError;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Helper to map ACP errors into [`CommandError`].
fn acp_error(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        code: "ACP_ERROR".to_string(),
        details: None,
    }
}

/// List all discovered ACP agents.
#[tauri::command]
pub async fn acp_list_agents(
    registry: State<'_, Arc<SystemAgentRegistry>>,
) -> Result<Vec<AcpAgentInfo>, CommandError> {
    registry
        .discover_agents()
        .await
        .map_err(|e| acp_error(format!("Failed to list agents: {e}")))
}

/// Start a new ACP session with the specified agent.
///
/// Returns the session ID. Emits `acp://session-state` events for
/// state transitions (Initializing -> Active).
#[tauri::command]
pub async fn acp_start_session(
    agent_id: String,
    app: AppHandle,
    service: State<'_, AcpClientService>,
) -> Result<String, CommandError> {
    // Emit initializing state
    let _ = app.emit(events::ACP_SESSION_STATE, &AcpSessionState::Initializing);

    let session_id = service.start_session(&agent_id).await.map_err(|e| {
        let _ = app.emit(
            events::ACP_SESSION_STATE,
            &AcpSessionState::Failed {
                reason: e.to_string(),
            },
        );
        acp_error(format!("Failed to start session: {e}"))
    })?;

    // Emit active state
    let _ = app.emit(events::ACP_SESSION_STATE, &AcpSessionState::Active);

    tracing::info!(session_id = %session_id, agent = %agent_id, "ACP session started");
    Ok(session_id)
}

/// Send a message to an active ACP session.
///
/// Sends a `session/prompt` JSON-RPC request with the user's message.
/// The agent streams back `session/update` notifications (emitted as
/// `acp://agent-message` events) followed by a final response to the
/// `session/prompt` request (also emitted).
#[tauri::command]
pub async fn acp_send_message(
    session_id: String,
    message: String,
    app: AppHandle,
    service: State<'_, AcpClientService>,
) -> Result<(), CommandError> {
    let agent_id = extract_agent_id(&session_id);

    // Get the agent-assigned session ID from the session/new response
    let acp_session_id = service
        .get_acp_session_id(&agent_id)
        .await
        .map_err(|e| acp_error(format!("Failed to get ACP session ID: {e}")))?
        .unwrap_or_else(|| session_id.clone());

    // Build the JSON-RPC `session/prompt` request
    let request_id = uuid::Uuid::new_v4().to_string();
    let msg = AcpMessage {
        jsonrpc: "2.0".to_string(),
        method: Some("session/prompt".to_string()),
        params: Some(serde_json::json!({
            "sessionId": acp_session_id,
            "prompt": [
                {
                    "type": "text",
                    "text": message,
                }
            ]
        })),
        id: Some(serde_json::json!(request_id)),
        result: None,
        error: None,
    };

    service
        .send_message(&agent_id, msg)
        .await
        .map_err(|e| acp_error(format!("Failed to send prompt: {e}")))?;

    // Read messages until we get the response to our session/prompt request.
    // The agent streams `session/update` notifications (no `id`) in between,
    // and may also send requests (with `method` + `id`) that we must respond to
    // (e.g. session/request_permission, fs/read_text_file, terminal/create).
    loop {
        match service.receive_message(&agent_id).await {
            Ok(response) => {
                let has_method = response.method.is_some();
                let has_id = response.id.is_some();

                // Agent REQUEST (method + id) — needs a response back.
                // Do NOT emit these to the frontend.
                if has_method && has_id {
                    let method = response.method.as_deref().unwrap_or("");
                    let req_id = response.id.clone().unwrap();
                    tracing::info!(method = %method, "Agent sent request, responding");

                    let reply = match method {
                        "session/request_permission" => {
                            // Find an "allow_once" or "allow_always" optionId from the options
                            let option_id = response
                                .params
                                .as_ref()
                                .and_then(|p| p.get("options"))
                                .and_then(|opts| opts.as_array())
                                .and_then(|arr| {
                                    // Prefer allow_always, fall back to allow_once
                                    arr.iter()
                                        .find(|opt| {
                                            opt.get("kind").and_then(|k| k.as_str())
                                                == Some("allow_always")
                                        })
                                        .or_else(|| {
                                            arr.iter().find(|opt| {
                                                opt.get("kind").and_then(|k| k.as_str())
                                                    == Some("allow_once")
                                            })
                                        })
                                })
                                .and_then(|opt| opt.get("optionId").and_then(|id| id.as_str()))
                                .unwrap_or("allow_always")
                                .to_string();

                            tracing::info!(option_id = %option_id, "Auto-allowing permission request");
                            AcpMessage {
                                jsonrpc: "2.0".to_string(),
                                method: None,
                                params: None,
                                id: Some(req_id),
                                result: Some(
                                    serde_json::json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
                                ),
                                error: None,
                            }
                        }
                        "session/elicitation" => {
                            // Auto-confirm elicitations
                            AcpMessage {
                                jsonrpc: "2.0".to_string(),
                                method: None,
                                params: None,
                                id: Some(req_id),
                                result: Some(serde_json::json!({ "optionId": "confirm" })),
                                error: None,
                            }
                        }
                        _ => {
                            // Unsupported request — return method-not-found error
                            tracing::warn!(method = %method, "Unsupported agent request, returning error");
                            AcpMessage {
                                jsonrpc: "2.0".to_string(),
                                method: None,
                                params: None,
                                id: Some(req_id),
                                result: None,
                                error: Some(AcpError {
                                    code: -32601,
                                    message: format!("Method not supported by client: {method}"),
                                    data: None,
                                }),
                            }
                        }
                    };

                    if let Err(e) = service.send_message(&agent_id, reply).await {
                        tracing::error!(error = %e, "Failed to send reply to agent request");
                    }
                    continue;
                }

                // Notification (method, no id) — streaming update, emit to frontend
                if has_method && !has_id {
                    let _ = app.emit(events::ACP_AGENT_MESSAGE, &response);
                    continue;
                }

                // Response to our session/prompt request (id, no method) — emit to frontend
                let _ = app.emit(events::ACP_AGENT_MESSAGE, &response);

                let is_our_response = response
                    .id
                    .as_ref()
                    .map(|id| id.as_str() == Some(&request_id))
                    .unwrap_or(false);

                if is_our_response || response.result.is_some() {
                    tracing::info!(session_id = %session_id, "Prompt turn completed");
                    break;
                }
                if response.error.is_some() {
                    tracing::error!(
                        session_id = %session_id,
                        error = ?response.error,
                        "Agent returned error for prompt"
                    );
                    break;
                }
            }
            Err(e) => {
                tracing::error!(session_id = %session_id, error = %e, "Failed to receive agent response");
                let _ = app.emit(
                    events::ACP_SESSION_STATE,
                    &AcpSessionState::Failed {
                        reason: e.to_string(),
                    },
                );
                return Err(acp_error(format!("Failed to receive response: {e}")));
            }
        }
    }

    Ok(())
}

/// End an ACP session gracefully.
#[tauri::command]
pub async fn acp_end_session(
    session_id: String,
    app: AppHandle,
    service: State<'_, AcpClientService>,
) -> Result<(), CommandError> {
    let agent_id = extract_agent_id(&session_id);

    let _ = app.emit(events::ACP_SESSION_STATE, &AcpSessionState::Completing);

    service.end_session(&agent_id).await.map_err(|e| {
        let _ = app.emit(
            events::ACP_SESSION_STATE,
            &AcpSessionState::Failed {
                reason: e.to_string(),
            },
        );
        acp_error(format!("Failed to end session: {e}"))
    })?;

    let _ = app.emit(events::ACP_SESSION_STATE, &AcpSessionState::Completed);
    tracing::info!(session_id = %session_id, "ACP session ended");
    Ok(())
}

/// Refresh the agent registry by re-scanning discovery paths.
///
/// Returns the updated list of agents.
#[tauri::command]
pub async fn acp_refresh_agents(
    registry: State<'_, Arc<SystemAgentRegistry>>,
) -> Result<Vec<AcpAgentInfo>, CommandError> {
    registry
        .refresh()
        .await
        .map_err(|e| acp_error(format!("Failed to refresh agents: {e}")))?;

    registry
        .discover_agents()
        .await
        .map_err(|e| acp_error(format!("Failed to list agents after refresh: {e}")))
}

/// Extract the agent ID from a session ID.
///
/// Session IDs are formatted as `acp-{agent_id}-{uuid_prefix}`.
/// This extracts the `{agent_id}` portion.
fn extract_agent_id(session_id: &str) -> String {
    // Strip "acp-" prefix, then take everything before the last "-"
    let without_prefix = session_id.strip_prefix("acp-").unwrap_or(session_id);
    // The UUID suffix is the last segment after the last hyphen
    if let Some(pos) = without_prefix.rfind('-') {
        without_prefix[..pos].to_string()
    } else {
        without_prefix.to_string()
    }
}
