//! Prototype: which snapshot's shape does a database hold, read from the
//! database's contents alone (ADR-104).
//!
//! Nothing here is called when a database is opened, and nothing is written
//! to one. The hash is the one a snapshot records ([`super::snapshot`]): the
//! tables and the core schemas, with what a user may change taken out, so it
//! identifies a shape, not a release.

use anyhow::Result;

use super::snapshot::{schemas_view, shape_hash, tables_view};
use crate::db::SqliteStore;

/// The canonical hash of the shape the database behind `store` holds: its
/// tables and its core schemas as stored.
pub async fn hash_of_database(store: &SqliteStore) -> Result<String> {
    let conn = store.read().await?;
    let tables = crate::db::schema::tables_of(conn.connection()).await?;
    drop(conn);
    let core: Vec<_> = store
        .get_all_schemas()
        .await?
        .into_iter()
        .filter(|s| s.is_core)
        .collect();
    Ok(shape_hash(&tables_view(&tables), &schemas_view(&core)))
}
