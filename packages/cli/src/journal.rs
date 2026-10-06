//! The write journal: a session's own writes, kept by the CLI.
//!
//! A harness plugin watches the item a session is working on and has to tell
//! the session's own writes from someone else's. It sets
//! [`SESSION_ENV`] to name the session; every process the harness starts
//! inherits it, so a `nodespace` command that writes a node, wherever it runs
//! (a script, a loop, a background job), appends the node id, the database and
//! the node's new version to `<state dir>/journals/<session>.jsonl`. The plugin
//! reads the file when the watched item's version moved: a version the file
//! holds is the session's own.
//!
//! The file holds ids and versions, never content, and lives outside the
//! database: core records nothing about who changed a node. A journal that
//! cannot be read or written never fails the command or changes its output.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;

/// The variable that names the session whose journal a write is appended to.
pub const SESSION_ENV: &str = "NODESPACE_WRITE_JOURNAL";

/// A journal not appended to for this long belongs to a session that is gone.
/// An entry is only read until the plugin next compares the item, so a live
/// session that loses an old file loses nothing it still needs.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

const EXTENSION: &str = "jsonl";

#[derive(Subcommand, Debug)]
pub enum JournalAction {
    /// Remove a session's write journal, and every journal no session has
    /// written to for an hour. A harness plugin runs this when a session
    /// starts and when it ends.
    End {
        /// The session the journal is named for.
        session: String,
    },
}

pub fn run(action: JournalAction) -> Result<()> {
    match action {
        JournalAction::End { session } => {
            if let Some(dir) = journals_dir() {
                end_in(&dir, &session, SystemTime::now());
            }
            Ok(())
        }
    }
}

fn journals_dir() -> Option<PathBuf> {
    nodespace_daemon::nodespace_dir()
        .ok()
        .map(|dir| dir.join("journals"))
}

/// A session id is a file name: only the characters an id is made of.
fn is_session_id(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 128
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn journal_path(dir: &Path, session: &str) -> Option<PathBuf> {
    is_session_id(session).then(|| dir.join(format!("{session}.{EXTENSION}")))
}

/// Removes `session`'s journal and the stale ones. Never fails.
fn end_in(dir: &Path, session: &str, now: SystemTime) {
    if let Some(path) = journal_path(dir, session) {
        let _ = fs::remove_file(path);
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_journal = path.extension().is_some_and(|ext| ext == EXTENSION);
        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok());
        if is_journal && age.is_some_and(|age| age >= STALE_AFTER) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Where this command's writes are recorded: nowhere, unless the harness named
/// a session.
pub struct WriteJournal {
    path: Option<PathBuf>,
    database: String,
}

impl WriteJournal {
    /// The journal of the session `SESSION_ENV` names, for writes to `database`.
    pub fn from_env(database: &str) -> Self {
        let session = std::env::var(SESSION_ENV).unwrap_or_default();
        let path = journals_dir().and_then(|dir| journal_path(&dir, &session));
        Self::at(path, database)
    }

    /// A journal at `path` (`None` records nothing), for writes to `database`.
    pub fn at(path: Option<PathBuf>, database: &str) -> Self {
        Self {
            path,
            database: database.to_string(),
        }
    }

    /// Appends one line saying `node_id` is now at `version`. One `write` of a
    /// whole line to a file opened for appending, so concurrent commands never
    /// interleave inside a line.
    pub fn record(&self, node_id: &str, version: i64) {
        let Some(path) = &self.path else {
            return;
        };
        let line = format!(
            "{}\n",
            json!({ "node_id": node_id, "database": self.database, "version": version })
        );
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn a_write_is_one_line_of_ids_and_a_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_path(dir.path(), "s-1").unwrap();
        let journal = WriteJournal::at(Some(path.clone()), "db-1");

        journal.record("n-1", 4);
        journal.record("n-1", 5);

        assert_eq!(
            lines(&path),
            vec![
                json!({ "node_id": "n-1", "database": "db-1", "version": 4 }),
                json!({ "node_id": "n-1", "database": "db-1", "version": 5 }),
            ]
        );
    }

    #[test]
    fn with_no_session_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let journal = WriteJournal::at(journal_path(dir.path(), ""), "db-1");

        journal.record("n-1", 4);

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_session_id_cannot_name_a_path() {
        for id in ["", "../x", "a/b", "a.b", &"x".repeat(129)] {
            assert_eq!(journal_path(Path::new("/j"), id), None, "{id}");
        }
        assert!(journal_path(Path::new("/j"), "0a1b-2c_3d").is_some());
    }

    #[test]
    fn a_journal_that_cannot_be_written_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("file");
        fs::write(&blocked, "").unwrap();

        // The journals directory would have to sit under a regular file.
        WriteJournal::at(Some(blocked.join("journals").join("s.jsonl")), "db").record("n", 1);
    }

    #[test]
    fn concurrent_appends_never_split_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = journal_path(dir.path(), "s-1").unwrap();
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let journal = WriteJournal::at(Some(path.clone()), "db-1");
                std::thread::spawn(move || {
                    for version in 0..50 {
                        journal.record(&format!("n-{worker}"), version);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(lines(&path).len(), 400);
    }

    #[test]
    fn ending_a_session_removes_its_journal_and_the_stale_ones() {
        let dir = tempfile::tempdir().unwrap();
        for session in ["mine", "crashed", "live"] {
            WriteJournal::at(journal_path(dir.path(), session), "db").record("n", 1);
        }
        fs::write(dir.path().join("notes.txt"), "kept").unwrap();

        // An hour and a minute on: every journal written now is stale, but
        // the one just touched is not.
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);
        WriteJournal::at(journal_path(dir.path(), "live"), "db").record("n", 2);
        end_in(dir.path(), "mine", SystemTime::now());

        assert!(!dir.path().join("mine.jsonl").exists());
        assert!(dir.path().join("crashed.jsonl").exists());
        assert!(dir.path().join("live.jsonl").exists());

        end_in(dir.path(), "mine", later);

        assert!(!dir.path().join("crashed.jsonl").exists());
        assert!(!dir.path().join("live.jsonl").exists());
        assert!(dir.path().join("notes.txt").exists());
    }
}
