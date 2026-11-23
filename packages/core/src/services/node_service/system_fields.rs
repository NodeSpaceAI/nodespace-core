//! Who may write a `system` field's value (Schema Management, Protection;
//! ADR-112 section 12).
//!
//! A field whose `protection` is `system` is maintained by code: the chat
//! engines, the playbook engine, the query runner, an extension. Its value is
//! written only by that code, so every node write names no such field:
//!
//! - The ordinary write paths (`create_node`, `update_node`, the bulk and
//!   hierarchy creates, markdown import) refuse a write that sets or changes a
//!   `system` field, naming the field and the type. The agent's tools, the
//!   Tauri commands and the CLI all reach the store through these, so each is
//!   refused by the one check ([`NodeService::ensure_system_fields_maintained`]).
//!   A write that carries a field's stored value back unchanged is not a change,
//!   so a client that sends a node back whole is not refused for it.
//! - The code that maintains a field writes it through
//!   [`NodeService::maintaining_system_fields`], which runs a write with the
//!   check lifted for exactly that write. It is not exposed by any Tauri
//!   command, CLI command or agent tool, and a test holds each surface to it
//!   (`system_field_write_surfaces_test`).

use super::crud::FieldOwnershipInfo;
use super::NodeService;
use crate::models::{Node, SchemaProtectionLevel};
use crate::services::error::NodeServiceError;
use serde_json::Value;
use std::future::Future;

tokio::task_local! {
    /// Set while a [`NodeService::maintaining_system_fields`] write runs.
    static MAINTAINING_SYSTEM_FIELDS: ();
}

/// A field's value in `properties`: the one in the nearest bucket of the node's
/// type chain that holds the key, as a read of the node resolves it. `null` is
/// how a cleared field is stored, so it reads as no value.
fn stored_in_chain<'a>(properties: &'a Value, chain: &[String], name: &str) -> Option<&'a Value> {
    let buckets = properties.as_object()?;
    chain
        .iter()
        .filter_map(|scope| buckets.get(scope.as_str()).and_then(Value::as_object))
        .find_map(|bucket| bucket.get(name))
        .filter(|value| !value.is_null())
}

impl NodeService {
    /// Run a write with the `system` field check lifted, for the code that
    /// maintains those fields: a chat engine writing a message's `timestamp`,
    /// the chat card service recording a confirmation, the playbook engine
    /// recording a suspension, an extension writing its own `system` custom
    /// fields (ADR-112 section 12).
    ///
    /// Generic on purpose: it names no field and no type, so a field's
    /// maintainer is whichever code calls this, and the schema's `system`
    /// mark is what keeps every other writer out. Keep the future narrow: only
    /// the write the maintainer owes, never a callback into code that takes
    /// input from a user. The scope belongs to the future it wraps; a task it
    /// spawns is not inside it.
    ///
    /// Nothing user-facing calls this. The agent's tools, the Tauri commands
    /// and the CLI reach the node write paths only through the
    /// daemon and `ops`, which do not.
    pub async fn maintaining_system_fields<F: Future>(&self, write: F) -> F::Output {
        MAINTAINING_SYSTEM_FIELDS.scope((), write).await
    }

    /// Whether the current write is inside [`Self::maintaining_system_fields`].
    fn is_maintaining_system_fields() -> bool {
        MAINTAINING_SYSTEM_FIELDS.try_with(|_| ()).is_ok()
    }

    /// Refuse a write that sets or changes a `system` field's value.
    ///
    /// `before` is the node as stored (`None` on a create) and `node` the node
    /// as the write would leave it, so the check sees the merged value however
    /// the caller named the field: flat, or under its declaring type's bucket.
    /// Run it before schema defaults are applied: a default the schema fills in
    /// is not a value the caller wrote.
    ///
    /// A play's suspension fields are settled by
    /// [`Self::settle_play_update`] and [`Self::ensure_play_created_unsuspended`],
    /// which also let an enabling write clear them, so they are left to those.
    pub(crate) fn ensure_system_fields_maintained(
        before: Option<&Node>,
        node: &Node,
        ownership: &FieldOwnershipInfo,
    ) -> Result<(), NodeServiceError> {
        if Self::is_maintaining_system_fields() {
            return Ok(());
        }
        let (fields, owners, chain) = ownership;
        let play_owned = crate::models::CoreNodeType::Play.as_str();
        for field in fields
            .iter()
            .filter(|f| f.protection == SchemaProtectionLevel::System)
        {
            let owner = owners.get(&field.name).map(String::as_str);
            if owner == Some(play_owned)
                && nodespace_types::PLAY_SUSPENSION_FIELDS.contains(&field.name.as_str())
            {
                continue;
            }
            let after = stored_in_chain(&node.properties, chain, &field.name);
            let was = before.and_then(|b| stored_in_chain(&b.properties, chain, &field.name));
            if after == was {
                continue;
            }
            let declared_by = owner.unwrap_or(&node.node_type);
            return Err(NodeServiceError::invalid_update(match before {
                None => format!(
                    "'{}' is a system field of the type '{declared_by}': the code that maintains \
                     it writes it, so it can't be set on a new {} node",
                    field.name, node.node_type
                ),
                Some(_) => format!(
                    "'{}' is a system field of the type '{declared_by}': the code that maintains \
                     it writes it, so a {} node's update can't change it",
                    field.name, node.node_type
                ),
            }));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::SqliteStore;
    use crate::models::{Node, NodeUpdate};
    use crate::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;

    /// The in-transaction update path, which only the crate can reach, holds
    /// the same line as the public ones.
    #[tokio::test]
    async fn the_in_transaction_update_refuses_a_system_field() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("t.db")).await.unwrap());
        let svc = Arc::new(NodeService::new(&mut store).await.unwrap());
        let id = svc
            .create_node(Node::new(
                "ai-chat-pty".into(),
                "Session".into(),
                json!({ "agent": "claude-code" }),
            ))
            .await
            .unwrap();
        let patch = || NodeUpdate::new().with_properties(json!({ "summary": "forged" }));
        let inner = Arc::clone(&svc);
        let err = svc
            .with_transaction(move |tx| {
                let id = id.clone();
                Box::pin(async move { inner.update_node_in_tx(tx, &id, patch()).await.map(|_| ()) })
            })
            .await
            .expect_err("a system field is the maintainer's");
        let message = err.to_string();
        assert!(
            message.contains("'summary'") && message.contains("'ai-chat'"),
            "{message}"
        );
    }
}
