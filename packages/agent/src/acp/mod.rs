//! The client side of the Agent Client Protocol: a daemon-owned session with an
//! external coding agent that speaks JSON-RPC over the agent's standard input
//! and output.
//!
//! The agent is an untrusted program, like an MCP server. It is started with no
//! shell, a built environment, a private working directory and its own process
//! group (the shared helpers of the remote tool runners); everything it sends is
//! bounded before it is buffered, credentials are removed from the raw bytes
//! before anything is parsed, and what it asks of the client is refused unless
//! it is the one request the client serves: a permission request, which is
//! surfaced as an event and never answered with an "always" choice.
//!
//! This module holds the engine only: it produces an in-memory stream of typed
//! events. Writing them to the graph, the gRPC surface and the viewer are other
//! layers.

mod engine;
mod events;
mod launch;
mod wire;

pub use engine::{AcpSession, PermissionChoice, SessionInfo};
pub use events::{
    AcpEvent, AcpUpdate, EndReason, PermissionOption, PermissionOptionKind, PermissionRequest,
    PermissionResolution, StopReason, ToolKind, ToolStatus,
};
pub use launch::AcpLaunch;
pub use wire::{AcpError, AcpLimits};

#[cfg(all(test, unix))]
mod tests;
