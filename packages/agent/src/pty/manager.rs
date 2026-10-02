//! Owns the collection of running [`PtySession`]s.
//!
//! [`PtySessionManager`] is held in `nodespaced`'s shared state and is the
//! single entry point through which gRPC handlers / Tauri commands create,
//! drive, and tear down agent sessions. Sessions live behind an
//! `Arc<Mutex<...>>` so concurrent callers can interact safely; the manager
//! also spawns a small watcher task per session that drops the entry when
//! the underlying agent process exits on its own.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::agent_catalog::context_assembly::GraphContextAssembler;
use crate::agent_types::AgentType;
use crate::pty::session::PtySession;

/// Lightweight snapshot of a session suitable for listing in a UI / RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: Uuid,
    pub agent_type: AgentType,
    pub started_at: DateTime<Utc>,
    /// The `ai-chat-pty` node the session was launched for, if any.
    pub node_id: Option<String>,
}

impl SessionMetadata {
    fn from_session(s: &PtySession) -> Self {
        Self {
            id: s.id,
            agent_type: s.agent_type,
            started_at: s.started_at,
            node_id: s.node_id.clone(),
        }
    }
}

/// Holds all active PTY sessions.
#[derive(Default, Clone)]
pub struct PtySessionManager {
    sessions: Arc<Mutex<HashMap<Uuid, Arc<PtySession>>>>,
}

impl PtySessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Launch a new agent session and register it. Returns the session id.
    ///
    /// `node_id` is the `ai-chat-pty` node the session is a view onto, when
    /// there is one.
    pub async fn launch(
        &self,
        agent_type: AgentType,
        initial_prompt: Option<String>,
        node_id: Option<String>,
        assembler: &GraphContextAssembler,
    ) -> anyhow::Result<Uuid> {
        let session = PtySession::launch(agent_type, initial_prompt, node_id, assembler).await?;
        Ok(self.insert(session).await)
    }

    /// Register an already-launched session. Split out so tests can build
    /// sessions without a real [`GraphContextAssembler`].
    pub async fn insert(&self, session: PtySession) -> Uuid {
        let id = session.id;
        let mut exit_rx = session.subscribe_exit();
        let session = Arc::new(session);

        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(id, session);
        }

        // Auto-cleanup: when the child process exits, remove the session entry
        // from the map. The session directory at
        // `~/.nodespace/agent-sessions/<uuid>/` is NOT deleted — it persists
        // on disk so artifacts survive restarts. `watch` latches the final
        // value, so this works whether the child exits before or after the
        // receiver was constructed.
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            // Fast path: exit already happened before insert returned.
            if exit_rx.borrow().is_none() {
                // Loop until the latched value transitions to Some, or the
                // sender is dropped (session removed by another path).
                while exit_rx.changed().await.is_ok() {
                    if exit_rx.borrow().is_some() {
                        break;
                    }
                }
            }
            let mut guard = sessions.lock().await;
            guard.remove(&id);
        });

        id
    }

    /// Return an `Arc` to the session, if it exists.
    pub async fn get(&self, id: &Uuid) -> Option<Arc<PtySession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    /// Terminate and remove a session. Returns `Ok(false)` if the session
    /// was not in the map (already cleaned up by the natural-exit watcher).
    ///
    /// Removing the session from the map drops the `Arc<PtySession>` when no
    /// other caller holds a reference. The session directory on disk is NOT
    /// deleted — it persists at `~/.nodespace/agent-sessions/<uuid>/`.
    pub async fn terminate(&self, id: &Uuid) -> anyhow::Result<bool> {
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(id)
        };

        let Some(session) = session else {
            return Ok(false);
        };

        session.terminate().await?;
        Ok(true)
    }

    /// Return a snapshot of every active session.
    pub async fn list(&self) -> Vec<SessionMetadata> {
        let sessions = self.sessions.lock().await;
        sessions
            .values()
            .map(|s| SessionMetadata::from_session(s))
            .collect()
    }

    /// Number of currently registered sessions. Useful for tests.
    #[cfg(test)]
    pub(crate) async fn len(&self) -> usize {
        self.sessions.lock().await.len()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    /// Wait until the manager's session count reaches `target` or the deadline
    /// expires. Polls every 50 ms because the natural-exit watcher is an
    /// independently-spawned task.
    async fn wait_for_len(manager: &PtySessionManager, target: usize, deadline: Duration) {
        let start = std::time::Instant::now();
        while start.elapsed() < deadline {
            if manager.len().await == target {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "manager len {} did not reach {} within {:?}",
            manager.len().await,
            target,
            deadline
        );
    }

    #[tokio::test]
    async fn insert_then_list_returns_metadata() {
        let manager = PtySessionManager::new();
        let mut session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 1".into()])
            .expect("launch session");
        session.node_id = Some("chat-node".to_string());

        let id = manager.insert(session).await;
        let listed = manager.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].node_id.as_deref(), Some("chat-node"));

        // Wait for the session to exit naturally so the manager's auto-prune
        // runs; otherwise the test would leak the watcher task.
        wait_for_len(&manager, 0, Duration::from_secs(3)).await;
    }

    #[tokio::test]
    async fn terminate_removes_session_and_returns_true() {
        let manager = PtySessionManager::new();
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 30".into()])
            .expect("launch session");

        let id = manager.insert(session).await;
        assert_eq!(manager.len().await, 1);

        let removed = timeout(Duration::from_secs(3), manager.terminate(&id))
            .await
            .expect("terminate returns within deadline")
            .expect("terminate succeeds");
        assert!(removed, "terminate should return true for active session");
        assert_eq!(manager.len().await, 0);
    }

    #[tokio::test]
    async fn terminate_unknown_returns_false() {
        let manager = PtySessionManager::new();
        let removed = manager
            .terminate(&Uuid::new_v4())
            .await
            .expect("terminate succeeds");
        assert!(!removed);
    }

    #[tokio::test]
    async fn natural_exit_auto_removes_session() {
        let manager = PtySessionManager::new();
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "echo bye".into()])
            .expect("launch session");

        let _id = manager.insert(session).await;
        wait_for_len(&manager, 0, Duration::from_secs(3)).await;
    }

    #[tokio::test]
    async fn get_returns_session_and_supports_write_resize() {
        let manager = PtySessionManager::new();
        let session = PtySession::launch_for_test("cat", vec![]).expect("launch cat");
        let id = manager.insert(session).await;

        let arc = manager.get(&id).await.expect("session present");
        arc.write_input(b"ok\n").await.expect("write input");
        arc.resize(100, 30).await.expect("resize");

        manager.terminate(&id).await.expect("terminate cat");
    }

    /// Regression for review Finding #2: very short-lived processes whose
    /// exit fires before the manager's auto-prune task has a chance to
    /// subscribe. The watch-channel redesign latches the final value so the
    /// subscriber still observes it. Repeated to make the race more likely.
    #[tokio::test]
    async fn ultra_short_lived_processes_are_pruned() {
        let manager = PtySessionManager::new();
        for _ in 0..10 {
            // `true` exits with status 0 essentially immediately. The watcher
            // very plausibly fires before the manager's auto-prune subscribes.
            let session = PtySession::launch_for_test("true", vec![]).expect("launch true session");
            manager.insert(session).await;
        }
        wait_for_len(&manager, 0, Duration::from_secs(3)).await;
    }
}
