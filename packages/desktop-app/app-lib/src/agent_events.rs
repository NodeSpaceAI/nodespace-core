//! Tauri event channel constants for the agent subsystem.
//!
//! These constants must match the TypeScript frontend event listeners.
//! They live in the desktop-app crate because they depend on Tauri,
//! which is not a dependency of the `nodespace-agent` crate.

/// Streaming inference chunk from the local agent.
pub const LOCAL_AGENT_CHUNK: &str = "local-agent://chunk";

/// Tool execution event from the local agent.
pub const LOCAL_AGENT_TOOL: &str = "local-agent://tool";

/// Local agent status change (idle, thinking, streaming, etc.).
pub const LOCAL_AGENT_STATUS: &str = "local-agent://status";

/// Local agent error event.
pub const LOCAL_AGENT_ERROR: &str = "local-agent://error";

/// Model download progress update.
pub const MODEL_DOWNLOAD_PROGRESS: &str = "model://download-progress";

/// Model download completed successfully.
pub const MODEL_DOWNLOAD_READY: &str = "model://download-ready";

/// Model status change (loading, loaded, error).
pub const MODEL_STATUS: &str = "model://status";
