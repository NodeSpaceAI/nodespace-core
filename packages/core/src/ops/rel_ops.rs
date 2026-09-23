//! Relationship Operations
//!
//! Typed orchestration for relationship CRUD. Extracted from MCP handlers.

use crate::models::schema::{
    builtin_forward_name, EdgeField, RelationshipCardinality, BUILTIN_RELATIONSHIP_NAMES,
};
use crate::ops::OpsError;
use crate::services::NodeService;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct CreateRelInput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
    pub edge_data: Option<Value>,
}

#[derive(Debug)]
pub struct CreateRelOutput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
}

#[derive(Debug)]
pub struct DeleteRelInput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
}

#[derive(Debug)]
pub struct UpdateRelPropsInput {
    pub source_id: String,
    pub relationship_name: String,
    pub target_id: String,
    /// The new edge attributes; replaces the edge's stored `properties` wholesale.
    pub properties: Value,
}

#[derive(Debug)]
pub struct GetRelatedInput {
    pub node_id: String,
    pub relationship_name: String,
    /// "out" (forward) or "in" (reverse). Ignored, silently, when
    /// `relationship_name` resolves to [`ResolvedRelName::Reverse`] — a
    /// reverse name already names one specific traversal, so there is
    /// nothing left for this field to select. See [`get_related_nodes`]'s
    /// doc comment for why.
    pub direction: String,
}

#[derive(Debug)]
pub struct GetRelatedOutput {
    pub node_id: String,
    pub relationship_name: String,
    pub direction: String,
    pub related_nodes: Vec<Value>,
    pub count: usize,
}

// ============================================================================
// Operations
// ============================================================================

/// Create a relationship edge between two nodes.
pub async fn create_relationship(
    node_service: &Arc<NodeService>,
    input: CreateRelInput,
) -> Result<CreateRelOutput, OpsError> {
    let edge_data = input.edge_data.unwrap_or(json!({}));

    node_service
        .create_relationship(
            &input.source_id,
            &input.relationship_name,
            &input.target_id,
            edge_data,
        )
        .await
        .map_err(OpsError::from)?;

    Ok(CreateRelOutput {
        source_id: input.source_id,
        relationship_name: input.relationship_name,
        target_id: input.target_id,
    })
}

/// Delete a relationship edge. Idempotent.
pub async fn delete_relationship(
    node_service: &Arc<NodeService>,
    input: DeleteRelInput,
) -> Result<(), OpsError> {
    node_service
        .delete_relationship(&input.source_id, &input.relationship_name, &input.target_id)
        .await
        .map_err(OpsError::from)?;

    Ok(())
}

/// Replace the edge attributes on an existing typed relationship edge.
pub async fn update_relationship_properties(
    node_service: &Arc<NodeService>,
    input: UpdateRelPropsInput,
) -> Result<(), OpsError> {
    node_service
        .update_relationship_properties(
            &input.source_id,
            &input.relationship_name,
            &input.target_id,
            input.properties,
        )
        .await
        .map_err(OpsError::from)?;

    Ok(())
}

// ============================================================================
// Relationship name resolution
// ============================================================================

/// How a caller-supplied relationship name matched the schema neighbourhood of
/// the node being traversed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedRelName {
    /// A built-in structural name (`has_child`, `mentions`, …), legal between
    /// any two nodes without being declared. Traversed exactly as given.
    Builtin,
    /// The forward `name` of a relationship declared on the node's own schema.
    /// Traversed exactly as given.
    Forward,
    /// A reverse name: either the `reverse_name` of a relationship declared by a
    /// schema targeting this node's type, or a built-in's fixed inverse
    /// (`child_of`, `has_member`, …). Edges are stored under the FORWARD name,
    /// so traversing by this name rewrites the name and always queries it
    /// inbound (`direction: "in"`) — a reverse name names exactly one
    /// traversal, so the caller-supplied direction plays no part in choosing
    /// it (see [`get_related_nodes`]'s doc comment).
    Reverse {
        /// The forward name the edges are actually stored under.
        forward_name: String,
        /// The type declaring the forward relationship (the far end here), used
        /// to narrow results when several schemas declare the same forward name
        /// toward this type.
        ///
        /// `None` for a built-in, which has no declaring schema and is legal
        /// between any two node types — narrowing by type would discard
        /// legitimate answers rather than disambiguate them.
        source_type: Option<String>,
    },
    /// The forward `name` of a relationship declared by another schema that
    /// targets this node's type — the node sits at the edge's far end. Already
    /// traversable as given (with `direction: "in"`), so it passes through
    /// unchanged; recognised here only so it is not mistaken for undeclared.
    InboundForward,
}

/// Resolve a caller-supplied relationship name against the schemas reachable
/// from `node_type`, so a declared `reverse_name` traverses the inverse edge
/// rather than silently matching nothing.
///
/// A `reverse_name` names a real, traversable direction — it is the name the
/// schema author chose for "the other way round", and answering it is the whole
/// point of modelling with relationships ("which decisions did this person sign
/// off on?"). It is not, however, what edges are *stored* under: the store keys
/// every edge on the forward `relationship_type`. Resolution bridges the two.
///
/// A name matching neither direction is an error, not an empty result. An empty
/// result is indistinguishable from "declared, but no edges yet", so a caller
/// who misspells a name — or reaches for one that was never declared — is told
/// the capability is absent rather than that the answer is zero. A *declared*
/// name with no edges still returns an empty list; only an undeclared one errors.
pub async fn resolve_relationship_name(
    node_service: &Arc<NodeService>,
    node_id: &str,
    node_type: &str,
    relationship_name: &str,
) -> Result<ResolvedRelName, OpsError> {
    if BUILTIN_RELATIONSHIP_NAMES.contains(&relationship_name) {
        return Ok(ResolvedRelName::Builtin);
    }

    // A built-in's reverse spelling (`child_of` → `has_child`). Checked before
    // any schema lookup, mirroring the forward short-circuit above: built-ins
    // have no declaration to consult, and a schema may not claim one of these
    // names in either direction (`update_schema` rejects both), so no
    // declaration can be shadowed here.
    if let Some(forward_name) = builtin_forward_name(relationship_name) {
        return Ok(ResolvedRelName::Reverse {
            forward_name: forward_name.to_string(),
            source_type: None,
        });
    }

    // Forward first: a forward name always wins over a same-spelled reverse
    // name on another schema, so resolution can never redirect a traversal that
    // already worked before this resolver existed.
    //
    // Resolved via `resolve_relationships` rather than a direct
    // `get_schema_node(node_type)` lookup: the latter returns only
    // `node_type`'s own directly-declared relationships, not the ADR-078
    // `extends`-chain-merged set. A relationship declared `Forward` only on
    // an ancestor schema and inherited (not redeclared) by a subtype was
    // therefore invisible here even though the write path
    // (`create_relationship` → `resolve_declared_relationship`) is already
    // chain-aware and would happily attach the edge — leaving a subtype
    // instance with a real edge that this resolver couldn't recognize as
    // declared in either direction, falling through to the `InvalidParams`
    // error below and silently returning an empty result at the call site
    // (`GraphResolver::fetch_related_nodes` treats that error as "undeclared,
    // not a failure"). Same fix pattern as `workflow_state.rs`,
    // `validation.rs`, and `graph_resolver.rs`'s `is_declared_many_relationship`.
    let (own_relationships, _owners) = node_service
        .resolve_relationships(node_type)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to resolve relationships: {}", e)))?;
    if own_relationships
        .iter()
        .any(|r| r.name == relationship_name)
    {
        return Ok(ResolvedRelName::Forward);
    }

    // Reverse: a schema targeting this node's type declares `relationship_name`
    // as its `reverse_name`.
    //
    // `get_inbound_relationships` reports an UNTYPED relationship (`target_type:
    // None` — the documented escape hatch for "the target type doesn't exist
    // yet") as inbound for every type in the workspace, since it may legitimately
    // point at anything. Resolution cannot take that at face value: accepting an
    // untyped reverse name from a type its edges never touch resolves to a
    // guaranteed empty result — the silent zero this resolver exists to
    // eliminate, returning through a side door. So an untyped declaration counts
    // only when an edge of that name actually reaches THIS node. A typed
    // declaration still applies unconditionally: its target_type names this type,
    // so "declared but not yet linked" is a real, informative empty answer.
    let all_inbound = node_service
        .get_inbound_relationships(node_type)
        .await
        .map_err(|e| {
            OpsError::Internal(format!("Failed to resolve inbound relationships: {}", e))
        })?;

    // Costs one query per untyped declaration. Untyped relationships are the
    // exception rather than the rule, and every one of them is a candidate both
    // for the match below and for the error's suggestion list, so there is no
    // subset worth skipping.
    let mut inbound = Vec::with_capacity(all_inbound.len());
    for (source_type, rel) in all_inbound {
        if rel.target_type.is_some() {
            inbound.push((source_type, rel));
            continue;
        }
        let reaches_this_node = !node_service
            .get_related_nodes(node_id, &rel.name, "in")
            .await
            .map_err(|e| {
                OpsError::Internal(format!("Failed to probe untyped relationship: {}", e))
            })?
            .is_empty();
        if reaches_this_node {
            inbound.push((source_type, rel));
        }
    }

    for (source_type, rel) in &inbound {
        if rel.reverse_name == relationship_name {
            return Ok(ResolvedRelName::Reverse {
                forward_name: rel.name.clone(),
                source_type: Some(source_type.clone()),
            });
        }
    }

    // The forward name of an inbound relationship: this node is the edge's
    // target, so `--type <forward> --direction in` already traverses it. That
    // spelling worked before this resolver existed — and is the very workaround
    // the reverse-name bug forced callers onto — so it must keep working.
    if inbound.iter().any(|(_, rel)| rel.name == relationship_name) {
        return Ok(ResolvedRelName::InboundForward);
    }

    // Neither. Name what this type CAN be traversed by, so the caller repairs
    // the call from the error alone instead of guessing at a zero. The two
    // spellings are kept as separate groups rather than one mixed list: a bare
    // name and a name carrying a "(with --direction in)" instruction read as
    // different kinds of thing, and interleaving them alphabetically gets harder
    // to scan the more relationships a workspace accumulates.
    let mut direct: Vec<String> = Vec::new();
    let mut needs_direction_in: Vec<String> = Vec::new();
    for rel in &own_relationships {
        if !BUILTIN_RELATIONSHIP_NAMES.contains(&rel.name.as_str()) {
            direct.push(rel.name.clone());
        }
    }
    for (_, rel) in &inbound {
        if BUILTIN_RELATIONSHIP_NAMES.contains(&rel.name.as_str()) {
            continue;
        }
        // The inbound side is ALWAYS reachable by the forward name read
        // inbound. The declared reverse_name is an ADDITIONAL, more natural
        // spelling for the same traversal that needs no --direction flag; it
        // does not replace the forward one. Listing only the reverse name would
        // omit a spelling that demonstrably works, which is the same
        // under-reporting this error exists to prevent.
        direct.push(rel.reverse_name.clone());
        needs_direction_in.push(rel.name.clone());
    }
    for list in [&mut direct, &mut needs_direction_in] {
        list.sort();
        list.dedup();
    }

    let mut parts: Vec<String> = Vec::new();
    if !direct.is_empty() {
        parts.push(format!("available: {}", direct.join(", ")));
    }
    if !needs_direction_in.is_empty() {
        parts.push(format!(
            "available with --direction in: {}",
            needs_direction_in.join(", ")
        ));
    }
    let available = if parts.is_empty() {
        "no typed relationships are declared for this type".to_string()
    } else {
        parts.join("; ")
    };

    Err(OpsError::InvalidParams(format!(
        "Relationship '{}' is not declared for node type '{}' in either direction ({}). \
         Built-in relationships (member_of, has_child, mentions, has_role) are universal.",
        relationship_name, node_type, available
    )))
}

/// Get nodes related via a specific relationship.
///
/// The supplied name is resolved against the node's schema neighbourhood first
/// (see [`resolve_relationship_name`]): a declared `reverse_name` traverses the
/// inverse edge, and an undeclared name errors rather than returning an empty
/// result. The returned `relationship_name`/`direction` describe the traversal
/// that actually ran — a reverse name comes back as the forward name with
/// direction `"in"` — so callers rendering the edge (the CLI's `--name-->`
/// line) show the real direction rather than the one that was asked for.
///
/// `input.direction` is **ignored** when the name resolves to
/// [`ResolvedRelName::Reverse`]. A reverse name — a declared `reverse_name` or
/// a built-in's fixed inverse (`child_of`) — names one specific traversal
/// (the forward relationship, read from the target end); there is no second,
/// meaningful direction it could also mean. An earlier version flipped the
/// stored direction relative to whatever `input.direction` carried, so
/// `--direction in` on top of an already-reverse name flipped it a *second*
/// time and queried the forward relationship's true outbound side — which,
/// for the (usual) case where the declaring and target types differ, cannot
/// ever have edges, so the call silently returned `count: 0`, indistinguishable
/// from "no edges exist". That outbound query is not lost: it is exactly what
/// the literal forward name already expresses (`--type billed_to` with no
/// flag, or `--direction out`), so resolving both spellings of `--direction`
/// to the one real traversal loses no capability and removes the silent-zero
/// trap.
pub async fn get_related_nodes(
    node_service: &Arc<NodeService>,
    input: GetRelatedInput,
) -> Result<GetRelatedOutput, OpsError> {
    // Validated up front, uniformly, rather than left to whichever branch
    // below happens to still forward `input.direction` downstream. Before
    // this fix, a Reverse resolution was the one path that both dropped the
    // caller's direction AND never re-validated it (the old flip collapsed
    // any non-"in" value, garbage included, into a valid "out"/"in" pair) —
    // a malformed value would silently succeed instead of erroring the way
    // the same typo does today on a Forward/Builtin/InboundForward name
    // (caught downstream in `NodeService::get_related_nodes`). Checking here
    // closes that gap for every branch, not just Reverse, and fails before
    // any DB work rather than after resolving the name.
    if input.direction != "out" && input.direction != "in" {
        return Err(OpsError::InvalidParams(format!(
            "direction must be 'in' or 'out', got '{}'",
            input.direction
        )));
    }

    let node = node_service
        .get_node(&input.node_id)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to load node: {}", e)))?
        .ok_or_else(|| OpsError::NotFound {
            id: input.node_id.clone(),
        })?;

    let resolved = resolve_relationship_name(
        node_service,
        &input.node_id,
        &node.node_type,
        &input.relationship_name,
    )
    .await?;

    // A reverse name addresses the same edges from the other end: rewrite to
    // the stored forward name, always queried inbound (see the doc comment
    // above — `input.direction` does not apply to this case).
    let (relationship_name, direction, source_type) = match &resolved {
        ResolvedRelName::Builtin | ResolvedRelName::Forward | ResolvedRelName::InboundForward => (
            input.relationship_name.clone(),
            input.direction.clone(),
            None,
        ),
        ResolvedRelName::Reverse {
            forward_name,
            source_type,
        } => (forward_name.clone(), "in".to_string(), source_type.clone()),
    };

    let nodes = node_service
        .get_related_nodes(&input.node_id, &relationship_name, &direction)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to get related nodes: {}", e)))?;

    // The store keys the "in" query on relationship_type alone, so two schemas
    // declaring the same forward name toward this type would both answer here.
    // A reverse name was declared by exactly one of them — restrict to it, the
    // same narrowing `get_node_relationships` applies to its inbound groups.
    //
    // `InboundForward` is deliberately NOT narrowed, though it could over-report
    // the same way. A caller naming the forward name asked for it generically,
    // not through one declarer's private reverse spelling, so every schema
    // declaring it is a legitimate answer — and narrowing would change what that
    // spelling returned before this resolver existed.
    let nodes: Vec<_> = match &source_type {
        Some(source_type) => nodes
            .into_iter()
            .filter(|n| n.node_type == *source_type)
            .collect(),
        None => nodes,
    };

    let count = nodes.len();
    // Emit the same flat, API-facing shape every other read path produces.
    // A plain `to_value(Node)` here would serialize properties exactly as
    // stored — namespaced under the schema id — leaking a storage detail that
    // must not cross the API boundary, and diverging from `node get`/`query`.
    //
    // Note this does more than de-nest: it also promotes type-specific fields
    // to the top level for `task`/`ai-chat`, reshapes `schema` nodes via
    // `SchemaNode::from_node`, and injects a `nodespace://` URI. That is the
    // frontend's wire contract, so do not "simplify" this back to a plain
    // `to_value` — the shapes are not equivalent. The CLI re-keys this into
    // its own snake_case node shape (`output::related_node_to_json`).
    let related_nodes: Vec<Value> =
        crate::models::nodes_to_typed_values(nodes).map_err(OpsError::Internal)?;

    Ok(GetRelatedOutput {
        node_id: input.node_id,
        relationship_name,
        direction,
        related_nodes,
        count,
    })
}

// ============================================================================
// Relationship viewer aggregation (read-only LIST)
// ============================================================================

/// A single related node on one end of a typed relationship, carrying enough
/// identity to be recognizable plus the connecting edge's stored properties.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedNodeView {
    pub id: String,
    pub node_type: String,
    /// Current computed title (read at query time, never snapshotted).
    pub title: Option<String>,
    /// Truncated content, for types without a title template.
    pub content_preview: String,
    /// The connecting edge's stored `properties` JSON (edge attributes).
    pub edge_properties: Value,
}

/// One relationship group: a single (relationship name, direction) pairing, with
/// all of its related nodes. Outbound and inbound are kept as SEPARATE groups —
/// a self-referential relationship legitimately appears twice (once per side).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipGroup {
    /// Underlying schema relationship name (the edge's `relationship_type`).
    pub relationship_name: String,
    /// "out" = declared on this node's own schema; "in" = another schema declares
    /// this node's type as its `target_type`.
    pub direction: String,
    /// The node type on the FAR end of the edge (outbound: the declared
    /// `target_type`; inbound: the declaring `source_type`).
    pub target_type: Option<String>,
    /// Reverse label supplied by the source schema. Always present — every
    /// declaration names the edge from both ends — so the inbound side displays
    /// it directly rather than deriving a label.
    pub reverse_name: String,
    /// The schema type that DECLARES this relationship.
    pub source_type: String,
    /// Effective cardinality for THIS side: the forward `cardinality` outbound,
    /// the `reverse_cardinality` inbound. Known on both sides, since a
    /// declaration must supply both.
    pub cardinality: RelationshipCardinality,
    /// Whether the forward relationship is required (outbound only). The viewer
    /// uses this to confirm-on-delete; last-edge removal is enforced server-side
    /// in `NodeService::delete_relationship`.
    pub required: Option<bool>,
    /// Declared edge field definitions, if any (drives the edge-attribute columns).
    pub edge_fields: Option<Vec<EdgeField>>,
    pub description: Option<String>,
    pub related: Vec<RelatedNodeView>,
    pub count: usize,
}

/// A node's typed relationships, grouped by (name, direction). Excludes the
/// built-in structural relationships. Declared outbound groups are always
/// included (even with no edges yet, so the viewer can offer to add the first
/// one); inbound groups appear only when they have at least one edge.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRelationshipsOutput {
    pub node_id: String,
    pub node_type: String,
    pub groups: Vec<RelationshipGroup>,
}

/// Truncate content to a compact preview (character-safe).
fn content_preview(content: &str) -> String {
    const MAX: usize = 140;
    let trimmed = content.trim();
    if trimmed.chars().count() <= MAX {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(MAX).collect();
        format!("{}…", truncated)
    }
}

/// Fetch the related nodes for one (relationship, direction) pairing, each with
/// its connecting edge's properties, mapped into `RelatedNodeView`s.
async fn collect_related(
    node_service: &Arc<NodeService>,
    node_id: &str,
    relationship_name: &str,
    direction: &str,
) -> Result<Vec<RelatedNodeView>, OpsError> {
    let pairs = node_service
        .get_related_nodes_with_edges(node_id, relationship_name, direction)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to get related nodes: {}", e)))?;

    Ok(pairs
        .into_iter()
        .map(|(node, edge_properties)| RelatedNodeView {
            content_preview: content_preview(&node.content),
            title: node.title.clone(),
            id: node.id,
            node_type: node.node_type,
            edge_properties,
        })
        .collect())
}

/// List a node's schema-declared typed relationships, grouped by name and
/// covering BOTH directions (read-only — edge creation/edit/deletion live on
/// `NodeService::create_relationship`/`update_relationship_properties`/`delete_relationship`).
///
/// - **Outbound** groups come from the node's own schema's `relationships`
///   (queried with direction `"out"`; the node is the edge source).
/// - **Inbound** groups come from [`NodeService::get_inbound_relationships`],
///   which resolves every schema whose relationship targets this node's type
///   (including untyped `target_type: None` relationships that apply to any
///   type), queried with direction `"in"` (the node is the edge target). The
///   inbound side is governed by `reverse_cardinality` and labeled by
///   `reverse_name`.
///
/// Each related node carries its connecting edge's `properties` so the viewer can
/// render edge attributes. Built-in structural relationships (`has_child`,
/// `mentions`, `member_of`, `has_role`) are excluded. Declared groups are always
/// returned on both sides — outbound and inbound — even when empty, so the viewer
/// can add the first edge and callers can distinguish a declared-but-unlinked
/// relationship from none at all.
pub async fn get_node_relationships(
    node_service: &Arc<NodeService>,
    node_id: &str,
) -> Result<NodeRelationshipsOutput, OpsError> {
    let node = node_service
        .get_node(node_id)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to load node: {}", e)))?
        .ok_or_else(|| OpsError::NotFound {
            id: node_id.to_string(),
        })?;
    let node_type = node.node_type.clone();

    let mut groups: Vec<RelationshipGroup> = Vec::new();

    // ---- Outbound: relationships declared on this node's own schema ----
    // Schema nodes have id == node_type. A node whose type has no schema (e.g.
    // plain text) simply has no outbound typed relationships. Declarations
    // arrive hydrated from the relationship table — the same consolidated read
    // path `NodeService::create_relationship` validates against, so the viewer
    // and the write path can never see different declaration sets.
    if let Some(schema_node) = node_service
        .get_schema_node(&node_type)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to load schema node: {}", e)))?
    {
        for rel in schema_node.relationships {
            if BUILTIN_RELATIONSHIP_NAMES.contains(&rel.name.as_str()) {
                continue;
            }
            // Emit the group even when it has no edges yet: an empty declared
            // outbound relationship still needs to render so the viewer can add
            // its first edge. The inbound branch below is symmetric.
            let related = collect_related(node_service, node_id, &rel.name, "out").await?;
            let count = related.len();
            groups.push(RelationshipGroup {
                relationship_name: rel.name.clone(),
                direction: "out".to_string(),
                target_type: rel.target_type.clone(),
                reverse_name: rel.reverse_name.clone(),
                source_type: node_type.clone(),
                cardinality: rel.cardinality.clone(),
                required: rel.required,
                edge_fields: rel.edge_fields.clone(),
                description: rel.description.clone(),
                related,
                count,
            });
        }
    }

    // ---- Inbound: other schemas whose relationship targets this node's type ----
    let inbound = node_service
        .get_inbound_relationships(&node_type)
        .await
        .map_err(|e| {
            OpsError::Internal(format!("Failed to resolve inbound relationships: {}", e))
        })?;

    // Whether a given node type satisfies a given declarer, cached across every
    // inbound group. `resolve_type_chain` is resolved live and issues a full
    // parent-map query per call, so without this the panel costs one round trip
    // per inbound edge per group; the previous exact-match `retain` cost none.
    //
    // Keyed on BOTH halves, and the `source_type` half is load-bearing — do not
    // "simplify" it away. Two schemas may declare the same forward name toward
    // the same target (`tasks` is declared on both `project` and `person`,
    // which `graph_resolver` documents as live rather than hypothetical), so
    // one node type can legitimately satisfy one declarer and not another.
    // `collect_related` scopes candidates by relationship NAME, not by
    // declarer, so both groups see both declarers' nodes and a node-type-only
    // key answers the second group with the first group's verdict — a wrong
    // node under the wrong group, which is the exact bug this narrowing
    // exists to prevent.
    let mut satisfies: HashMap<(String, String), bool> = HashMap::new();

    for (source_type, rel) in inbound {
        if BUILTIN_RELATIONSHIP_NAMES.contains(&rel.name.as_str()) {
            continue;
        }
        let related = collect_related(node_service, node_id, &rel.name, "in").await?;
        // The "in" query keys only on relationship_type; two schemas can declare
        // the same relationship name targeting this type, so restrict this group
        // to nodes of the declaring source type — otherwise e.g. `task` and `bug`
        // both declaring `assigned_to → person` would each surface the other's
        // edges under the wrong group and double the count.
        //
        // Matched against each node's ancestor chain rather than by equality:
        // `task.blocks` declares its reverse toward `task`, so an `issue` on
        // the far end IS a valid member of that group (ADR-078). An exact
        // comparison silently dropped every subtype instance, which rendered
        // as a confident "0" rather than as a visibly missing group.
        //
        // Resolved per node rather than once for the declarer, because the
        // question is "is THIS node a `source_type`" — the answer depends on
        // the node's own chain, not the declarer's descendants.
        let mut kept = Vec::with_capacity(related.len());
        for r in related {
            let key = (r.node_type.clone(), source_type.clone());
            let answer = match satisfies.get(&key) {
                Some(known) => *known,
                None => {
                    let chain = node_service
                        .resolve_type_chain(&r.node_type)
                        .await
                        .map_err(|e| {
                            OpsError::Internal(format!("Failed to resolve type chain: {e}"))
                        })?;
                    // Inserted only after a successful resolve, so an error
                    // short-circuits without caching a wrong default.
                    let answer = chain.contains(&source_type);
                    satisfies.insert(key, answer);
                    answer
                }
            };
            if answer {
                kept.push(r);
            }
        }
        let related = kept;
        // Emit the group even with no edges yet — symmetric with the outbound
        // branch above — so a type reached only through a derived inbound
        // relationship (e.g. `task`, whose `project` link is declared outbound on
        // `project`) still surfaces the relationship, and the viewer can add the
        // first inbound edge from this side.
        let count = related.len();
        groups.push(RelationshipGroup {
            relationship_name: rel.name.clone(),
            direction: "in".to_string(),
            target_type: Some(source_type.clone()),
            reverse_name: rel.reverse_name.clone(),
            source_type,
            // Reading from the inbound side, the reverse cardinality governs.
            cardinality: rel.reverse_cardinality.clone(),
            required: None,
            edge_fields: rel.edge_fields.clone(),
            description: rel.description.clone(),
            related,
            count,
        });
    }

    Ok(NodeRelationshipsOutput {
        node_id: node_id.to_string(),
        node_type,
        groups,
    })
}
