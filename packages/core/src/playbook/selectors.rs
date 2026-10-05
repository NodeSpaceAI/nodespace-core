//! Play selectors: which nodes a trigger applies to, said the way a query
//! says it (ADR-086 §11).
//!
//! A selector is a type and filters written into the rule, or a reference to
//! a saved `query` node. Either way it runs as one query, in SQL, through the
//! same [`QueryService`] a query node runs through.

use crate::models::{CoreNodeType, Node, QueryFields};
use crate::ops::query_ops::resolve_filters;
use crate::playbook::types::Selector;
use crate::services::{NodeService, QueryDefinition, QueryService};

/// Why a selector could not be read or run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    /// The saved query the selector names does not exist.
    #[error("saved query '{query_id}' does not exist")]
    QueryNotFound { query_id: String },
    /// The node the selector names is not a query.
    #[error("'{query_id}' is a '{node_type}' node, not a query")]
    NotAQuery { query_id: String, node_type: String },
    /// The selector's filters cannot be run: a stored query that does not
    /// decode, or a relationship path the schemas do not declare.
    #[error("{0}")]
    Invalid(String),
    /// A lookup failed. Not a statement about the selector.
    #[error("{0}")]
    Lookup(String),
}

/// The query a selector stands for: its target type and filters, with every
/// relationship path resolved against the schemas.
///
/// A saved query contributes its target type and filters only. Its sorting,
/// limit and view describe how a viewer shows it, and a play selects every
/// matching node, so they play no part.
pub async fn selector_query(
    node_service: &NodeService,
    select: &Selector,
) -> Result<QueryDefinition, SelectorError> {
    let (target_type, filters) = match select {
        Selector::Inline(inline) => (inline.target_type.clone(), inline.filters.clone()),
        Selector::Query(saved) => {
            let node = node_service
                .get_node(&saved.query_id)
                .await
                .map_err(|e| SelectorError::Lookup(e.to_string()))?
                .ok_or_else(|| SelectorError::QueryNotFound {
                    query_id: saved.query_id.clone(),
                })?;
            let is_query = node_service
                .type_is_a(&node.node_type, CoreNodeType::Query)
                .await
                .map_err(|e| SelectorError::Lookup(e.to_string()))?;
            if !is_query {
                return Err(SelectorError::NotAQuery {
                    query_id: saved.query_id.clone(),
                    node_type: node.node_type,
                });
            }
            let fields = QueryFields::from_properties(&node.properties).map_err(|e| {
                SelectorError::Invalid(format!("saved query '{}': {e}", saved.query_id))
            })?;
            (fields.target_type, fields.filters)
        }
    };

    // A rule selects the same nodes on every device, and a relative date is
    // the local day of whichever device asks (ADR-091).
    if filters.iter().any(|filter| filter.has_relative_date()) {
        return Err(SelectorError::Invalid(
            "a play's selector cannot filter by a date relative to today: the local day \
             differs between devices, and a rule must select the same nodes on each. Compare \
             the date in a rule condition instead, with today()."
                .to_string(),
        ));
    }

    // Names are checked for shape before they are resolved, so a malformed
    // one is reported as malformed and not as undeclared.
    let mut query = QueryDefinition {
        target_type,
        filters,
        sorting: None,
        limit: None,
    };
    query
        .validate_identifiers()
        .map_err(|e| SelectorError::Invalid(e.to_string()))?;

    query.filters = resolve_filters(node_service, &query.target_type, query.filters)
        .await
        .map_err(|e| match e {
            crate::ops::OpsError::Internal(message) => SelectorError::Lookup(message),
            other => SelectorError::Invalid(other.to_string()),
        })?;
    Ok(query)
}

/// Run a selector: the type it selects, and every participating node it
/// matches. The filters run in SQL: one statement finds the matching ids and
/// one batched read loads them, however many nodes match.
///
/// Archived nodes never match: the query service applies the governance
/// participation check to every query it runs (ADR-087), and a play acts only
/// on nodes that participate.
pub async fn select_nodes(
    node_service: &NodeService,
    select: &Selector,
) -> Result<(String, Vec<Node>), SelectorError> {
    let query = selector_query(node_service, select).await?;
    let nodes = QueryService::new(node_service.store().clone())
        .execute(&query)
        .await
        .map_err(|e| SelectorError::Lookup(e.to_string()))?;
    Ok((query.target_type, nodes))
}

/// Whether a selector matches one particular node, and the type it selects
/// when it does. `Ok(None)` when the node is not selected.
///
/// The same query [`select_nodes`] runs, narrowed to the node's id, so "would
/// this scan pick this node up" and the scan itself cannot disagree.
pub async fn selects_node(
    node_service: &NodeService,
    select: &Selector,
    node: &Node,
) -> Result<Option<String>, SelectorError> {
    if !crate::governance::participates(node) {
        return Ok(None);
    }
    // A bare type selects by the node's type alone, which the node in hand
    // answers without a query: the selected type is the node's own or one it
    // extends.
    if let Selector::Inline(inline) = select {
        if inline.filters.is_empty() {
            let chain = node_service
                .resolve_type_chain(&node.node_type)
                .await
                .map_err(|e| SelectorError::Lookup(e.to_string()))?;
            let selected = inline.target_type == "*" || chain.contains(&inline.target_type);
            return Ok(selected.then(|| inline.target_type.clone()));
        }
    }
    let query = selector_query(node_service, select).await?;
    let matches = QueryService::new(node_service.store().clone())
        .matches(&query, &node.id)
        .await
        .map_err(|e| SelectorError::Lookup(e.to_string()))?;
    Ok(matches.then_some(query.target_type))
}
