//! Reading a node with what governs it (ADR-094 §2, §4 and §7): the nodes
//! its relationship paths reach, the skills that apply to it, and a version
//! of the whole read.
//!
//! The read knows no type. It follows the context paths the node's type
//! declares and the paths it is given, whatever that node is. The skills it
//! returns are derived from the node: those attached, through the skill
//! schema's `attached_to` relationship, to the node, to a saved query the
//! node currently matches, or to a node a path reached.

use crate::governance;
use crate::models::{CoreNodeType, Node, QueryFields, SKILL_ATTACHED_TO};
use crate::ops::path_ops::{resolve_hop, undeclared_message, HopResolution};
use crate::ops::query_ops::checked_definition;
use crate::ops::skill_ops::{self, GuidanceSchema, GuidanceSkill, SkillGuidance};
use crate::ops::OpsError;
use crate::services::{NodeService, QueryDefinition};
use nodespace_types::{RelationshipHop, RelationshipPath, ResolvedHop, ResolvedPath};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// The most paths one read is given, and the most context paths one schema
/// declares.
pub const MAX_CONTEXT_PATHS: usize = 20;

/// The most nodes one path returns. A path that reaches more is cut to the
/// first of them and says so ([`PathNodes::limit_reached`]): each returned
/// node costs a read of its children, and a caller reading context wants the
/// few nodes that govern one, not a listing.
pub const MAX_NODES_PER_PATH: usize = 50;

/// The most items one saved query run returns with their context. Each item
/// is a context read of its own.
pub const MAX_CONTEXT_ITEMS: usize = 50;

/// A node to read, and the paths to follow from it besides the context paths
/// its type declares.
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

/// A saved query the node read currently matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedQuery {
    pub id: String,
    pub title: String,
}

/// A skill that applies to the node read, with what it was reached through.
#[derive(Debug, Clone)]
pub struct AttachedSkill {
    /// The skill as a fetch by name returns it.
    pub skill: GuidanceSkill,
    /// The ids of the returned nodes it is attached to, in the order the
    /// read returned them.
    pub attached_to: Vec<String>,
    /// The saved queries it is attached to that the node read currently
    /// matches.
    pub matched_queries: Vec<MatchedQuery>,
}

/// A set of skills, each once, and the schemas those skills are linked to
/// through `applies_to`.
#[derive(Debug, Clone, Default)]
pub struct AttachedSkills {
    pub skills: Vec<AttachedSkill>,
    pub schemas: Vec<GuidanceSchema>,
}

/// What a context read returns.
#[derive(Debug, Clone)]
pub struct NodeContext {
    pub node: ContextNode,
    /// One entry per path followed: the context paths the node's type
    /// declares, then the paths asked for that are not among them.
    pub paths: Vec<PathNodes>,
    /// The node's applicable skills: those attached to the node, to a saved
    /// query it currently matches, and to a node a path reached.
    pub attached: AttachedSkills,
    /// Changes when the node changes, when a node the read returned changes,
    /// when the content of an applicable skill changes, and when the set of
    /// nodes or skills returned changes. Two reads with no such change
    /// between them carry the same version. Derived from what is stored.
    pub version: String,
}

/// The items of a saved query run, each with its context.
#[derive(Debug, Clone)]
pub struct ContextItems {
    /// One context read per item, in the order the run returned them. Each
    /// item's `attached` holds that item's applicable skills.
    pub items: Vec<NodeContext>,
    /// Every skill an item carries, and every skill attached to the query
    /// that ran, each once. `attached_to` holds the query that ran where the
    /// skill is attached to it; what a skill reaches each item through is on
    /// the item.
    pub attached: AttachedSkills,
}

/// The saved queries that have a skill attached, each ready to be asked
/// whether it matches a node.
///
/// Membership is asked per node and never stored: a node carries a queue's
/// skills for as long as it matches the queue, whoever reads it. Only these
/// queries are asked, so the cost of a read grows with the number of queues
/// that hand a skill over, not with the number of saved queries.
pub struct SkillQueries {
    queries: Vec<(MatchedQuery, QueryDefinition)>,
}

impl SkillQueries {
    /// Read every saved query a skill is attached to.
    ///
    /// A query is asked as it is stored: its type and filters, a relative
    /// date read on the day of the read, no run-time narrowing. Its sorting
    /// and limit decide what a run shows, not what the query matches, so
    /// they play no part. An archived query hands nothing over. A stored
    /// query that cannot be run (a filter naming a relationship since
    /// removed) is left out and logged: it matches nothing until it is
    /// repaired, and must not fail the read of every node.
    pub async fn load(node_service: &NodeService) -> Result<Self, OpsError> {
        let failed = |e| OpsError::Internal(format!("Failed to read the queries with skills: {e}"));
        let target_ids = node_service
            .store()
            .get_edge_target_ids(SKILL_ATTACHED_TO)
            .await
            .map_err(failed)?;
        let mut targets = node_service
            .store()
            .get_nodes_by_ids(&target_ids)
            .await
            .map_err(failed)?;

        let mut queries = Vec::new();
        for id in &target_ids {
            let Some(node) = targets.remove(id) else {
                continue;
            };
            if !governance::participates(&node)
                || !node_service
                    .type_is_a(&node.node_type, CoreNodeType::Query)
                    .await?
            {
                continue;
            }
            let definition = match QueryFields::from_properties(&node.properties) {
                Ok(fields) => {
                    checked_definition(node_service, fields.target_type, fields.filters, None, None)
                        .await
                }
                Err(e) => Err(OpsError::InvalidParams(e.to_string())),
            };
            match definition {
                Ok(definition) => queries.push((
                    MatchedQuery {
                        id: node.id,
                        title: node.content,
                    },
                    definition,
                )),
                Err(OpsError::InvalidParams(reason)) => tracing::warn!(
                    query_id = %node.id,
                    %reason,
                    "A saved query with skills attached cannot be run; no node is read as \
                     matching it"
                ),
                Err(e) => return Err(e),
            }
        }
        Ok(Self { queries })
    }

    /// The ids of the queries membership is asked of.
    pub fn query_ids(&self) -> Vec<&str> {
        self.queries
            .iter()
            .map(|(query, _)| query.id.as_str())
            .collect()
    }

    /// The queries `node` currently matches. An archived node matches none.
    pub async fn matching(
        &self,
        node_service: &NodeService,
        node: &Node,
    ) -> Result<Vec<MatchedQuery>, OpsError> {
        let mut matched = Vec::new();
        if self.queries.is_empty() || !governance::participates(node) {
            return Ok(matched);
        }
        for (query, definition) in &self.queries {
            let is_member =
                crate::ops::query_ops::definition_matches(node_service, definition, node)
                    .await
                    .map_err(|e| {
                        OpsError::Internal(format!(
                            "Failed to ask whether '{}' matches the saved query '{}': {e}",
                            node.id, query.id
                        ))
                    })?;
            if is_member {
                matched.push(query.clone());
            }
        }
        Ok(matched)
    }
}

/// Read `input.node_id` with what governs it: the nodes its type's context
/// paths and `input.paths` reach, its applicable skills, and the version of
/// the read.
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
/// by id is, so an archived one is returned with the skills attached to it;
/// it matches no saved query.
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
    let queries = SkillQueries::load(node_service).await?;
    let mut cache = ReadCache::default();
    context_of(node_service, root, input.paths, &queries, &mut cache).await
}

/// Read each of `nodes`, the items a saved query run returned, with its
/// context. `query_id` is the query that ran: the skills attached to it are
/// returned whether or not an item is.
pub async fn read_node_contexts(
    node_service: &NodeService,
    nodes: Vec<Node>,
    query_id: &str,
) -> Result<ContextItems, OpsError> {
    let queries = SkillQueries::load(node_service).await?;
    let mut cache = ReadCache::default();
    let mut attached =
        applicable_skills(node_service, &[query_id.to_string()], &[], &mut cache).await?;
    let mut items = Vec::with_capacity(nodes.len());
    for node in nodes {
        let item = context_of(node_service, node, Vec::new(), &queries, &mut cache).await?;
        for skill in &item.attached.skills {
            if !attached
                .skills
                .iter()
                .any(|known| known.skill.id == skill.skill.id)
            {
                attached.skills.push(AttachedSkill {
                    skill: skill.skill.clone(),
                    attached_to: Vec::new(),
                    matched_queries: Vec::new(),
                });
            }
        }
        for schema in &item.attached.schemas {
            if !attached.schemas.iter().any(|known| known.id == schema.id) {
                attached.schemas.push(schema.clone());
            }
        }
        items.push(item);
    }
    Ok(ContextItems { items, attached })
}

/// The context of `root`: its type's context paths and then `asked`,
/// followed; its applicable skills; the version.
async fn context_of(
    node_service: &NodeService,
    root: Node,
    asked: Vec<RelationshipPath>,
    queries: &SkillQueries,
    cache: &mut ReadCache,
) -> Result<NodeContext, OpsError> {
    let declared = cache.context_paths(node_service, &root.node_type).await?;

    let mut returned_ids = vec![root.id.clone()];
    let mut paths: Vec<PathNodes> = Vec::with_capacity(declared.len() + asked.len());
    let follow = declared
        .into_iter()
        .map(|(path, schema)| (path, Some(schema)))
        .chain(asked.into_iter().map(|path| (path, None)));
    for (path, declared_by) in follow {
        if paths.iter().any(|followed| followed.path == path) {
            continue;
        }
        let mut reached = match (walk(node_service, &root, &path).await, declared_by) {
            (Ok(reached), _) => reached,
            // The path was valid when the schema saved it, and a relationship
            // it names has since gone. Said with the schema to repair, since
            // the caller of a read did not write the path.
            (Err(OpsError::InvalidParams(reason)), Some(schema)) => {
                return Err(OpsError::InvalidParams(format!(
                    "the context path '{path}' that schema '{schema}' declares no longer \
                     resolves ({reason}). Remove it with update_schema's remove_context_paths, \
                     or declare the relationship again."
                )));
            }
            (Err(e), _) => return Err(e),
        };
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

    let matched = queries.matching(node_service, &root).await?;
    let attached = applicable_skills(node_service, &returned_ids, &matched, cache).await?;
    let node = context_node(node_service, root).await?;
    let version = context_version(&node, &paths, &attached);
    Ok(NodeContext {
        node,
        paths,
        attached,
        version,
    })
}

/// The version of a read: a digest of the id and version of every node it
/// returned, of what each path reached, and of every skill and schema it
/// returned with what the skill was reached through.
///
/// A skill is digested by what it hands over, not by its node's version: its
/// procedure is its child subtree, and an edit there leaves the skill node's
/// own version as it was.
fn context_version(node: &ContextNode, paths: &[PathNodes], attached: &AttachedSkills) -> String {
    let mut hasher = Sha256::new();
    // Each part is length-prefixed, so no two different reads share a byte
    // stream.
    let mut part = |text: &str| {
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    };
    let node_parts = |part: &mut dyn FnMut(&str), node: &ContextNode| {
        part(&node.node.id);
        part(&node.node.version.to_string());
        part(&node.checkboxes.len().to_string());
        for checkbox in &node.checkboxes {
            part(&checkbox.id);
            part(&checkbox.version.to_string());
        }
    };

    node_parts(&mut part, node);
    part(&paths.len().to_string());
    for reached in paths {
        part(&reached.path.to_string());
        part(if reached.limit_reached { "more" } else { "all" });
        part(&reached.nodes.len().to_string());
        for node in &reached.nodes {
            node_parts(&mut part, node);
        }
    }
    part(&attached.skills.len().to_string());
    for attached in &attached.skills {
        let skill = &attached.skill;
        part(&skill.id);
        part(&skill.name);
        part(&skill.use_for);
        part(&skill.instructions);
        part(&skill.tool_commands.len().to_string());
        for command in &skill.tool_commands {
            part(&command.tool);
            part(&command.command);
        }
        part(&attached.attached_to.len().to_string());
        for node_id in &attached.attached_to {
            part(node_id);
        }
        part(&attached.matched_queries.len().to_string());
        for query in &attached.matched_queries {
            part(&query.id);
            part(&query.title);
        }
    }
    part(&attached.schemas.len().to_string());
    for schema in &attached.schemas {
        part(&schema.id);
        part(&schema.name);
        part(&schema.definition.to_string());
    }
    let digest = format!("{:x}", hasher.finalize());
    digest[..16].to_string()
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
    applicable_skills(node_service, node_ids, &[], &mut ReadCache::default()).await
}

/// The skills attached to any of `returned_ids`, the nodes a read returned,
/// or to any of `matched`, the saved queries the node read matches. Each
/// skill once, with every node and query it was reached through.
///
/// The node read is first among `returned_ids`. Its own skills come first,
/// then those of the queries it matches, then those of the nodes its paths
/// reached.
async fn applicable_skills(
    node_service: &NodeService,
    returned_ids: &[String],
    matched: &[MatchedQuery],
    cache: &mut ReadCache,
) -> Result<AttachedSkills, OpsError> {
    let mut target_ids: Vec<String> = returned_ids.to_vec();
    for query in matched {
        if !target_ids.contains(&query.id) {
            target_ids.push(query.id.clone());
        }
    }
    let by_target = node_service
        .store()
        .get_edge_sources_by_target(&target_ids, SKILL_ATTACHED_TO)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read attached skills: {e}")))?;

    struct Reached {
        skill_id: String,
        attached_to: Vec<String>,
        matched_queries: Vec<MatchedQuery>,
    }
    let mut order: Vec<Reached> = Vec::new();
    let mut reach = |skill_id: &String, node_id: Option<&String>, query: Option<&MatchedQuery>| {
        let index = match order.iter().position(|known| known.skill_id == *skill_id) {
            Some(index) => index,
            None => {
                order.push(Reached {
                    skill_id: skill_id.clone(),
                    attached_to: Vec::new(),
                    matched_queries: Vec::new(),
                });
                order.len() - 1
            }
        };
        let entry = &mut order[index];
        if let Some(node_id) = node_id {
            if !entry.attached_to.contains(node_id) {
                entry.attached_to.push(node_id.clone());
            }
        }
        if let Some(query) = query {
            if !entry.matched_queries.contains(query) {
                entry.matched_queries.push(query.clone());
            }
        }
    };
    let skills_of = |id: &String| by_target.get(id).into_iter().flatten();
    let (own, reached) = returned_ids.split_at(returned_ids.len().min(1));
    for node_id in own {
        for skill_id in skills_of(node_id) {
            reach(skill_id, Some(node_id), None);
        }
    }
    for query in matched {
        for skill_id in skills_of(&query.id) {
            reach(skill_id, None, Some(query));
        }
    }
    for node_id in reached {
        for skill_id in skills_of(node_id) {
            reach(skill_id, Some(node_id), None);
        }
    }
    if order.is_empty() {
        return Ok(AttachedSkills::default());
    }

    let skill_ids: Vec<String> = order.iter().map(|known| known.skill_id.clone()).collect();
    let guidance = cache.guidance(node_service, skill_ids).await?;
    let skills = guidance
        .skills
        .into_iter()
        .map(|skill| {
            let (attached_to, matched_queries) = order
                .iter()
                .find(|known| known.skill_id == skill.id)
                .map(|known| (known.attached_to.clone(), known.matched_queries.clone()))
                .unwrap_or_default();
            AttachedSkill {
                skill,
                attached_to,
                matched_queries,
            }
        })
        .collect();
    Ok(AttachedSkills {
        skills,
        schemas: guidance.schemas,
    })
}

/// What the items of one call share, read once: the context paths of each
/// type, and each set of skills as it is handed over. The items of a queue
/// are mostly of one type and carry the same procedure, so a run with context
/// renders that procedure once and not once per item.
///
/// It lives for one call and is never kept: every call reads what is stored.
#[derive(Default)]
struct ReadCache {
    context_paths: HashMap<String, Vec<(RelationshipPath, String)>>,
    guidance: HashMap<Vec<String>, SkillGuidance>,
}

impl ReadCache {
    async fn context_paths(
        &mut self,
        node_service: &NodeService,
        node_type: &str,
    ) -> Result<Vec<(RelationshipPath, String)>, OpsError> {
        if let Some(known) = self.context_paths.get(node_type) {
            return Ok(known.clone());
        }
        let paths = node_service.resolve_context_paths(node_type).await?;
        self.context_paths
            .insert(node_type.to_string(), paths.clone());
        Ok(paths)
    }

    /// The skills among `skill_ids`, in that order, as a fetch by name
    /// returns them, with their schemas. An id that is archived, or is not a
    /// skill, is left out.
    async fn guidance(
        &mut self,
        node_service: &NodeService,
        skill_ids: Vec<String>,
    ) -> Result<SkillGuidance, OpsError> {
        if let Some(known) = self.guidance.get(&skill_ids) {
            return Ok(known.clone());
        }
        let mut loaded = node_service
            .store()
            .get_nodes_by_ids(&skill_ids)
            .await
            .map_err(|e| OpsError::Internal(format!("Failed to read attached skills: {e}")))?;

        let mut skill_nodes = Vec::with_capacity(skill_ids.len());
        for skill_id in &skill_ids {
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
        self.guidance.insert(skill_ids, guidance.clone());
        Ok(guidance)
    }
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
