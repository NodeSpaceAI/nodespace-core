//! ACP session state machine and orchestration.
//!
//! Manages the lifecycle of an external agent session: initialization handshake,
//! message routing while active, and clean teardown. The [`AcpSession`] struct
//! enforces a strict state machine (Idle → Initializing → Active → Completing →
//! Completed, with failure transitions to Failed from most states).
//!
//! [`AcpClientService`] is the public facade that manages multiple concurrent
//! sessions, enforcing at most one active session per agent type.
//!
//! Issue #1004

use crate::acp::transport::{StdioTransport, StdioTransportConfig};
use crate::agent_types::{
    AcpAgentInfo, AcpMessage, AcpSessionState, AcpTransport, AgentRegistry, TransportError,
};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Timeout for the initialization handshake (waiting for `initialized` response).
const INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Default timeout for waiting on a completion response from the agent.
const COMPLETION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// ACP protocol version.
const PROTOCOL_VERSION: &str = "1";

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors returned by session operations.
#[derive(Debug, Error)]
pub enum SessionError {
    /// The requested state transition is not valid from the current state.
    #[error("invalid state transition from {from:?} to {to:?}")]
    InvalidTransition {
        /// Current state.
        from: AcpSessionState,
        /// Attempted target state.
        to: AcpSessionState,
    },

    /// An operation was attempted that is not valid in the current state.
    #[error("operation not permitted in state {0:?}")]
    InvalidOperation(AcpSessionState),

    /// The agent was not found in the registry.
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    /// The agent is not available (binary missing or auth not satisfied).
    #[error("agent not available: {0}")]
    AgentNotAvailable(String),

    /// A session already exists for this agent.
    #[error("session already exists for agent: {0}")]
    DuplicateSession(String),

    /// No session found for the given ID.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// The initialization handshake timed out.
    #[error("initialization timed out after {0:?}")]
    InitTimeout(std::time::Duration),

    /// Transport layer error.
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// ---------------------------------------------------------------------------
// AcpSession
// ---------------------------------------------------------------------------

/// A single ACP agent session with enforced state machine transitions.
///
/// The session owns the transport connection to the agent process and tracks
/// metadata about the conversation (message count, timestamps).
pub struct AcpSession {
    /// Unique session identifier.
    id: String,
    /// Information about the agent this session communicates with.
    agent_info: AcpAgentInfo,
    /// The transport layer (stdin/stdout to the agent process).
    transport: Option<StdioTransport>,
    /// Current session state.
    state: AcpSessionState,
    /// When the session was created.
    created_at: chrono::DateTime<chrono::Utc>,
    /// Number of messages exchanged in this session.
    message_count: u64,
}

impl AcpSession {
    /// Create a new session in the Idle state.
    fn new(id: String, agent_info: AcpAgentInfo) -> Self {
        Self {
            id,
            agent_info,
            transport: None,
            state: AcpSessionState::Idle,
            created_at: chrono::Utc::now(),
            message_count: 0,
        }
    }

    /// Return the current session state.
    pub fn state(&self) -> &AcpSessionState {
        &self.state
    }

    /// Return the session ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the agent info.
    pub fn agent_info(&self) -> &AcpAgentInfo {
        &self.agent_info
    }

    /// Return the message count.
    pub fn message_count(&self) -> u64 {
        self.message_count
    }

    /// Return when the session was created.
    pub fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.created_at
    }

    /// Attempt a state transition. Returns an error if the transition is invalid.
    fn transition(&mut self, target: AcpSessionState) -> Result<(), SessionError> {
        let valid = match (&self.state, &target) {
            // Normal flow
            (AcpSessionState::Idle, AcpSessionState::Initializing) => true,
            (AcpSessionState::Initializing, AcpSessionState::Active) => true,
            (AcpSessionState::Active, AcpSessionState::Completing) => true,
            (AcpSessionState::Completing, AcpSessionState::Completed) => true,
            // Failure transitions
            (AcpSessionState::Initializing, AcpSessionState::Failed { .. }) => true,
            (AcpSessionState::Active, AcpSessionState::Failed { .. }) => true,
            (AcpSessionState::Completing, AcpSessionState::Failed { .. }) => true,
            // Everything else is invalid
            _ => false,
        };

        if !valid {
            return Err(SessionError::InvalidTransition {
                from: self.state.clone(),
                to: target,
            });
        }

        debug!(
            session_id = %self.id,
            from = ?self.state,
            to = ?target,
            "Session state transition"
        );
        self.state = target;
        Ok(())
    }

    /// Run the initialization handshake: spawn transport, send `initialize`,
    /// wait for `initialized` response, send `session/new`.
    async fn initialize(&mut self) -> Result<(), SessionError> {
        self.transition(AcpSessionState::Initializing)?;

        // Build transport config from agent info
        let config = StdioTransportConfig {
            binary: self.agent_info.binary.clone(),
            args: self.agent_info.args.clone(),
            env: HashMap::new(),
            working_dir: None,
        };

        // Spawn the transport
        let transport = match StdioTransport::spawn(config).await {
            Ok(t) => t,
            Err(e) => {
                let reason = format!("transport spawn failed: {e}");
                error!(session_id = %self.id, error = %e, "Failed to spawn agent transport");
                self.state = AcpSessionState::Failed {
                    reason: reason.clone(),
                };
                return Err(SessionError::Transport(e));
            }
        };

        self.transport = Some(transport);
        info!(session_id = %self.id, agent = %self.agent_info.id, "Agent transport spawned");

        // Send `initialize` message
        let init_msg = AcpMessage {
            jsonrpc: "2.0".to_string(),
            method: Some("initialize".to_string()),
            params: Some(serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {}
            })),
            id: Some(serde_json::json!(1)),
            result: None,
            error: None,
        };

        if let Err(e) = self.transport_send(&init_msg).await {
            let reason = format!("failed to send initialize: {e}");
            self.state = AcpSessionState::Failed { reason };
            return Err(e);
        }

        // Wait for `initialized` response with timeout
        let receive_result = tokio::time::timeout(INIT_TIMEOUT, self.transport_receive()).await;

        match receive_result {
            Ok(Ok(response)) => {
                // Validate it looks like an initialized response
                if response.error.is_some() {
                    let reason = format!(
                        "agent returned error during initialization: {:?}",
                        response.error
                    );
                    self.state = AcpSessionState::Failed {
                        reason: reason.clone(),
                    };
                    return Err(SessionError::Other(anyhow::anyhow!(reason)));
                }
                debug!(
                    session_id = %self.id,
                    "Received initialized response"
                );
            }
            Ok(Err(e)) => {
                let reason = format!("transport error during initialization: {e}");
                self.state = AcpSessionState::Failed { reason };
                return Err(e);
            }
            Err(_elapsed) => {
                let reason = format!("initialization timed out after {:?}", INIT_TIMEOUT);
                warn!(session_id = %self.id, %reason);
                self.state = AcpSessionState::Failed { reason };
                return Err(SessionError::InitTimeout(INIT_TIMEOUT));
            }
        }

        // Send `session/new` with MCP server config
        let session_new_msg = AcpMessage {
            jsonrpc: "2.0".to_string(),
            method: Some("session/new".to_string()),
            params: Some(serde_json::json!({
                "mcpServers": [{
                    "name": "nodespace",
                    "url": "http://localhost:3100/mcp"
                }]
            })),
            id: Some(serde_json::json!(2)),
            result: None,
            error: None,
        };

        if let Err(e) = self.transport_send(&session_new_msg).await {
            let reason = format!("failed to send session/new: {e}");
            self.state = AcpSessionState::Failed { reason };
            return Err(e);
        }

        // Wait for session/new acknowledgement
        let session_result = tokio::time::timeout(INIT_TIMEOUT, self.transport_receive()).await;

        match session_result {
            Ok(Ok(response)) => {
                if response.error.is_some() {
                    let reason = format!(
                        "agent returned error during session/new: {:?}",
                        response.error
                    );
                    self.state = AcpSessionState::Failed {
                        reason: reason.clone(),
                    };
                    return Err(SessionError::Other(anyhow::anyhow!(reason)));
                }
                debug!(
                    session_id = %self.id,
                    "Received session/new acknowledgement"
                );
            }
            Ok(Err(e)) => {
                let reason = format!("transport error during session/new: {e}");
                self.state = AcpSessionState::Failed { reason };
                return Err(e);
            }
            Err(_elapsed) => {
                let reason = format!("session/new timed out after {:?}", INIT_TIMEOUT);
                warn!(session_id = %self.id, %reason);
                self.state = AcpSessionState::Failed { reason };
                return Err(SessionError::InitTimeout(INIT_TIMEOUT));
            }
        }

        // Transition to Active
        self.transition(AcpSessionState::Active)?;
        info!(session_id = %self.id, agent = %self.agent_info.id, "Session active");
        Ok(())
    }

    /// Send a message to the agent. Only valid in the Active state.
    async fn send_message(&mut self, message: AcpMessage) -> Result<(), SessionError> {
        if self.state != AcpSessionState::Active {
            return Err(SessionError::InvalidOperation(self.state.clone()));
        }

        self.transport_send(&message).await?;
        self.message_count += 1;
        Ok(())
    }

    /// Receive the next message from the agent. Only valid in the Active state.
    async fn receive_message(&mut self) -> Result<AcpMessage, SessionError> {
        if self.state != AcpSessionState::Active {
            return Err(SessionError::InvalidOperation(self.state.clone()));
        }

        let msg = self.transport_receive().await?;
        self.message_count += 1;
        Ok(msg)
    }

    /// End the session: transition to Completing, wait for final response, then Completed.
    async fn end(&mut self) -> Result<(), SessionError> {
        // Allow ending from Active or Completing states
        if self.state == AcpSessionState::Active {
            self.transition(AcpSessionState::Completing)?;
        } else if self.state != AcpSessionState::Completing {
            return Err(SessionError::InvalidOperation(self.state.clone()));
        }

        // Attempt graceful shutdown of the transport
        if let Some(ref transport) = self.transport {
            // Wait briefly for any final agent response
            let _final_response =
                tokio::time::timeout(COMPLETION_TIMEOUT, transport.shutdown()).await;
        }

        self.transition(AcpSessionState::Completed)?;
        info!(
            session_id = %self.id,
            agent = %self.agent_info.id,
            messages = self.message_count,
            "Session completed"
        );
        Ok(())
    }

    /// Mark the session as failed with a reason.
    fn fail(&mut self, reason: String) {
        warn!(
            session_id = %self.id,
            %reason,
            "Session failed"
        );
        self.state = AcpSessionState::Failed { reason };
    }

    // -- Internal transport helpers --

    /// Send a message via the transport, converting errors appropriately.
    async fn transport_send(&self, message: &AcpMessage) -> Result<(), SessionError> {
        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| SessionError::InvalidOperation(self.state.clone()))?;

        transport.send(message.clone()).await.map_err(|e| {
            error!(session_id = %self.id, error = %e, "Transport send failed");
            SessionError::Transport(e)
        })
    }

    /// Receive a message from the transport, converting errors appropriately.
    async fn transport_receive(&self) -> Result<AcpMessage, SessionError> {
        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| SessionError::InvalidOperation(self.state.clone()))?;

        transport.receive().await.map_err(|e| {
            error!(session_id = %self.id, error = %e, "Transport receive failed");
            SessionError::Transport(e)
        })
    }
}

// ---------------------------------------------------------------------------
// AcpClientService
// ---------------------------------------------------------------------------

/// Manages multiple ACP sessions, enforcing one session per agent type.
///
/// Thread-safe via interior mutability (`Arc<Mutex<...>>`).
pub struct AcpClientService {
    sessions: Mutex<HashMap<String, AcpSession>>,
    registry: Arc<dyn AgentRegistry>,
}

impl AcpClientService {
    /// Create a new client service backed by the given agent registry.
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            registry,
        }
    }

    /// Start a new session with the specified agent.
    ///
    /// Verifies the agent exists and is available via the registry, then
    /// runs the initialization handshake. Returns the session ID on success.
    ///
    /// Fails if a session already exists for this agent.
    pub async fn start_session(&self, agent_id: &str) -> Result<String, SessionError> {
        // Verify agent via registry
        let agent_info = self
            .registry
            .get_agent(agent_id)
            .await
            .map_err(|_| SessionError::AgentNotFound(agent_id.to_string()))?;

        if !agent_info.available {
            return Err(SessionError::AgentNotAvailable(agent_id.to_string()));
        }

        // Check for duplicate session
        {
            let sessions = self.sessions.lock().await;
            if sessions.contains_key(agent_id) {
                return Err(SessionError::DuplicateSession(agent_id.to_string()));
            }
        }

        // Create and initialize session
        let session_id = format!(
            "acp-{}-{}",
            agent_id,
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("0")
        );

        let mut session = AcpSession::new(session_id.clone(), agent_info);
        session.initialize().await?;

        // Store session keyed by agent_id
        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(agent_id.to_string(), session);
        }

        info!(session_id = %session_id, agent = %agent_id, "Session started");
        Ok(session_id)
    }

    /// Send a message to an agent's session.
    pub async fn send_message(
        &self,
        agent_id: &str,
        message: AcpMessage,
    ) -> Result<(), SessionError> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(agent_id)
            .ok_or_else(|| SessionError::SessionNotFound(agent_id.to_string()))?;

        session.send_message(message).await
    }

    /// Receive the next message from an agent's session.
    pub async fn receive_message(&self, agent_id: &str) -> Result<AcpMessage, SessionError> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(agent_id)
            .ok_or_else(|| SessionError::SessionNotFound(agent_id.to_string()))?;

        session.receive_message().await
    }

    /// End an agent's session gracefully.
    pub async fn end_session(&self, agent_id: &str) -> Result<(), SessionError> {
        let mut session = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .remove(agent_id)
                .ok_or_else(|| SessionError::SessionNotFound(agent_id.to_string()))?
        };

        session.end().await
    }

    /// Get the current state of an agent's session.
    pub async fn get_session_state(&self, agent_id: &str) -> Result<AcpSessionState, SessionError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(agent_id)
            .ok_or_else(|| SessionError::SessionNotFound(agent_id.to_string()))?;

        Ok(session.state().clone())
    }

    /// Mark a session as failed with a diagnostic reason.
    ///
    /// This is used when an external event (e.g., transport failure detected
    /// outside of a send/receive call) needs to mark the session as failed.
    /// The session is removed from the active sessions map after being marked.
    pub async fn fail_session(&self, agent_id: &str, reason: String) -> Result<(), SessionError> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(agent_id)
            .ok_or_else(|| SessionError::SessionNotFound(agent_id.to_string()))?;

        session.fail(reason);
        // Remove the failed session so a new one can be started
        sessions.remove(agent_id);
        Ok(())
    }

    /// List all active session agent IDs.
    pub async fn active_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions.keys().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::{
        AcpAgentInfo, AcpAuthMethod, AcpMessage, AcpSessionState, AgentRegistry, RegistryError,
    };
    use async_trait::async_trait;
    use std::sync::Arc;

    // =========================================================================
    // Test helpers
    // =========================================================================

    fn test_agent_info(id: &str) -> AcpAgentInfo {
        AcpAgentInfo {
            id: id.to_string(),
            name: format!("Test Agent {}", id),
            binary: "/bin/bash".to_string(),
            args: vec![
                "-c".to_string(),
                // Echo agent: reads JSON-RPC requests, echoes them back as responses.
                // The first message it receives is the `initialize` request; it responds
                // with an `initialized` result. Subsequent messages are echoed as-is.
                r#"while IFS= read -r line; do echo "$line"; done"#.to_string(),
            ],
            auth_method: AcpAuthMethod::AgentManaged,
            available: true,
            version: Some("1.0.0".to_string()),
        }
    }

    fn unavailable_agent_info(id: &str) -> AcpAgentInfo {
        let mut info = test_agent_info(id);
        info.available = false;
        info
    }

    /// A mock registry that returns pre-configured agents.
    struct MockRegistry {
        agents: Vec<AcpAgentInfo>,
    }

    impl MockRegistry {
        fn new(agents: Vec<AcpAgentInfo>) -> Self {
            Self { agents }
        }
    }

    #[async_trait]
    impl AgentRegistry for MockRegistry {
        async fn discover_agents(&self) -> Result<Vec<AcpAgentInfo>, RegistryError> {
            Ok(self.agents.clone())
        }

        async fn get_agent(&self, agent_id: &str) -> Result<AcpAgentInfo, RegistryError> {
            self.agents
                .iter()
                .find(|a| a.id == agent_id)
                .cloned()
                .ok_or_else(|| RegistryError::NotFound(agent_id.to_string()))
        }

        async fn refresh(&self) -> Result<(), RegistryError> {
            Ok(())
        }
    }

    fn test_request(id: u64, method: &str) -> AcpMessage {
        AcpMessage {
            jsonrpc: "2.0".to_string(),
            method: Some(method.to_string()),
            params: Some(serde_json::json!({})),
            id: Some(serde_json::json!(id)),
            result: None,
            error: None,
        }
    }

    // =========================================================================
    // State machine transition tests
    // =========================================================================

    #[test]
    fn test_new_session_starts_idle() {
        let session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        assert_eq!(*session.state(), AcpSessionState::Idle);
    }

    #[test]
    fn test_idle_to_initializing_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        assert!(session.transition(AcpSessionState::Initializing).is_ok());
        assert_eq!(*session.state(), AcpSessionState::Initializing);
    }

    #[test]
    fn test_initializing_to_active_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        assert!(session.transition(AcpSessionState::Active).is_ok());
        assert_eq!(*session.state(), AcpSessionState::Active);
    }

    #[test]
    fn test_active_to_completing_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        session.transition(AcpSessionState::Active).unwrap();
        assert!(session.transition(AcpSessionState::Completing).is_ok());
        assert_eq!(*session.state(), AcpSessionState::Completing);
    }

    #[test]
    fn test_completing_to_completed_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        session.transition(AcpSessionState::Active).unwrap();
        session.transition(AcpSessionState::Completing).unwrap();
        assert!(session.transition(AcpSessionState::Completed).is_ok());
        assert_eq!(*session.state(), AcpSessionState::Completed);
    }

    #[test]
    fn test_initializing_to_failed_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        let result = session.transition(AcpSessionState::Failed {
            reason: "test error".to_string(),
        });
        assert!(result.is_ok());
        assert!(matches!(session.state(), AcpSessionState::Failed { .. }));
    }

    #[test]
    fn test_active_to_failed_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        session.transition(AcpSessionState::Active).unwrap();
        let result = session.transition(AcpSessionState::Failed {
            reason: "transport died".to_string(),
        });
        assert!(result.is_ok());
    }

    #[test]
    fn test_completing_to_failed_valid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        session.transition(AcpSessionState::Active).unwrap();
        session.transition(AcpSessionState::Completing).unwrap();
        let result = session.transition(AcpSessionState::Failed {
            reason: "timeout".to_string(),
        });
        assert!(result.is_ok());
    }

    // -- Invalid transitions --

    #[test]
    fn test_idle_to_active_invalid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        let result = session.transition(AcpSessionState::Active);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::InvalidTransition { .. }
        ));
    }

    #[test]
    fn test_idle_to_completing_invalid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        let result = session.transition(AcpSessionState::Completing);
        assert!(result.is_err());
    }

    #[test]
    fn test_idle_to_completed_invalid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        let result = session.transition(AcpSessionState::Completed);
        assert!(result.is_err());
    }

    #[test]
    fn test_idle_to_failed_invalid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        let result = session.transition(AcpSessionState::Failed {
            reason: "nope".to_string(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_completed_to_anything_invalid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        session.transition(AcpSessionState::Active).unwrap();
        session.transition(AcpSessionState::Completing).unwrap();
        session.transition(AcpSessionState::Completed).unwrap();

        // No transitions from Completed
        assert!(session.transition(AcpSessionState::Idle).is_err());
        assert!(session.transition(AcpSessionState::Active).is_err());
        assert!(session
            .transition(AcpSessionState::Failed {
                reason: "x".to_string()
            })
            .is_err());
    }

    #[test]
    fn test_failed_to_anything_invalid() {
        let mut session = AcpSession::new("test-1".to_string(), test_agent_info("agent-a"));
        session.transition(AcpSessionState::Initializing).unwrap();
        session
            .transition(AcpSessionState::Failed {
                reason: "err".to_string(),
            })
            .unwrap();

        // No transitions from Failed
        assert!(session.transition(AcpSessionState::Idle).is_err());
        assert!(session.transition(AcpSessionState::Active).is_err());
        assert!(session.transition(AcpSessionState::Initializing).is_err());
    }

    // =========================================================================
    // Session metadata tests
    // =========================================================================

    #[test]
    fn test_session_id_and_agent_info() {
        let info = test_agent_info("agent-x");
        let session = AcpSession::new("sess-42".to_string(), info.clone());
        assert_eq!(session.id(), "sess-42");
        assert_eq!(session.agent_info().id, "agent-x");
        assert_eq!(session.message_count(), 0);
    }

    // =========================================================================
    // Full lifecycle integration tests (with echo agent subprocess)
    // =========================================================================

    #[tokio::test]
    async fn test_session_initialize_and_send() {
        let info = test_agent_info("echo-agent");
        let mut session = AcpSession::new("test-init".to_string(), info);

        // Initialize (echo agent echoes the `initialize` request back)
        let result = session.initialize().await;
        assert!(result.is_ok(), "Initialize failed: {:?}", result.err());
        assert_eq!(*session.state(), AcpSessionState::Active);

        // Send a message
        let msg = test_request(10, "user/message");
        let send_result = session.send_message(msg).await;
        assert!(send_result.is_ok());
        assert_eq!(session.message_count(), 1);

        // Receive echoed message
        let recv_result = session.receive_message().await;
        assert!(recv_result.is_ok());
        let received = recv_result.unwrap();
        assert_eq!(received.id, Some(serde_json::json!(10)));
        assert_eq!(session.message_count(), 2);

        // End session
        let end_result = session.end().await;
        assert!(end_result.is_ok());
        assert_eq!(*session.state(), AcpSessionState::Completed);
    }

    #[tokio::test]
    async fn test_send_message_not_active_fails() {
        let info = test_agent_info("echo-agent");
        let mut session = AcpSession::new("test-not-active".to_string(), info);

        // Session is Idle -- sending should fail
        let msg = test_request(1, "test");
        let result = session.send_message(msg).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::InvalidOperation(_)
        ));
    }

    #[tokio::test]
    async fn test_receive_message_not_active_fails() {
        let info = test_agent_info("echo-agent");
        let mut session = AcpSession::new("test-recv-idle".to_string(), info);

        let result = session.receive_message().await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::InvalidOperation(_)
        ));
    }

    #[tokio::test]
    async fn test_init_fails_on_immediate_exit() {
        // Agent that exits immediately (no response to initialize request).
        // This tests the transport failure path during initialization.
        let mut info = test_agent_info("fast-exit");
        info.binary = "/bin/bash".to_string();
        info.args = vec!["-c".to_string(), "exit 0".to_string()];

        let mut session = AcpSession::new("test-fast-exit".to_string(), info);
        let result = session.initialize().await;
        assert!(result.is_err());
        assert!(matches!(session.state(), AcpSessionState::Failed { .. }));
    }

    #[tokio::test]
    async fn test_session_fail_marks_failed() {
        let info = test_agent_info("agent-f");
        let mut session = AcpSession::new("test-fail".to_string(), info);
        session.fail("something went wrong".to_string());
        assert!(matches!(
            session.state(),
            AcpSessionState::Failed { reason } if reason == "something went wrong"
        ));
    }

    // =========================================================================
    // AcpClientService tests
    // =========================================================================

    fn make_service(agents: Vec<AcpAgentInfo>) -> AcpClientService {
        let registry = Arc::new(MockRegistry::new(agents));
        AcpClientService::new(registry)
    }

    #[tokio::test]
    async fn test_service_start_session() {
        let service = make_service(vec![test_agent_info("echo-agent")]);

        let result = service.start_session("echo-agent").await;
        assert!(result.is_ok(), "Start session failed: {:?}", result.err());

        let session_id = result.unwrap();
        assert!(session_id.starts_with("acp-echo-agent-"));

        let state = service.get_session_state("echo-agent").await.unwrap();
        assert_eq!(state, AcpSessionState::Active);
    }

    #[tokio::test]
    async fn test_service_duplicate_session_rejected() {
        let service = make_service(vec![test_agent_info("echo-agent")]);

        service.start_session("echo-agent").await.unwrap();

        let result = service.start_session("echo-agent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::DuplicateSession(_)
        ));
    }

    #[tokio::test]
    async fn test_service_agent_not_found() {
        let service = make_service(vec![]);

        let result = service.start_session("nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::AgentNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_service_agent_not_available() {
        let service = make_service(vec![unavailable_agent_info("unavail-agent")]);

        let result = service.start_session("unavail-agent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::AgentNotAvailable(_)
        ));
    }

    #[tokio::test]
    async fn test_service_send_receive() {
        let service = make_service(vec![test_agent_info("echo-agent")]);
        service.start_session("echo-agent").await.unwrap();

        let msg = test_request(5, "test/ping");
        service.send_message("echo-agent", msg).await.unwrap();

        let response = service.receive_message("echo-agent").await.unwrap();
        assert_eq!(response.id, Some(serde_json::json!(5)));
    }

    #[tokio::test]
    async fn test_service_end_session() {
        let service = make_service(vec![test_agent_info("echo-agent")]);
        service.start_session("echo-agent").await.unwrap();

        let result = service.end_session("echo-agent").await;
        assert!(result.is_ok());

        // Session should be removed
        let state_result = service.get_session_state("echo-agent").await;
        assert!(state_result.is_err());
        assert!(matches!(
            state_result.unwrap_err(),
            SessionError::SessionNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_service_end_nonexistent_session() {
        let service = make_service(vec![]);

        let result = service.end_session("ghost").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::SessionNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_service_concurrent_different_agents() {
        let service = make_service(vec![test_agent_info("agent-a"), test_agent_info("agent-b")]);

        // Start two sessions with different agents
        let id_a = service.start_session("agent-a").await.unwrap();
        let id_b = service.start_session("agent-b").await.unwrap();

        assert_ne!(id_a, id_b);

        let sessions = service.active_sessions().await;
        assert_eq!(sessions.len(), 2);

        // Both should be active
        assert_eq!(
            service.get_session_state("agent-a").await.unwrap(),
            AcpSessionState::Active,
        );
        assert_eq!(
            service.get_session_state("agent-b").await.unwrap(),
            AcpSessionState::Active,
        );

        // End both
        service.end_session("agent-a").await.unwrap();
        service.end_session("agent-b").await.unwrap();

        assert!(service.active_sessions().await.is_empty());
    }

    #[tokio::test]
    async fn test_service_send_to_nonexistent_session() {
        let service = make_service(vec![]);

        let result = service.send_message("nope", test_request(1, "test")).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SessionError::SessionNotFound(_)
        ));
    }
}
