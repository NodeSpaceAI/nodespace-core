//! PTY-based agent session engine (ADR-032).
//!
//! [`PtySession`] owns one running external agent process attached to a
//! pseudo-terminal. [`PtySessionManager`] owns the collection of active
//! sessions and is meant to be held in `nodespaced`'s shared state.
//!
//! The session lifecycle is:
//!
//! 1. Create a persistent session directory at `~/.nodespace/agent-sessions/<uuid>/`.
//! 2. Write the context file ([`GraphContextAssembler::write_context_file`]) so the
//!    agent picks up its `CLAUDE.md` / `AGENTS.md` on launch, and copy `SKILL.md`
//!    into the same directory so the agent drives NodeSpace via the `nodespace` CLI.
//! 3. Spawn the agent binary inside a freshly opened PTY rooted at the session dir.
//! 4. Stream stdout/stderr bytes through a `broadcast::Sender<OutputChunk>`.
//! 5. Accept stdin via [`PtySession::write_input`] and resize via [`PtySession::resize`].
//! 6. On [`PtySession::terminate`] (or when the child exits naturally), the session
//!    directory is **not** deleted — artifacts survive across restarts.

pub mod capture;
pub mod detection;
pub mod harness_session;
pub mod manager;
pub mod plain_text;
pub mod session;

pub use capture::SessionCapture;
pub use detection::{detect_all_agents, AgentAvailability};
pub use harness_session::find_harness_session_id;
pub use manager::{PtySessionManager, SessionMetadata};
pub use session::{ExitStatus, OutputChunk, PtySession};
