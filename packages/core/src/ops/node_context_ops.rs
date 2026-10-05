//! Reading a node with what its relationship paths reach, and the skills
//! attached along the way (ADR-094 §2 and §3).
//!
//! The read knows no type. It follows the paths it is given from the node it
//! is given, whatever that node is, and returns the skills linked to any node
//! it returned through the skill schema's `attached_to` relationship.

use crate::governance;
use crate::models::{CoreNodeType, Node, SKILL_ATTACHED_TO};
use crate::ops::path_ops::{resolve_hop, undeclared_message, HopResolution};
use crate::ops::skill_ops::{self, GuidanceSchema, GuidanceSkill};
use crate::ops::OpsError;
use crate::services::NodeService;
use nodespace_types::{RelationshipHop, RelationshipPath, ResolvedHop, ResolvedPath};
use std::collections::HashSet;

/// The most paths one read follows.
pub const MAX_CONTEXT_PATHS: usize = 20;

/// The most nodes one path returns. A path that reaches more is cut to the
/// first of them and says so ([`PathNodes::limit_reached`]): each returned
/// node costs a read of its children, and a caller reading context wants the
/// few nodes that govern one, not a listing.
pub const MAX_NODES_PER_PATH: usize = 50;

/// A node to read, and the paths to follow from it.
#[derive(Debug)]
pub struct NodeContextInput {
    pub node_id: String,
    pub paths: Vec<RelationshipPath>,
}

/// A node as a context read returns it: its fields, and its direct checkbox
/// children in sibling order. Never its whole subtree.
#[derive(Debug, Clone)]
pub struct ContextNode {
    pub node: Node,
    pub checkboxes: Vec<Node>,
}

/// The nodes one path reaches from the node read.
#[derive(Debug, Clone)]
pub struct PathNodes {
    pub path: RelationshipPath,
    /// Each node once, in the order the walk first reached it, up to
    /// [`MAX_NODES_PER_PATH`].
    pub nodes: Vec<ContextNode>,
    /// Whether the path reaches more nodes than `nodes` holds.
    pub limit_reached: bool,
}

/// A skill attached to one or more of the nodes a read returned.
#[derive(Debug, Clone)]
pub struct AttachedSkill {
    /// The skill as a fetch by name returns it.
    pub skill: GuidanceSkill,
    /// The ids of the returned nodes it is attached to, in the order the
    /// read returned them.
    pub attached_to: Vec<String>,
}

/// The skills attached to a set of nodes, each once, and the schemas those
/// skills are linked to through `applies_to`.
#[derive(Debug, Clone, Default)]
pub struct AttachedSkills {
    pub skills: Vec<AttachedSkill>,
    pub schemas: Vec<GuidanceSchema>,
}

/// What a context read returns.
#[derive(Debug, Clone)]
pub struct NodeContext {
    pub node: ContextNode,
    /// One entry per path asked for, in the order asked.
    pub paths: Vec<PathNodes>,
    /// The skills attached to the node and to every node a path reached.
    pub attached: AttachedSkills,
}

/// Read `input.node_id` with the nodes each of `input.paths` reaches and the
/// skills attached to any of them.
///
/// A path's names are resolved from the node the walk stands on: the first
/// against the type of the node read, each later one against the types of the
/// nodes the hop before it reached. A name is refused, with an error naming
/// the path, when none of the types it is read from declares it. Where those
/// nodes are of several types, the ones whose type does not declare the name
/// lead nowhere and the rest are followed. Where the hop before reached
/// nothing, the name is checked against the type that hop declares; if it
/// declares no one type (a built-in, a relationship to any node, or a name
/// that means different relationships from different types), there is
/// nothing to check the name against and the path reaches nothing.
///
/// Archived nodes are neither reached nor walked through, and an archived
/// skill is not returned. The node asked for is read by its id, as any read
/// by id is, so an archived one is returned with the skills attached to it.
pub async fn read_node_context(
    node_service: &NodeService,
    input: NodeContextInput,
) -> Result<NodeContext, OpsError> {
    if input.paths.len() > MAX_CONTEXT_PATHS {
        return Err(OpsError::InvalidParams(format!(
            "{} paths were given; one read follows at most {MAX_CONTEXT_PATHS}",
            input.paths.len()
        )));
    }
    let root = node_service
        .get_node(&input.node_id)
        .await?
        .ok_or_else(|| OpsError::NotFound {
            id: input.node_id.clone(),
        })?;

    let mut returned_ids = vec![root.id.clone()];
    let mut paths = Vec::with_capacity(input.paths.len());
    for path in input.paths {
        let mut reached = walk(node_service, &root, &path).await?;
        let limit_reached = reached.len() > MAX_NODES_PER_PATH;
        reached.truncate(MAX_NODES_PER_PATH);
        let mut nodes = Vec::with_capacity(reached.len());
        for node in reached {
            if !returned_ids.contains(&node.id) {
                returned_ids.push(node.id.clone());
            }
            nodes.push(context_node(node_service, node).await?);
        }
        paths.push(PathNodes {
            path,
            nodes,
            limit_reached,
        });
    }

    Ok(NodeContext {
        node: context_node(node_service, root).await?,
        paths,
        attached: attached_skills(node_service, &returned_ids).await?,
    })
}

/// The skills attached to any of `node_ids` through `attached_to`, each once,
/// in the order first met, with the nodes of `node_ids` it is attached to.
///
/// An archived skill is not returned, and neither is the source of an
/// `attached_to` edge that is not a skill.
pub async fn attached_skills(
    node_service: &NodeService,
    node_ids: &[String],
) -> Result<AttachedSkills, OpsError> {
    let by_target = node_service
        .store()
        .get_edge_sources_by_target(node_ids, SKILL_ATTACHED_TO)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read attached skills: {e}")))?;

    // Skill ids in the order first met, each with the nodes it is attached to.
    let mut order: Vec<(String, Vec<String>)> = Vec::new();
    for node_id in node_ids {
        for skill_id in by_target.get(node_id).into_iter().flatten() {
            match order.iter_mut().find(|(known, _)| known == skill_id) {
                Some((_, attached_to)) => {
                    if !attached_to.contains(node_id) {
                        attached_to.push(node_id.clone());
                    }
                }
                None => order.push((skill_id.clone(), vec![node_id.clone()])),
            }
        }
    }
    if order.is_empty() {
        return Ok(AttachedSkills::default());
    }

    let skill_ids: Vec<String> = order.iter().map(|(id, _)| id.clone()).collect();
    let mut loaded = node_service
        .store()
        .get_nodes_by_ids(&skill_ids)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read attached skills: {e}")))?;

    let mut skill_nodes = Vec::with_capacity(order.len());
    for (skill_id, _) in &order {
        let Some(node) = loaded.remove(skill_id) else {
            continue;
        };
        if governance::participates(&node)
            && node_service
                .type_is_a(&node.node_type, CoreNodeType::Skill)
                .await?
        {
            skill_nodes.push(node);
        }
    }

    let guidance = skill_ops::fetch_skills(node_service, &skill_nodes).await?;
    let skills = guidance
        .skills
        .into_iter()
        .map(|skill| {
            let attached_to = order
                .iter()
                .find(|(id, _)| *id == skill.id)
                .map(|(_, attached_to)| attached_to.clone())
                .unwrap_or_default();
            AttachedSkill { skill, attached_to }
        })
        .collect();
    Ok(AttachedSkills {
        skills,
        schemas: guidance.schemas,
    })
}

/// `node` with its direct checkbox children.
async fn context_node(node_service: &NodeService, node: Node) -> Result<ContextNode, OpsError> {
    let mut checkboxes = Vec::new();
    for child in node_service.get_children(&node.id).await? {
        if governance::participates(&child)
            && node_service
                .type_is_a(&child.node_type, CoreNodeType::Checkbox)
                .await?
        {
            checkboxes.push(child);
        }
    }
    Ok(ContextNode { node, checkboxes })
}

/// The nodes `path` reaches from `root`, each once, in the order first
/// reached.
///
/// Walked a hop at a time, so each name is resolved against the nodes the
/// walk is standing on: a hop that follows a relationship with no declared
/// target type is resolved once the nodes it reached are known. Each hop is
/// one statement per type of node it leaves from.
async fn walk(
    node_service: &NodeService,
    root: &Node,
    path: &RelationshipPath,
) -> Result<Vec<Node>, OpsError> {
    if path.is_empty() {
        return Err(OpsError::InvalidParams(
            "a path needs at least one relationship name".to_string(),
        ));
    }

    let mut frontier = vec![root.clone()];
    // The type the schemas say the walk stands on, where they say one. It
    // checks a name when the hop before it reached nothing.
    let mut declared_type = Some(root.node_type.clone());

    for hop in path.hops() {
        let mut types: Vec<String> = Vec::new();
        for node in &frontier {
            if !types.contains(&node.node_type) {
                types.push(node.node_type.clone());
            }
        }
        if types.is_empty() {
            match &declared_type {
                Some(declared) => types.push(declared.clone()),
                // Nothing to stand on and no declared type to read the
                // name from: there is nothing to check it against, and
                // nothing it could reach.
                None => return Ok(Vec::new()),
            }
        }

        let mut resolved: Vec<(String, ResolvedHop)> = Vec::new();
        let mut undeclared_for = None;
        for node_type in &types {
            match resolve_from(node_service, node_type, hop, path).await? {
                Some(hop) => resolved.push((node_type.clone(), hop)),
                None => undeclared_for = Some(node_type.clone()),
            }
        }
        if resolved.is_empty() {
            // `types` is not empty, so some type refused the name.
            let node_type = undeclared_for.unwrap_or_default();
            let message = undeclared_message(node_service, &node_type, &hop.name).await?;
            return Err(OpsError::InvalidParams(format!("path '{path}': {message}")));
        }

        let far_types: HashSet<Option<&str>> = resolved
            .iter()
            .map(|(_, hop)| hop.far_type.as_deref())
            .collect();
        declared_type = match far_types.into_iter().collect::<Vec<_>>().as_slice() {
            [Some(only)] => Some(only.to_string()),
            _ => None,
        };

        let mut next: Vec<Node> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (node_type, hop) in resolved {
            let start_ids: Vec<String> = frontier
                .iter()
                .filter(|node| node.node_type == node_type)
                .map(|node| node.id.clone())
                .collect();
            if start_ids.is_empty() {
                continue;
            }
            let step = ResolvedPath { hops: vec![hop] };
            let reach = node_service
                .store()
                .resolve_relationship_path(&start_ids, &step, false)
                .await
                .map_err(|e| OpsError::Internal(format!("Failed to walk path '{path}': {e}")))?;
            for start_id in &start_ids {
                for node in reach.at(0, start_id) {
                    if seen.insert(node.id.clone()) {
                        next.push(node.clone());
                    }
                }
            }
        }
        frontier = next;
    }
    Ok(frontier)
}

/// What `hop` resolves to from `node_type`: `None` when the type declares no
/// such relationship in either direction.
async fn resolve_from(
    node_service: &NodeService,
    node_type: &str,
    hop: &RelationshipHop,
    path: &RelationshipPath,
) -> Result<Option<ResolvedHop>, OpsError> {
    match resolve_hop(node_service, Some(node_type), hop)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to resolve path '{path}': {e}")))?
    {
        HopResolution::Resolved(resolved) => Ok(Some(resolved)),
        HopResolution::Undeclared | HopResolution::TypeUnknown => Ok(None),
    }
}
