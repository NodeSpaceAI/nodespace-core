//! The record the daemon leaves behind when it refuses to open its database.
//!
//! Written by `nodespaced` as JSON into the `.nodespace/` state directory under
//! `nodespace_proto::socket::incompatible_database_name`, read by the desktop
//! app to explain why the daemon is not running and to know which file to move
//! aside. Lives here because both sides must agree on its shape and neither
//! depends on the other.

use serde::{Deserialize, Serialize};

/// The daemon's default database was created by a different version of
/// NodeSpace and its tables do not match this version's schema. NodeSpace does
/// not migrate databases, so the only way forward is to move the file aside
/// and start with a fresh one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncompatibleDatabase {
    /// Absolute path of the database file the daemon refused to open.
    pub database_path: String,
    /// Which tables differ and how, for logs and support. Not meant to be
    /// shown to a user as the headline.
    pub detail: String,
    /// When the daemon refused the database, RFC 3339.
    pub detected_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_camel_case_json() {
        let record = IncompatibleDatabase {
            database_path: "/home/u/.nodespace/database/nodespace.db".to_string(),
            detail: "relationship: missing reverse_relationship_type".to_string(),
            detected_at: "2026-09-28T10:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(
            json["databasePath"],
            "/home/u/.nodespace/database/nodespace.db"
        );
        assert_eq!(json["detectedAt"], "2026-09-28T10:00:00Z");
        let back: IncompatibleDatabase = serde_json::from_value(json).unwrap();
        assert_eq!(back, record);
    }
}
