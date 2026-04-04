//! Tauri commands for ACP (Agent Communication Protocol) operations.
//!
//! Bridges the Svelte frontend to [`AcpClientService`] and
//! [`SystemAgentRegistry`] via Tauri IPC. Session state changes are
//! forwarded to the frontend through Tauri event channels.
//!
//! Issue #1008

use crate::acp::registry::SystemAgentRegistry;
use crate::acp::session::AcpClientService;
use crate::agent_types::{events, AcpAgentInfo, AcpMessage, AcpSessionState, AgentRegistry};
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

    let session_id = service
        .start_session(&agent_id)
        .await
        .map_err(|e| {
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
/// Wraps the user message in a JSON-RPC `message/send` request and
/// waits for the agent's response, which is emitted on `acp://agent-message`.
#[tauri::command]
pub async fn acp_send_message(
    session_id: String,
    message: String,
    app: AppHandle,
    service: State<'_, AcpClientService>,
) -> Result<(), CommandError> {
    // Build the JSON-RPC message
    let msg = AcpMessage {
        jsonrpc: "2.0".to_string(),
        method: Some("message/send".to_string()),
        params: Some(serde_json::json!({
            "message": {
                "role": "user",
                "content": message,
            }
        })),
        id: Some(serde_json::json!(uuid::Uuid::new_v4().to_string())),
        result: None,
        error: None,
    };

    // The session_id from `start_session` encodes the agent_id prefix;
    // extract the agent_id portion (before the UUID suffix).
    // Format: "acp-{agent_id}-{uuid_prefix}"
    let agent_id = extract_agent_id(&session_id);

    service
        .send_message(&agent_id, msg)
        .await
        .map_err(|e| acp_error(format!("Failed to send message: {e}")))?;

    // Wait for the agent's response
    match service.receive_message(&agent_id).await {
        Ok(response) => {
            let _ = app.emit(events::ACP_AGENT_MESSAGE, &response);
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
