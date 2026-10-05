//! PTY-based agent session engine (ADR-032).
//!
//! [`PtySession`] owns one running external agent process attached to a
//! pseudo-terminal. [`PtySessionManager`] owns the collection of active
//! sessions and is meant to be held in `nodespaced`'s shared state.
//!
//! The session lifecycle is:
//!
//! 1. Take the working directory the launch names (a project's folder on this
//!    machine, ADR-093 §8), or create a private one at
//!    `~/.nodespace/agent-sessions/<uuid>/`. Nothing is written into it.
//! 2. Spawn the agent binary inside a freshly opened PTY rooted there, with an
//!    allowlisted environment that names the database, the daemon socket, the
//!    session and what it was launched for.
//! 3. Stream stdout/stderr bytes through a `broadcast::Sender<OutputChunk>`.
//! 4. Accept stdin via [`PtySession::write_input`] and resize via [`PtySession::resize`].
//! 5. Take the harness's own session id when its plugin reports it
//!    ([`PtySession::report_harness_session_id`]).

pub mod capture;
pub mod detection;
pub mod manager;
pub mod plain_text;
pub mod session;

pub use capture::SessionCapture;
pub use detection::{
    agent_search_path, detect_all_agents, detect_all_agents_on, resolve_binary, AgentAvailability,
};
pub use manager::{PtySessionManager, SessionMetadata};
pub use session::{
    ExitStatus, OutputChunk, PtySession, SessionLaunch, LAUNCHED_FOR_ENV_VAR, SESSION_ENV_VAR,
};
