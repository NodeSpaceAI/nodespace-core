//! The record of each release's built-in shape, and the classification of
//! every change between releases by what it would require of an existing
//! database (ADR-104).
//!
//! The record only describes and classifies. It moves no data, stores no
//! version in a database and changes nothing about opening one.
//!
//! - [`snapshot`]: the shape snapshot, generated from the table DDL, the
//!   seeded core schemas and the shipped seeds, with a canonical hash.
//! - [`change_log`]: the hand-classified change log, its format, and the
//!   difference between two snapshots.
//! - [`gate`]: the check that keeps the log complete.
//!
//! The files live in `packages/core/shape-record/`: `current.json` is the
//! generated snapshot of this build (embedded below), `snapshots/` holds the
//! snapshot each release shipped, and `change-log.json` is the log.

pub mod change_log;
pub mod gate;
pub mod identify;
pub mod snapshot;

/// The snapshot of this build's shape, as committed and embedded in the
/// binary.
pub const EMBEDDED_SNAPSHOT: &str = include_str!("../../shape-record/current.json");

/// The change log, as committed and embedded in the binary.
pub const EMBEDDED_CHANGE_LOG: &str = include_str!("../../shape-record/change-log.json");

/// The canonical hash recorded in the embedded snapshot: the shape this build
/// ships, as [`identify::hash_of_database`] computes it from a database.
pub fn embedded_shape_hash() -> Option<String> {
    let snapshot: serde_json::Value = serde_json::from_str(EMBEDDED_SNAPSHOT).ok()?;
    snapshot.get("hash")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The snapshot embedded in the binary is the one this build generates,
    /// and the embedded log is a valid log.
    #[tokio::test]
    async fn the_embedded_snapshot_is_what_the_build_generates() {
        assert_eq!(EMBEDDED_SNAPSHOT, snapshot::generate().await.unwrap());
        assert!(embedded_shape_hash().is_some_and(|h| h.len() == 64));
        let log = change_log::ChangeLog::parse(EMBEDDED_CHANGE_LOG).unwrap();
        assert_eq!(change_log::validate(&log), Vec::<String>::new());
    }
}
