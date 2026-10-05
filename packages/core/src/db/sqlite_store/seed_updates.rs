//! `SqliteStore` methods for the pending seed updates table (ADR-094 §8).
//! See `db::schema` for the table shape and
//! `services::node_service::seed_updates` for the reconciliation that writes
//! it and the choices that clear it.
use super::*;
use crate::models::seed_update::{PendingSeedUpdateRow, SeedAspect};
use std::str::FromStr;

impl SqliteStore {
    /// Record that `aspect` of the seeded node `node_id` has a shipped version
    /// the user has not decided on. Recording the same version again changes
    /// nothing, so a row keeps the time it was first recorded across restarts;
    /// a newer shipped version replaces the row.
    pub async fn record_pending_seed_update(
        &self,
        node_id: &str,
        aspect: SeedAspect,
        shipped_version: &str,
    ) -> Result<()> {
        self.write()
            .await
            .execute(
                "INSERT INTO pending_seed_update (node_id, aspect, shipped_version, recorded_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (node_id, aspect) DO UPDATE SET \
                     shipped_version = excluded.shipped_version, \
                     recorded_at = excluded.recorded_at \
                 WHERE pending_seed_update.shipped_version <> excluded.shipped_version",
                libsql::params![
                    node_id.to_string(),
                    aspect.as_str(),
                    shipped_version.to_string(),
                    Utc::now().to_rfc3339()
                ],
            )
            .await
            .context("Failed to record pending seed update")?;
        Ok(())
    }

    /// Remove the pending row for `aspect` of `node_id`. Returns whether one
    /// existed.
    pub async fn clear_pending_seed_update(
        &self,
        node_id: &str,
        aspect: SeedAspect,
    ) -> Result<bool> {
        let removed = self
            .write()
            .await
            .execute(
                "DELETE FROM pending_seed_update WHERE node_id = ?1 AND aspect = ?2",
                libsql::params![node_id.to_string(), aspect.as_str()],
            )
            .await
            .context("Failed to clear pending seed update")?;
        Ok(removed > 0)
    }

    /// Every pending row, oldest first.
    pub async fn list_pending_seed_updates(&self) -> Result<Vec<PendingSeedUpdateRow>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT node_id, aspect, shipped_version, recorded_at FROM pending_seed_update \
                 ORDER BY recorded_at, node_id, aspect",
                (),
            )
            .await
            .context("Failed to list pending seed updates")?;

        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            records.push(row_to_pending_seed_update(&row)?);
        }
        Ok(records)
    }

    /// The pending row for `aspect` of `node_id`, if there is one.
    pub async fn get_pending_seed_update(
        &self,
        node_id: &str,
        aspect: SeedAspect,
    ) -> Result<Option<PendingSeedUpdateRow>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT node_id, aspect, shipped_version, recorded_at FROM pending_seed_update \
                 WHERE node_id = ?1 AND aspect = ?2",
                libsql::params![node_id.to_string(), aspect.as_str()],
            )
            .await
            .context("Failed to read pending seed update")?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_pending_seed_update(&row)?)),
            None => Ok(None),
        }
    }
}

fn row_to_pending_seed_update(row: &libsql::Row) -> Result<PendingSeedUpdateRow> {
    let node_id: String = row.get(0)?;
    let aspect: String = row.get(1)?;
    let shipped_version: String = row.get(2)?;
    let recorded_at: String = row.get(3)?;

    Ok(PendingSeedUpdateRow {
        node_id,
        aspect: SeedAspect::from_str(&aspect)
            .map_err(|e| anyhow::anyhow!("corrupt pending_seed_update.aspect: {e}"))?,
        shipped_version,
        recorded_at: DateTime::parse_from_rfc3339(&recorded_at)
            .context("corrupt pending_seed_update.recorded_at")?
            .with_timezone(&Utc),
    })
}
