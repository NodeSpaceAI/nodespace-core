//! `SqliteStore` methods — resolving a node's type through its `extends` chain
//! (ADR-086 §5).
//!
//! Code that applies a type's rule never compares `node_type` with a string:
//! a subtype would fall out of the rule. It asks here for the type's chain or
//! its nearest core ancestor, read from the `type_ancestry` table that the
//! schema triggers keep in step with the `extends` edges.
use super::*;
use crate::db::schema::TYPE_ANCESTRY_TABLE;
use crate::models::CoreNodeType;

/// The chain of a type no schema describes: just itself.
fn unextended(node_type: &str) -> Vec<String> {
    vec![node_type.to_string()]
}

/// A core type's chain needs no read: the registry holds it, and nothing can
/// make a core type extend anything else.
fn core_chain(node_type: &str) -> Option<Vec<String>> {
    CoreNodeType::from_id(node_type).map(|core| {
        core.chain()
            .into_iter()
            .map(|t| t.as_str().to_string())
            .collect()
    })
}

/// A type with no ancestry rows has no schema, so it is its own chain.
fn chain_or_unextended(chain: Vec<String>, node_type: &str) -> Vec<String> {
    if chain.is_empty() {
        unextended(node_type)
    } else {
        chain
    }
}

fn chain_sql() -> String {
    format!("SELECT ancestor FROM {TYPE_ANCESTRY_TABLE} WHERE node_type = ?1 ORDER BY depth")
}

impl SqliteStore {
    /// `node_type`'s `extends` chain, nearest scope first: `["issue", "task"]`
    /// for an issue extending task, `["task"]` for task itself. A type with no
    /// schema is its own chain.
    pub async fn type_chain(&self, node_type: &str) -> Result<Vec<String>> {
        if let Some(chain) = core_chain(node_type) {
            return Ok(chain);
        }
        let mut rows = self
            .read()
            .await?
            .query(&chain_sql(), libsql::params![node_type])
            .await
            .context("Failed to read a type's ancestry")?;
        let mut chain = Vec::new();
        while let Some(row) = rows.next().await? {
            chain.push(row.get::<String>(0)?);
        }
        Ok(chain_or_unextended(chain, node_type))
    }

    /// `_in_tx` twin of [`Self::type_chain`]: reads on the transaction's own
    /// connection, so it sees a schema or `extends` edge the transaction wrote.
    pub(crate) async fn type_chain_in_tx(tx: &Tx<'_>, node_type: &str) -> Result<Vec<String>> {
        if let Some(chain) = core_chain(node_type) {
            return Ok(chain);
        }
        let mut rows = tx
            .conn()
            .query(&chain_sql(), libsql::params![node_type])
            .await
            .context("Failed to read a type's ancestry in transaction")?;
        let mut chain = Vec::new();
        while let Some(row) = rows.next().await? {
            chain.push(row.get::<String>(0)?);
        }
        Ok(chain_or_unextended(chain, node_type))
    }

    /// The nearest core type in `node_type`'s chain: the type itself when it
    /// is core, else its closest core ancestor. `None` for a user-defined type
    /// that extends no core type.
    pub async fn core_type_of(&self, node_type: &str) -> Result<Option<CoreNodeType>> {
        if let Some(core) = CoreNodeType::from_id(node_type) {
            return Ok(Some(core));
        }
        Ok(CoreNodeType::nearest(&self.type_chain(node_type).await?))
    }

    /// `_in_tx` twin of [`Self::core_type_of`].
    pub(crate) async fn core_type_of_in_tx(
        tx: &Tx<'_>,
        node_type: &str,
    ) -> Result<Option<CoreNodeType>> {
        if let Some(core) = CoreNodeType::from_id(node_type) {
            return Ok(Some(core));
        }
        Ok(CoreNodeType::nearest(
            &Self::type_chain_in_tx(tx, node_type).await?,
        ))
    }

    /// Whether `node_type` is `base` or extends it, however far down the
    /// chain.
    pub async fn type_is_a(&self, node_type: &str, base: CoreNodeType) -> Result<bool> {
        Ok(self
            .core_type_of(node_type)
            .await?
            .is_some_and(|core| core.is_a(base)))
    }

    /// `_in_tx` twin of [`Self::type_is_a`].
    pub(crate) async fn type_is_a_in_tx(
        tx: &Tx<'_>,
        node_type: &str,
        base: CoreNodeType,
    ) -> Result<bool> {
        Ok(Self::core_type_of_in_tx(tx, node_type)
            .await?
            .is_some_and(|core| core.is_a(base)))
    }
}

impl SqliteStore {
    /// Whether `node_type` is abstract: declared so by its schema, or by the
    /// registry for a core type. No node is created with an abstract type or
    /// retyped into one (ADR-086 §6).
    pub async fn is_abstract_type(&self, node_type: &str) -> Result<bool> {
        // The registry is the authority for a core type, and needs no read.
        if let Some(core) = CoreNodeType::from_id(node_type) {
            return Ok(core.is_abstract());
        }
        let mut rows = self
            .read()
            .await?
            .query(
                &format!(
                    "SELECT 1 FROM node WHERE id = ?1 AND {} \
                     AND json_type(properties, '$.abstract') = 'true'",
                    crate::db::schema::is_exactly_sql("node_type", CoreNodeType::Schema)
                ),
                libsql::params![node_type],
            )
            .await
            .context("Failed to read whether a type is abstract")?;
        Ok(rows.next().await?.is_some())
    }

    /// Whether any node has exactly `node_type` as its type. Subtypes do not
    /// count: this asks about the type itself, for the check that an abstract
    /// type has no instances of its own.
    pub async fn has_nodes_of_exact_type(&self, node_type: &str) -> Result<bool> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT 1 FROM node WHERE node_type = ?1 LIMIT 1",
                libsql::params![node_type],
            )
            .await
            .context("Failed to check for nodes of a type")?;
        Ok(rows.next().await?.is_some())
    }
}
