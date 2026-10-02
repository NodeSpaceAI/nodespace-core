//! The session id an agent harness gave its own conversation.
//!
//! A harness that can resume a conversation records it under the user's home
//! directory, under an id of its own. That id, not the PTY session's, is what
//! the harness's resume flag takes. [`find_harness_session_id`] reads it back
//! from the harness's session store once the harness has exited.
//!
//! Every PTY session runs in a working directory of its own
//! (`~/.nodespace/agent-sessions/<uuid>/`), and both stores record the working
//! directory a session ran in, so the directory identifies the session's
//! records without guessing from timestamps.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::agent_catalog::registry::{SessionStore, SystemAgentRegistry};
use crate::agent_types::AgentType;

/// How deep Codex nests a rollout file under its sessions directory
/// (`<yyyy>/<mm>/<dd>/`).
const CODEX_ROLLOUT_DEPTH: usize = 3;

/// Find the id `agent_type`'s harness recorded for the session that ran in
/// `session_dir`, looking in the harness's session store under `home`.
///
/// Returns `None` for a harness with no session store, and when the store
/// holds no record of the session (the harness exited before starting a
/// conversation, say). When the harness recorded several conversations in the
/// directory (the user cleared or switched conversations mid-session), the
/// one written to last is returned: it is the one the session ended on.
///
/// `started_at` is when the PTY session launched. Codex files every
/// conversation on the machine in one tree, so a rollout last written before
/// then cannot be this session's and is not opened. Claude Code's directory
/// for the working directory holds this session's conversations only, and
/// needs no such filter.
///
/// Reads the filesystem synchronously: call it off the async executor.
pub fn find_harness_session_id(
    agent_type: AgentType,
    home: &Path,
    session_dir: &Path,
    started_at: DateTime<Utc>,
) -> Option<String> {
    let store = SystemAgentRegistry::new().get(agent_type)?.session_store?;
    let working_dirs = working_dir_spellings(session_dir);
    match store {
        SessionStore::ClaudeProjects => claude_session_id(home, &working_dirs),
        SessionStore::CodexRollouts => codex_session_id(home, &working_dirs, started_at.into()),
    }
}

/// The ways a harness may have spelled the session's working directory: as
/// launched, and with symlinks resolved (what a process reads back as its
/// current directory).
fn working_dir_spellings(session_dir: &Path) -> Vec<PathBuf> {
    let mut spellings = vec![session_dir.to_path_buf()];
    if let Ok(resolved) = session_dir.canonicalize() {
        if resolved != session_dir {
            spellings.push(resolved);
        }
    }
    spellings
}

/// Claude Code keeps one directory per working directory, named after it, and
/// in it one `<session id>.jsonl` per conversation.
fn claude_session_id(home: &Path, working_dirs: &[PathBuf]) -> Option<String> {
    let projects = home.join(".claude").join("projects");
    working_dirs
        .iter()
        .flat_map(|dir| files_in(&projects.join(claude_project_dir_name(dir))))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|path| {
            // A conversation's file is named by its id, a UUID. Anything else
            // in the directory is not a conversation that can be resumed.
            let id = path.file_stem()?.to_str()?.to_string();
            Uuid::parse_str(&id).ok()?;
            Some((modified(&path)?, id))
        })
        .max_by_key(|(written, _)| *written)
        .map(|(_, id)| id)
}

/// The directory name Claude Code derives from a working directory: the path
/// with every character that is not an ASCII letter or digit replaced by `-`.
fn claude_project_dir_name(working_dir: &Path) -> String {
    working_dir
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Codex keeps one rollout file per conversation, filed by date. The first
/// line of a rollout records the conversation's id and working directory.
fn codex_session_id(
    home: &Path,
    working_dirs: &[PathBuf],
    started_at: SystemTime,
) -> Option<String> {
    let mut rollouts = Vec::new();
    collect_rollouts(
        &home.join(".codex").join("sessions"),
        CODEX_ROLLOUT_DEPTH,
        &mut rollouts,
    );
    rollouts
        .into_iter()
        .filter_map(|path| {
            let written = modified(&path)?;
            if written < started_at {
                return None;
            }
            let id = codex_rollout_session_id(&path, working_dirs)?;
            Some((written, id))
        })
        .max_by_key(|(written, _)| *written)
        .map(|(_, id)| id)
}

/// Gather the `rollout-*.jsonl` files `depth` directories below `dir`.
fn collect_rollouts(dir: &Path, depth: usize, rollouts: &mut Vec<PathBuf>) {
    for path in files_in(dir) {
        if depth > 0 {
            if path.is_dir() {
                collect_rollouts(&path, depth - 1, rollouts);
            }
            continue;
        }
        let is_rollout = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"));
        if is_rollout {
            rollouts.push(path);
        }
    }
}

/// The session id a rollout file records, if it ran in one of `working_dirs`.
fn codex_rollout_session_id(rollout: &Path, working_dirs: &[PathBuf]) -> Option<String> {
    let mut first_line = String::new();
    BufReader::new(fs::File::open(rollout).ok()?)
        .read_line(&mut first_line)
        .ok()?;
    let record: serde_json::Value = serde_json::from_str(&first_line).ok()?;
    if record.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    let meta = record.get("payload")?;
    let working_dir = Path::new(meta.get("cwd")?.as_str()?);
    if !working_dirs.iter().any(|dir| dir == working_dir) {
        return None;
    }
    Some(meta.get("id")?.as_str()?.to_string())
}

/// The entries of `dir`, or none when it cannot be read (it does not exist
/// when the harness has never run).
fn files_in(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SESSION_A: &str = "0c5a8c1e-7d0b-4b5e-9f3a-2f1d6f0f8a01";
    const SESSION_B: &str = "7b1e2a44-3c9d-4f6a-8e21-5a0c9d3e7b02";

    /// A home directory holding a PTY session's working directory, as the
    /// daemon lays it out.
    struct Fixture {
        home: tempfile::TempDir,
        session_dir: PathBuf,
        started_at: DateTime<Utc>,
    }

    impl Fixture {
        fn new() -> Self {
            let home = tempfile::TempDir::new().unwrap();
            // Resolved up front, so the fixture's spelling of the directory is
            // the one a harness would read back.
            let root = home.path().canonicalize().unwrap();
            let session_dir = root
                .join(".nodespace")
                .join("agent-sessions")
                .join("5f0e0c1a-1111-4222-8333-444455556666");
            fs::create_dir_all(&session_dir).unwrap();
            Self {
                home,
                session_dir,
                started_at: Utc::now() - chrono::Duration::minutes(5),
            }
        }

        fn home(&self) -> &Path {
            self.home.path()
        }

        fn find(&self, agent_type: AgentType) -> Option<String> {
            find_harness_session_id(agent_type, self.home(), &self.session_dir, self.started_at)
        }

        /// A conversation file in Claude Code's store for `working_dir`.
        fn claude_conversation(&self, working_dir: &Path, file_name: &str) -> PathBuf {
            let dir = self
                .home()
                .join(".claude")
                .join("projects")
                .join(claude_project_dir_name(working_dir));
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(file_name);
            fs::write(&path, "{\"type\":\"mode\",\"mode\":\"normal\"}\n").unwrap();
            path
        }

        /// A rollout file in Codex's store, recording `working_dir`.
        fn codex_rollout(&self, id: &str, working_dir: &Path) -> PathBuf {
            let dir = self
                .home()
                .join(".codex")
                .join("sessions")
                .join("2026")
                .join("10")
                .join("02");
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("rollout-2026-10-02T09-15-00-{id}.jsonl"));
            let meta = serde_json::json!({
                "timestamp": "2026-10-02T09:15:00.000Z",
                "type": "session_meta",
                "payload": { "id": id, "cwd": working_dir, "cli_version": "0.50.0" }
            });
            fs::write(&path, format!("{meta}\n{{\"type\":\"response_item\"}}\n")).unwrap();
            path
        }
    }

    fn set_modified(path: &Path, when: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn claude_code_names_its_project_directory_after_the_working_directory() {
        assert_eq!(
            claude_project_dir_name(Path::new(
                "/Users/sam/.nodespace/agent-sessions/5f0e0c1a-1111-4222-8333-444455556666"
            )),
            "-Users-sam--nodespace-agent-sessions-5f0e0c1a-1111-4222-8333-444455556666"
        );
    }

    #[test]
    fn a_claude_code_session_is_found_by_its_working_directory() {
        let fixture = Fixture::new();
        fixture.claude_conversation(&fixture.session_dir, &format!("{SESSION_A}.jsonl"));
        // Another session's conversation, in another working directory.
        fixture.claude_conversation(
            &fixture.session_dir.with_file_name("another-session"),
            &format!("{SESSION_B}.jsonl"),
        );

        assert_eq!(
            fixture.find(AgentType::ClaudeCode).as_deref(),
            Some(SESSION_A)
        );
    }

    #[test]
    fn the_claude_code_conversation_written_to_last_is_the_one_returned() {
        let fixture = Fixture::new();
        let first =
            fixture.claude_conversation(&fixture.session_dir, &format!("{SESSION_A}.jsonl"));
        let last = fixture.claude_conversation(&fixture.session_dir, &format!("{SESSION_B}.jsonl"));
        let now = SystemTime::now();
        set_modified(&first, now - Duration::from_secs(120));
        set_modified(&last, now - Duration::from_secs(10));

        assert_eq!(
            fixture.find(AgentType::ClaudeCode).as_deref(),
            Some(SESSION_B)
        );
    }

    #[test]
    fn only_a_claude_code_conversation_file_names_a_session() {
        let fixture = Fixture::new();
        fixture.claude_conversation(&fixture.session_dir, "agent-a1b2c3.jsonl");
        fixture.claude_conversation(&fixture.session_dir, &format!("{SESSION_A}.txt"));
        fs::create_dir(
            fixture
                .home()
                .join(".claude")
                .join("projects")
                .join(claude_project_dir_name(&fixture.session_dir))
                .join(SESSION_B),
        )
        .unwrap();

        assert_eq!(fixture.find(AgentType::ClaudeCode), None);
    }

    #[test]
    fn a_codex_session_is_found_by_the_working_directory_its_rollout_records() {
        let fixture = Fixture::new();
        fixture.codex_rollout(SESSION_A, &fixture.session_dir);
        fixture.codex_rollout(
            SESSION_B,
            &fixture.session_dir.with_file_name("another-session"),
        );

        assert_eq!(fixture.find(AgentType::Codex).as_deref(), Some(SESSION_A));
    }

    #[test]
    fn the_codex_rollout_written_to_last_is_the_one_returned() {
        let fixture = Fixture::new();
        let first = fixture.codex_rollout(SESSION_A, &fixture.session_dir);
        let last = fixture.codex_rollout(SESSION_B, &fixture.session_dir);
        let now = SystemTime::now();
        set_modified(&first, now - Duration::from_secs(120));
        set_modified(&last, now - Duration::from_secs(10));

        assert_eq!(fixture.find(AgentType::Codex).as_deref(), Some(SESSION_B));
    }

    #[test]
    fn a_codex_rollout_last_written_before_the_session_started_is_not_read() {
        let fixture = Fixture::new();
        let rollout = fixture.codex_rollout(SESSION_A, &fixture.session_dir);
        set_modified(
            &rollout,
            SystemTime::from(fixture.started_at) - Duration::from_secs(60),
        );

        assert_eq!(fixture.find(AgentType::Codex), None);
    }

    #[test]
    fn a_codex_file_that_is_not_a_rollout_record_is_ignored() {
        let fixture = Fixture::new();
        let rollout = fixture.codex_rollout(SESSION_A, &fixture.session_dir);
        // Not a session record on its first line.
        fs::write(&rollout, "{\"type\":\"response_item\"}\n").unwrap();
        // Not JSON at all.
        fs::write(
            rollout.with_file_name(format!("rollout-2026-10-02T09-20-00-{SESSION_B}.jsonl")),
            "not json\n",
        )
        .unwrap();
        // A session record, but not in a rollout file.
        let meta = serde_json::json!({
            "type": "session_meta",
            "payload": { "id": SESSION_B, "cwd": fixture.session_dir }
        });
        fs::write(rollout.with_file_name("notes.jsonl"), format!("{meta}\n")).unwrap();

        assert_eq!(fixture.find(AgentType::Codex), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_working_directory_reached_through_a_symlink_is_matched_resolved() {
        let fixture = Fixture::new();
        fixture.claude_conversation(&fixture.session_dir, &format!("{SESSION_A}.jsonl"));
        fixture.codex_rollout(SESSION_B, &fixture.session_dir);
        let link = fixture.session_dir.with_file_name("linked-session");
        std::os::unix::fs::symlink(&fixture.session_dir, &link).unwrap();

        let find = |agent_type| {
            find_harness_session_id(agent_type, fixture.home(), &link, fixture.started_at)
        };
        assert_eq!(find(AgentType::ClaudeCode).as_deref(), Some(SESSION_A));
        assert_eq!(find(AgentType::Codex).as_deref(), Some(SESSION_B));
    }

    #[test]
    fn a_session_the_harness_never_recorded_has_no_id() {
        let fixture = Fixture::new();
        assert_eq!(fixture.find(AgentType::ClaudeCode), None);
        assert_eq!(fixture.find(AgentType::Codex), None);
    }

    #[test]
    fn a_harness_without_a_session_store_has_no_id() {
        let fixture = Fixture::new();
        fixture.claude_conversation(&fixture.session_dir, &format!("{SESSION_A}.jsonl"));
        fixture.codex_rollout(SESSION_B, &fixture.session_dir);

        for agent_type in [
            AgentType::AntigravityCli,
            AgentType::Pi,
            AgentType::OpenCode,
        ] {
            assert_eq!(fixture.find(agent_type), None, "{agent_type:?}");
        }
    }
}
