//! Regression coverage for a real, reproduced schema-bootstrap race: two
//! writer connections opening concurrently against the same brand-new
//! database file, both running `create_schema`.
//!
//! This is the caller shape `DatabaseManager::get_or_open` (in the daemon)
//! used to allow for two concurrent requests naming the same not-yet-open
//! database — "the assembly runs outside the `open` lock... then a re-check
//! keeps whichever handle landed first and shuts the other one down" — before
//! it was changed to serialize per-id. Even with that daemon-level fix,
//! `SqliteStore::new` itself must stay safe under this shape: nothing stops a
//! future caller (a second daemon process, a test harness, a not-yet-written
//! code path) from opening the same file twice at once, and the fix belongs
//! at the layer that actually talks to SQLite, not only one caller above it.
//!
//! Before the fix (`apply_writer_pragmas` setting `busy_timeout` AFTER
//! `journal_mode`, and `create_schema` running as a sequence of separate
//! autocommit statements rather than one transaction), this reliably failed:
//! roughly 30% of concurrent opens hit `SQLite failure: \`database is
//! locked\`` on the `journal_mode` pragma, because the ordinary write-lock
//! contention between two connections racing to create the same schema had
//! no retry grace (default `busy_timeout` is 0). 500 rounds x 8 concurrent
//! opens (4000 total) produced 1186 failures pre-fix; 0 post-fix.

use nodespace_core::db::SqliteStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_sqlitestore_new_against_the_same_path_never_fails() {
    let mut failures: Vec<String> = Vec::new();
    let rounds = 40;
    for round in 0..rounds {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("nodespace.db");
        let mut handles = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            handles.push(tokio::spawn(async move {
                SqliteStore::new(path).await.map(|_| ())
            }));
        }
        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(format!("round {round}: {e:?}")),
                Err(e) => failures.push(format!("round {round} panicked: {e:?}")),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} / {} concurrent SqliteStore::new opens against the same path failed \
         (schema creation must be safe under this exact race — see this test's \
         module doc comment):\n{}",
        failures.len(),
        rounds * 8,
        failures.join("\n"),
    );
}
