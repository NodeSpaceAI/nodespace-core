use chrono::{DateTime, NaiveDate, Utc};

use crate::ai_chat::{AiChatMessageNode, AiChatNativeNode, AiChatPtyNode};
use crate::collection::CollectionNode;
use crate::core_type::CoreNodeType;
use crate::database_settings::{DatabaseSettingsFields, DatabaseSettingsNode};
use crate::decision::DecisionNode;
use crate::node::{Node, NodeEnvelope};
use crate::person::PersonNode;
use crate::plan::PlanNode;
use crate::play::{PlayFields, PlayNode};
use crate::priority::priority_prop;
use crate::project::{ProjectNode, ProjectStatus};
use crate::query::{QueryFields, QueryNode};
use crate::schema::LinkValue;
use crate::skill::{SkillFields, SkillNode};
use crate::spec::SpecNode;
use crate::task::{TaskNode, TaskStatus};

fn normalize_date_field(s: &str) -> String {
    if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return s.to_string();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.format("%Y-%m-%d").to_string();
    }
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return dt.format("%Y-%m-%d").to_string();
    }
    s.to_string()
}

/// Convert a `Node` to its strongly-typed JSON representation for the frontend.
///
/// For a core type with a typed wire struct (`WireShape::Typed` in the
/// registry), promotes the type's fields to top-level fields. Every other
/// node travels in the generic shape: a primitive, a core type whose fields
/// stay in `properties`, and any user-defined or extension type. A subtype is
/// never converted to its base's struct (ADR-086 §5), so the match is on the
/// node's exact type. A `schema` node also keeps the generic shape here: its
/// typed struct, `SchemaNode`, is filled by the store and returned by the
/// schema reads. Adds a `nodespace://` URI field for rich client rendering.
///
/// This is the single canonical implementation used by all entry points
/// (Tauri commands, MCP, HTTP) and the SOLE authority for property flattening
/// and `nodespace://` URI production. Do NOT re-implement either in another layer
/// (e.g. a TypeScript-side flatten in the frontend converters) — the frontend
/// `nodeTo*` converters trust the flat, top-level shape this function guarantees.
/// The `wire_contract` tests below pin that shape.
pub fn node_to_typed_value(node: Node) -> Result<serde_json::Value, String> {
    let mut node = node;
    // Read before flattening drops the `_seed` marker.
    let is_seeded = PlayFields::seeded_in(&node.properties);
    flatten_properties_for_api(&mut node);

    let node_id = node.id.clone();
    let generic = |node: Node| {
        serde_json::to_value(node).map_err(|e| format!("Failed to serialize node: {}", e))
    };
    // No catch-all over the core types: a new variant has to say here which
    // shape it travels in.
    let mut value = match CoreNodeType::from_id(&node.node_type) {
        Some(CoreNodeType::Task) => task_node_to_value(node),
        Some(CoreNodeType::AiChatNative) => ai_chat_native_node_to_value(node),
        Some(CoreNodeType::AiChatPty) => ai_chat_pty_node_to_value(node),
        Some(CoreNodeType::AiChatMessage) => ai_chat_message_node_to_value(node),
        Some(CoreNodeType::Person) => person_node_to_value(node),
        Some(CoreNodeType::Project) => project_node_to_value(node),
        Some(CoreNodeType::Spec) => spec_node_to_value(node),
        Some(CoreNodeType::Plan) => plan_node_to_value(node),
        Some(CoreNodeType::Decision) => decision_node_to_value(node),
        Some(CoreNodeType::Collection) => collection_node_to_value(node),
        Some(CoreNodeType::Skill) => skill_node_to_value(node),
        Some(CoreNodeType::DatabaseSettings) => database_settings_node_to_value(node),
        Some(CoreNodeType::Query) => query_node_to_value(node),
        Some(CoreNodeType::Play) => play_node_to_value(node, is_seeded),
        Some(
            CoreNodeType::Text
            | CoreNodeType::Header
            | CoreNodeType::CodeBlock
            | CoreNodeType::QuoteBlock
            | CoreNodeType::OrderedList
            | CoreNodeType::Checkbox
            | CoreNodeType::HorizontalLine
            | CoreNodeType::Table
            | CoreNodeType::Date
            | CoreNodeType::AgentGuidance
            // A `SchemaNode` is filled by the store, from the schema's row
            // and its declaration edges, and read through the schema reads.
            // A schema node read as a plain node keeps the generic shape.
            | CoreNodeType::Schema
            // A tool's fields travel inside `properties`.
            | CoreNodeType::ToolNative
            // Abstract: no node has it as its type, and a subtype read at its
            // scope keeps the generic shape rather than borrowing a struct.
            | CoreNodeType::AiChat
            | CoreNodeType::Tool,
        ) => generic(node),
        None => generic(node),
    }?;

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "uri".to_string(),
            serde_json::Value::String(format!("nodespace://{}", node_id)),
        );
    }

    Ok(value)
}

/// Convert a vec of nodes to their strongly-typed JSON representations.
pub fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<serde_json::Value>, String> {
    nodes.into_iter().map(node_to_typed_value).collect()
}

/// Flatten namespaced properties into the flat API shape.
///
/// Storage format: `{ "task": { "status": "open" } }`
/// API format:     `{ "status": "open" }`
///
/// This is the single definition of the rule. Callers that hold a `Node` should
/// go through [`node_to_typed_value`]; this function exists for the callers that
/// do not — notably the CLI, which only ever has the gRPC `NodeData` (whose
/// `properties` is a JSON-encoded string) and so cannot build a `Node` to pass.
/// Both share this body so the two surfaces cannot drift apart.
///
/// `_`-prefixed keys (`_schema_version`, and sibling namespaces like `_seed`)
/// are internal and never exposed. Dormant namespaces left by a previous type
/// change are not exposed either.
///
/// Note the asymmetry between the branches: inside the type's own namespace an
/// object is a real schema-defined field value and is preserved, whereas in the
/// already-flat fallback a nested object can only be another type's namespace
/// and is dropped. A `schema` node is the one exception: its definition is
/// flat and never bucketed, so an object among its properties is a structural
/// rule and is kept.
///
/// This governs human-readable CLI output as well as JSON: `write_human_node`
/// and `node_to_json` share one call into it, so the two cannot disagree.
pub fn flatten_namespaced_properties(
    properties: &serde_json::Value,
    node_type: &str,
) -> serde_json::Value {
    flatten_namespaced_properties_at_scope(properties, std::slice::from_ref(&node_type))
}

/// Flatten namespaced properties at an explicit scope chain (ADR-078).
///
/// The general form of [`flatten_namespaced_properties`], which is now the
/// single-scope case. `scope_chain` is nearest-scope-first: reading an `issue`
/// node at its own scope passes `["issue", "task"]` and yields both its own
/// and its inherited fields, while reading the same node at `task` scope
/// passes `["task"]` and yields task's fields only — the issue's own fields
/// **absent**, not merely unresolved. That truncation is what makes a
/// base-scoped query return a homogeneous result set.
///
/// A nearer scope wins any key collision. Well-formed chains have none
/// (redeclaration is rejected at write time); this only decides the
/// retroactive-collision case ADR-078 leaves open.
///
/// Stays pure and free of any schema lookup: the caller resolves the chain,
/// because every caller is in code that can reach the store and this crate
/// cannot. Passing a single scope reads that type's bucket alone.
pub fn flatten_namespaced_properties_at_scope(
    properties: &serde_json::Value,
    scope_chain: &[&str],
) -> serde_json::Value {
    let Some(props_obj) = properties.as_object() else {
        return properties.clone();
    };

    // A schema's definition is not bucketed: its properties are flat, and an
    // object among them is a structural rule (`children`, `parent`), not
    // another type's namespace. It is read as stored.
    if scope_chain
        .first()
        .is_some_and(|scope| CoreNodeType::Schema.is_exactly(scope))
    {
        return serde_json::Value::Object(
            props_obj
                .iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
    }

    // Any bucket in the chain present? If so, the node is in storage shape and
    // the chain decides what is visible.
    let has_any_bucket = scope_chain
        .iter()
        .any(|scope| props_obj.get(*scope).and_then(|v| v.as_object()).is_some());

    if has_any_bucket {
        let mut out = serde_json::Map::new();
        for scope in scope_chain {
            let Some(bucket) = props_obj.get(*scope).and_then(|v| v.as_object()) else {
                continue;
            };
            for (k, v) in bucket {
                if k.starts_with('_') {
                    continue;
                }
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        return serde_json::Value::Object(out);
    }

    // Already-flat fallback, unchanged: a nested object here can only be
    // another type's namespace, so it is dropped.
    serde_json::Value::Object(
        props_obj
            .iter()
            .filter(|(k, v)| !v.is_object() && !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// Flatten namespaced properties for API response, in place.
///
/// A core type's chain is the registry's, so a core subtype's inherited
/// buckets are read here directly. For any other type this is single-scope by
/// design: this crate has no store access and cannot resolve a user-defined
/// `extends` chain. The service layer collapses such a node's inherited
/// buckets into its own before the wire boundary
/// (`NodeService::collapse_chain_for_wire`), so one bucket carries the whole
/// effective property set by the time it reaches here — and a dormant bucket
/// left by an earlier type change stays excluded, as it always has been.
fn flatten_properties_for_api(node: &mut Node) {
    node.properties = match CoreNodeType::from_id(&node.node_type) {
        Some(core) => {
            let chain: Vec<&str> = core.chain().into_iter().map(CoreNodeType::as_str).collect();
            flatten_namespaced_properties_at_scope(&node.properties, &chain)
        }
        None => flatten_namespaced_properties(&node.properties, &node.node_type),
    };
}

/// The core fields a typed conversion promotes out of `properties`:
/// `due_date` is stored in the `task` bucket and travels as the top-level
/// `dueDate`.
///
/// Promotion is a move, not a copy — [`node_to_typed_value`] removes these keys
/// from `properties`, so each core field has exactly one home on the wire and
/// `properties` carries only extension fields. Consumers that want the flat,
/// storage-keyed view back (the CLI's snake_case node shape, the agent's
/// model-facing property map) rebuild it with [`flat_properties_view`] rather
/// than hard-coding these lists.
pub fn promoted_fields(node_type: &str) -> &'static [PromotedField] {
    match CoreNodeType::from_id(node_type) {
        Some(core) => core_promoted_fields(core),
        None => &[],
    }
}

/// The JSON shape a promoted field's stored value has. A stored value of any
/// other shape is dropped to the field's default rather than promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotedShape {
    /// A string, promoted as stored.
    Text,
    /// A string holding a date, normalized to `YYYY-MM-DD` on read.
    Date,
    Number,
    Boolean,
    Array,
    Object,
}

/// One core field a typed conversion promotes out of `properties`.
///
/// This is the one definition of the mapping. The frontend's
/// `TYPED_CORE_FIELDS` is generated from it (ADR-086 §8), and the core schemas
/// are pinned to it by `typed_wire_shapes_promote_their_schemas_fields`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromotedField {
    /// The schema field name, as stored in the type's bucket (`due_date`).
    pub storage: &'static str,
    /// The top-level key on the typed node (`dueDate`).
    pub wire: &'static str,
    pub shape: PromotedShape,
    /// System-managed: read on the wire, never set by a typed update.
    pub read_only: bool,
}

impl PromotedField {
    const fn new(storage: &'static str, wire: &'static str, shape: PromotedShape) -> Self {
        Self {
            storage,
            wire,
            shape,
            read_only: false,
        }
    }

    const fn text(storage: &'static str, wire: &'static str) -> Self {
        Self::new(storage, wire, PromotedShape::Text)
    }

    const fn date(storage: &'static str, wire: &'static str) -> Self {
        Self::new(storage, wire, PromotedShape::Date)
    }

    const fn read_only(self) -> Self {
        Self {
            read_only: true,
            ..self
        }
    }
}

/// [`promoted_fields`] for a core type. Matched without a catch-all, so a new
/// core type has to state which of its fields its wire struct promotes.
///
/// A core subtype's list covers its whole chain: the fields it inherits and
/// its own. `schema` has a typed wire struct but promotes nothing through this
/// list: its struct is built from properties the flat property view never
/// carries back.
pub fn core_promoted_fields(core: CoreNodeType) -> &'static [PromotedField] {
    use PromotedField as F;
    use PromotedShape::{Array, Boolean, Number, Object};
    match core {
        CoreNodeType::Task => {
            const {
                &[
                    F::text("status", "status"),
                    F::text("priority", "priority"),
                    F::date("due_date", "dueDate"),
                    F::date("started_at", "startedAt"),
                    F::date("completed_at", "completedAt"),
                    F::new("pull_request", "pullRequest", Object),
                    F::new("commits", "commits", Array),
                ]
            }
        }
        CoreNodeType::Person => {
            const {
                &[
                    F::text("first_name", "firstName"),
                    F::text("last_name", "lastName"),
                    F::text("email", "email"),
                ]
            }
        }
        CoreNodeType::Project => {
            const {
                &[
                    F::text("status", "status"),
                    F::text("priority", "priority"),
                    F::date("start_date", "startDate"),
                    F::date("end_date", "endDate"),
                    F::new("repository", "repository", Object),
                    F::text("checkout_path", "checkoutPath"),
                ]
            }
        }
        CoreNodeType::Spec => {
            const {
                &[
                    F::text("objective", "objective"),
                    F::text("boundaries", "boundaries"),
                    F::text("spec_status", "specStatus"),
                ]
            }
        }
        CoreNodeType::Plan => {
            const {
                &[
                    F::text("approach", "approach"),
                    F::text("risks", "risks"),
                    F::text("plan_status", "planStatus"),
                ]
            }
        }
        CoreNodeType::Decision => const { &[F::text("decision_status", "decisionStatus")] },
        CoreNodeType::Collection => const { &[F::text("description", "description")] },
        CoreNodeType::Skill => {
            const {
                &[
                    F::text("use_for", "useFor"),
                    F::text("not_for", "notFor"),
                    F::new("tool_whitelist", "toolWhitelist", Array),
                    F::new("max_iterations", "maxIterations", Number),
                    F::text("role", "role"),
                ]
            }
        }
        CoreNodeType::DatabaseSettings => {
            const {
                &[
                    F::new("required_extensions", "requiredExtensions", Array),
                    F::new("capture_enabled", "captureEnabled", Boolean),
                    F::text("capture_content", "captureContent"),
                    F::new("providers", "providers", Array),
                ]
            }
        }
        CoreNodeType::Query => {
            const {
                &[
                    F::text("target_type", "targetType"),
                    F::new("filters", "filters", Array),
                    F::new("sorting", "sorting", Array),
                    F::new("limit", "limit", Number),
                    F::text("generated_by", "generatedBy"),
                    F::text("generator_context", "generatorContext"),
                    F::new("execution_count", "executionCount", Number).read_only(),
                    F::text("last_executed", "lastExecuted").read_only(),
                    F::new("view_config", "viewConfig", Object),
                ]
            }
        }
        CoreNodeType::Play => {
            const {
                &[
                    F::new("rules", "rules", Array),
                    F::text("description", "description"),
                    F::new("enabled", "enabled", Boolean),
                    F::text("suspended_reason", "suspendedReason").read_only(),
                    F::text("suspended_message", "suspendedMessage").read_only(),
                    F::text("suspended_at", "suspendedAt").read_only(),
                ]
            }
        }
        CoreNodeType::Text
        | CoreNodeType::Header
        | CoreNodeType::CodeBlock
        | CoreNodeType::QuoteBlock
        | CoreNodeType::OrderedList
        | CoreNodeType::Checkbox
        | CoreNodeType::HorizontalLine
        | CoreNodeType::Table
        | CoreNodeType::Date
        | CoreNodeType::AgentGuidance
        | CoreNodeType::Schema
        | CoreNodeType::Tool
        | CoreNodeType::ToolNative => &[],
        // The chat family (ADR-088). The base's fields come first in each
        // subtype's list, as each subtype's struct embeds them. None is marked
        // read-only: the flag says what a typed update may set, and the
        // family has no typed update.
        CoreNodeType::AiChat => {
            const {
                &[
                    F::text("agent", "agent"),
                    F::text("model", "model"),
                    F::text("summary", "summary"),
                    F::text("last_active", "lastActive"),
                ]
            }
        }
        CoreNodeType::AiChatNative => {
            const {
                &[
                    F::text("agent", "agent"),
                    F::text("model", "model"),
                    F::text("summary", "summary"),
                    F::text("last_active", "lastActive"),
                    F::text("provider", "provider"),
                    F::text("turn_status", "turnStatus"),
                    F::new("context_tokens", "contextTokens", Number),
                ]
            }
        }
        CoreNodeType::AiChatPty => {
            const {
                &[
                    F::text("agent", "agent"),
                    F::text("model", "model"),
                    F::text("summary", "summary"),
                    F::text("last_active", "lastActive"),
                    F::text("session_status", "sessionStatus"),
                    F::text("session_id", "sessionId"),
                    F::text("transcript", "transcript"),
                    F::new("exit_code", "exitCode", Number),
                ]
            }
        }
        // A message's text is its `content`; these are its other fields.
        CoreNodeType::AiChatMessage => {
            const {
                &[
                    F::text("role", "role"),
                    F::text("timestamp", "timestamp"),
                    F::text("reasoning", "reasoning"),
                    F::text("outcome", "outcome"),
                    F::new("options", "options", Array),
                ]
            }
        }
    }
}

/// The storage keys a typed client must write through `core`'s typed update
/// rather than the generic one: the promoted fields of a core type that has a
/// typed update (ADR-086 §7). Empty for every other type, whose fields have no
/// typed write path to prefer.
pub fn typed_update_fields(core: CoreNodeType) -> &'static [PromotedField] {
    match core.wire() {
        crate::core_type::WireShape::Typed { update: true } => core_promoted_fields(core),
        crate::core_type::WireShape::Typed { update: false }
        | crate::core_type::WireShape::Generic
        | crate::core_type::WireShape::Envelope => &[],
    }
}

/// Remove a type's promoted core fields from its flat `properties`.
fn without_promoted(mut properties: serde_json::Value, core: CoreNodeType) -> serde_json::Value {
    if let Some(obj) = properties.as_object_mut() {
        for field in core_promoted_fields(core) {
            obj.remove(field.storage);
        }
    }
    properties
}

/// Rebuild a typed node value's flat, storage-keyed property map: its
/// `properties` plus every promoted core field folded back under its storage
/// key (`dueDate` → `due_date`). Null and absent core fields are left out.
///
/// For consumers whose contract is the flat property map — the CLI's node
/// shape and the agent's model-facing summaries, both of which also *write*
/// with these bare keys. The frontend reads the typed fields directly.
pub fn flat_properties_view(typed: &serde_json::Value) -> serde_json::Value {
    let mut props = typed
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    if let Some(node_type) = typed.get("nodeType").and_then(|v| v.as_str()) {
        for field in promoted_fields(node_type) {
            if let Some(value) = typed.get(field.wire).filter(|v| !v.is_null()) {
                props
                    .entry(field.storage.to_string())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    serde_json::Value::Object(props)
}

fn task_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;

    let status = props
        .get("status")
        .and_then(|v| v.as_str())
        .map(TaskStatus::from_value)
        .unwrap_or_default();

    let priority = priority_prop(props);

    let due_date = props
        .get("due_date")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let started_at = props
        .get("started_at")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let completed_at = props
        .get("completed_at")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let pull_request = link_prop(props, "pull_request");
    let commits = props
        .get("commits")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| LinkValue::from_json(item).ok())
                .collect()
        });

    let task = TaskNode {
        envelope: extension_envelope(node, CoreNodeType::Task),
        status,
        priority,
        due_date,
        started_at,
        completed_at,
        pull_request,
        commits,
    };

    serde_json::to_value(&task).map_err(|e| format!("Failed to serialize task node: {}", e))
}

/// The envelope of a typed node: the node with its promoted core fields taken
/// out of `properties`, which then holds extension fields only.
fn extension_envelope(node: Node, core: CoreNodeType) -> NodeEnvelope {
    let Node { properties, .. } = &node;
    let properties = without_promoted(properties.clone(), core);
    NodeEnvelope { properties, ..node }
}

fn string_prop(props: &serde_json::Value, key: &str) -> Option<String> {
    props.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// A stored link field. A stored value that is not a link reads as absent:
/// writes are checked by the schema, so this only meets a row written around
/// the service layer.
fn link_prop(props: &serde_json::Value, key: &str) -> Option<LinkValue> {
    props.get(key).and_then(|v| LinkValue::from_json(v).ok())
}

/// A stored closed-enum field. A missing or unknown value reads as the
/// schema default, which the enum's `Default` is pinned to.
fn closed_enum_prop<T: serde::de::DeserializeOwned + Default>(
    props: &serde_json::Value,
    key: &str,
) -> T {
    props
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn spec_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;
    let objective = string_prop(props, "objective");
    let boundaries = string_prop(props, "boundaries");
    let spec_status = closed_enum_prop(props, "spec_status");

    let spec = SpecNode {
        envelope: extension_envelope(node, CoreNodeType::Spec),
        objective,
        boundaries,
        spec_status,
    };

    serde_json::to_value(&spec).map_err(|e| format!("Failed to serialize spec node: {}", e))
}

fn plan_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;
    let approach = string_prop(props, "approach");
    let risks = string_prop(props, "risks");
    let plan_status = closed_enum_prop(props, "plan_status");

    let plan = PlanNode {
        envelope: extension_envelope(node, CoreNodeType::Plan),
        approach,
        risks,
        plan_status,
    };

    serde_json::to_value(&plan).map_err(|e| format!("Failed to serialize plan node: {}", e))
}

fn decision_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let decision_status = closed_enum_prop(&node.properties, "decision_status");

    let decision = DecisionNode {
        envelope: extension_envelope(node, CoreNodeType::Decision),
        decision_status,
    };

    serde_json::to_value(&decision).map_err(|e| format!("Failed to serialize decision node: {}", e))
}

fn person_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;
    let first_name = string_prop(props, "first_name");
    let last_name = string_prop(props, "last_name");
    let email = string_prop(props, "email");

    let person = PersonNode {
        envelope: extension_envelope(node, CoreNodeType::Person),
        first_name,
        last_name,
        email,
    };

    serde_json::to_value(&person).map_err(|e| format!("Failed to serialize person node: {}", e))
}

fn project_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;
    let status = props
        .get("status")
        .and_then(|v| v.as_str())
        .map(ProjectStatus::from_value)
        .unwrap_or_default();
    let priority = priority_prop(props);
    let start_date = props
        .get("start_date")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);
    let end_date = props
        .get("end_date")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let repository = link_prop(props, "repository");
    let checkout_path = props
        .get("checkout_path")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let project = ProjectNode {
        envelope: extension_envelope(node, CoreNodeType::Project),
        status,
        priority,
        start_date,
        end_date,
        repository,
        checkout_path,
    };

    serde_json::to_value(&project).map_err(|e| format!("Failed to serialize project node: {}", e))
}

fn collection_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let description = string_prop(&node.properties, "description");

    let collection = CollectionNode {
        envelope: extension_envelope(node, CoreNodeType::Collection),
        description,
    };

    serde_json::to_value(&collection)
        .map_err(|e| format!("Failed to serialize collection node: {}", e))
}

/// A stored skill whose fields do not decode keeps its node in the batch with
/// the schema defaults, as a malformed query does. Writes are checked by
/// `SkillNodeBehavior::validate`, so this only meets a row written around the
/// service layer.
fn skill_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let fields = SkillFields::from_properties(&node.properties).unwrap_or_else(|e| {
        eprintln!("skill node '{}' has unreadable fields: {e}", node.id);
        SkillFields::default()
    });

    let skill = SkillNode {
        envelope: extension_envelope(node, CoreNodeType::Skill),
        fields,
    };

    serde_json::to_value(&skill).map_err(|e| format!("Failed to serialize skill node: {}", e))
}

/// A stored settings node whose fields do not decode keeps its node in the
/// batch with the schema defaults, as a malformed query does. Writes are
/// checked by `DatabaseSettingsNodeBehavior::validate`, and the open guard
/// reads the stored extension list itself rather than this shape.
fn database_settings_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let fields = DatabaseSettingsFields::from_properties(&node.properties).unwrap_or_else(|e| {
        eprintln!(
            "database-settings node '{}' has unreadable fields: {e}",
            node.id
        );
        DatabaseSettingsFields::default()
    });

    let settings = DatabaseSettingsNode {
        envelope: extension_envelope(node, CoreNodeType::DatabaseSettings),
        fields,
    };

    serde_json::to_value(&settings)
        .map_err(|e| format!("Failed to serialize database-settings node: {}", e))
}

/// A stored query whose fields do not decode keeps its node in the batch with
/// the schema defaults rather than failing every other node alongside it —
/// the same batch-safety rule as a malformed schema node. Writes are checked
/// by `QueryNodeBehavior::validate`, so this only meets a row written around
/// the service layer.
fn query_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let fields = QueryFields::from_properties(&node.properties).unwrap_or_else(|e| {
        eprintln!("query node '{}' has unreadable fields: {e}", node.id);
        QueryFields::from_properties(&serde_json::json!({}))
            .expect("empty properties always decode")
    });

    let query = QueryNode {
        envelope: extension_envelope(node, CoreNodeType::Query),
        fields,
    };

    serde_json::to_value(&query).map_err(|e| format!("Failed to serialize query node: {}", e))
}

/// A stored play whose fields do not decode keeps its node in the batch with
/// the fields that do decode and the schema defaults for the rest, as a
/// malformed query does. Writes are checked by `PlayNodeBehavior::validate`,
/// so this only meets a row written around the service layer. Such a play is
/// one the engine suspends, so its switch and suspension are kept readable.
///
/// `is_seeded` is read from the stored properties by the caller: the `_seed`
/// marker is gone from the flattened `node` this receives.
fn play_node_to_value(node: Node, is_seeded: bool) -> Result<serde_json::Value, String> {
    let fields = PlayFields::from_properties(&node.properties).unwrap_or_else(|e| {
        eprintln!("play node '{}' has unreadable fields: {e}", node.id);
        PlayFields::readable_from_properties(&node.properties)
    });

    let play = PlayNode {
        envelope: extension_envelope(node, CoreNodeType::Play),
        fields,
        is_seeded,
    };

    serde_json::to_value(&play).map_err(|e| format!("Failed to serialize play node: {}", e))
}

fn ai_chat_native_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let chat = AiChatNativeNode::from_node(node).map_err(|e| e.to_string())?;
    serde_json::to_value(&chat).map_err(|e| format!("Failed to serialize ai-chat-native node: {e}"))
}

fn ai_chat_message_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let message = AiChatMessageNode::from_node(node).map_err(|e| e.to_string())?;
    serde_json::to_value(&message)
        .map_err(|e| format!("Failed to serialize ai-chat-message node: {e}"))
}

fn ai_chat_pty_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let chat = AiChatPtyNode::from_node(node).map_err(|e| e.to_string())?;
    serde_json::to_value(&chat).map_err(|e| format!("Failed to serialize ai-chat-pty node: {e}"))
}

#[cfg(test)]
mod wire_contract {
    use super::*;
    use crate::node::Node;

    // These tests pin the wire contract that the frontend TS converters trust:
    // for typed nodes, type-specific fields are promoted to TOP-LEVEL and the
    // namespaced `properties.<type>` form is flattened away. If this contract
    // ever changes, the frontend nodeTo* converters must change in lockstep.

    #[test]
    fn task_promotes_fields_top_level_and_flattens_properties() {
        let node = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({
                "task": { "status": "in_progress", "priority": "high", "custom:store": "Costco" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        // Core fields promoted to top level.
        assert_eq!(out["status"], "in_progress");
        assert_eq!(out["priority"], "high");
        // `properties` is flattened: no `task` namespace survives.
        assert!(out["properties"].get("task").is_none());
        // User/custom fields remain in flat `properties`.
        assert_eq!(out["properties"]["custom:store"], "Costco");
        // Promotion is a move: core fields have one home, the top level.
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:store": "Costco" })
        );
        // URI is injected by the backend.
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    #[test]
    fn flat_properties_view_folds_promoted_fields_back_under_storage_keys() {
        let node = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({
                "task": { "status": "done", "due_date": "2026-05-01", "custom:store": "Costco" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(
            flat_properties_view(&out),
            serde_json::json!({
                "status": "done",
                "due_date": "2026-05-01",
                "custom:store": "Costco"
            })
        );
    }

    #[test]
    fn collection_promotes_its_description_and_keeps_the_envelope() {
        let node = Node::new(
            "collection".to_string(),
            "Clients".to_string(),
            serde_json::json!({
                "collection": { "description": "Accounts we bill", "custom:owner": "Ada" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "collection");
        assert_eq!(out["content"], "Clients");
        assert_eq!(out["lifecycleStatus"], "active");
        assert_eq!(out["version"], 1);
        assert_eq!(out["description"], "Accounts we bill");
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:owner": "Ada" })
        );
    }

    #[test]
    fn collection_without_a_description_omits_it() {
        for properties in [
            serde_json::json!({}),
            serde_json::json!({ "collection": { "description": null } }),
        ] {
            let node = Node::new("collection".to_string(), "Clients".to_string(), properties);
            let out = node_to_typed_value(node).unwrap();
            assert!(out.get("description").is_none());
            assert_eq!(out["properties"], serde_json::json!({}));
        }
    }

    #[test]
    fn skill_promotes_fields_top_level_and_keeps_the_envelope() {
        let node = Node::new(
            "skill".to_string(),
            "Graph Editing".to_string(),
            serde_json::json!({
                "skill": {
                    "use_for": "Update a record",
                    "not_for": "Delete records",
                    "tool_whitelist": ["update_node", "get_node"],
                    "max_iterations": 3,
                    "custom:team": "Agents"
                },
                "_seed": { "tier": "system" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "skill");
        assert_eq!(out["content"], "Graph Editing");
        assert_eq!(out["lifecycleStatus"], "active");
        assert_eq!(out["useFor"], "Update a record");
        assert_eq!(out["notFor"], "Delete records");
        assert_eq!(
            out["toolWhitelist"],
            serde_json::json!(["update_node", "get_node"])
        );
        assert_eq!(out["maxIterations"], 3);
        assert_eq!(out["role"], "tool");
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:team": "Agents" })
        );
        assert_eq!(
            flat_properties_view(&out),
            serde_json::json!({
                "use_for": "Update a record",
                "not_for": "Delete records",
                "tool_whitelist": ["update_node", "get_node"],
                "max_iterations": 3,
                "role": "tool",
                "custom:team": "Agents"
            })
        );
    }

    #[test]
    fn skill_without_fields_takes_the_schema_defaults() {
        let node = Node::new(
            "skill".to_string(),
            "Research".to_string(),
            serde_json::json!({}),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["useFor"], "");
        assert!(out.get("notFor").is_none());
        assert_eq!(out["toolWhitelist"], serde_json::json!([]));
        assert_eq!(out["maxIterations"], 2);
    }

    /// One malformed skill must not fail the batch it travels in.
    #[test]
    fn a_malformed_skill_keeps_its_node_with_the_defaults() {
        let node = Node::new(
            "skill".to_string(),
            "Research".to_string(),
            serde_json::json!({ "skill": { "tool_whitelist": "get_node" } }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["content"], "Research");
        assert_eq!(out["toolWhitelist"], serde_json::json!([]));
    }

    #[test]
    fn database_settings_promotes_required_extensions_and_keeps_the_envelope() {
        let node = Node::new_with_id(
            "database-settings-singleton".to_string(),
            "database-settings".to_string(),
            "Database Settings".to_string(),
            serde_json::json!({
                "database-settings": { "required_extensions": ["fixture"] }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["id"], "database-settings-singleton");
        assert_eq!(out["nodeType"], "database-settings");
        assert_eq!(out["lifecycleStatus"], "active");
        assert_eq!(out["requiredExtensions"], serde_json::json!(["fixture"]));
        assert_eq!(out["properties"], serde_json::json!({}));
    }

    #[test]
    fn database_settings_without_a_list_reads_as_empty() {
        for properties in [
            serde_json::json!({}),
            serde_json::json!({ "database-settings": { "required_extensions": null } }),
        ] {
            let node = Node::new(
                "database-settings".to_string(),
                "Database Settings".to_string(),
                properties,
            );
            let out = node_to_typed_value(node).unwrap();
            assert_eq!(out["requiredExtensions"], serde_json::json!([]));
        }
    }

    #[test]
    fn flat_properties_view_leaves_untyped_nodes_alone() {
        let node = Node::new(
            "invoice".to_string(),
            "INV-1".to_string(),
            serde_json::json!({ "invoice": { "amount": 5 } }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(
            flat_properties_view(&out),
            serde_json::json!({ "amount": 5 })
        );
    }

    #[test]
    fn person_promotes_fields_top_level_and_flattens_properties() {
        let mut node = Node::new(
            "person".to_string(),
            String::new(),
            serde_json::json!({
                "person": {
                    "first_name": "Ada",
                    "last_name": "Lovelace",
                    "email": "ada@example.com",
                    "custom:team": "Engines"
                }
            }),
        );
        node.title = Some("Ada Lovelace".to_string());
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "person");
        assert_eq!(out["firstName"], "Ada");
        assert_eq!(out["lastName"], "Lovelace");
        assert_eq!(out["email"], "ada@example.com");
        assert_eq!(out["title"], "Ada Lovelace");
        assert!(out["properties"].get("person").is_none());
        assert_eq!(out["properties"]["custom:team"], "Engines");
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    #[test]
    fn person_with_unset_or_null_fields_omits_them() {
        let node = Node::new(
            "person".to_string(),
            String::new(),
            serde_json::json!({ "person": { "first_name": "Ada", "email": null } }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["firstName"], "Ada");
        assert!(out.get("lastName").is_none());
        assert!(out.get("email").is_none());
    }

    #[test]
    fn project_promotes_fields_top_level_and_normalizes_dates() {
        let node = Node::new(
            "project".to_string(),
            "Launch".to_string(),
            serde_json::json!({
                "project": {
                    "status": "active",
                    "priority": "high",
                    "start_date": "2026-03-01T09:00:00Z",
                    "end_date": "2026-04-30",
                    "custom:budget": 1200
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "project");
        assert_eq!(out["status"], "active");
        assert_eq!(out["priority"], "high");
        assert_eq!(out["startDate"], "2026-03-01");
        assert_eq!(out["endDate"], "2026-04-30");
        assert!(out["properties"].get("project").is_none());
        assert_eq!(out["properties"]["custom:budget"], 1200);
    }

    #[test]
    fn project_without_status_defaults_to_planning() {
        let node = Node::new(
            "project".to_string(),
            "Launch".to_string(),
            serde_json::json!({ "project": {} }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["status"], "planning");
        assert!(out.get("priority").is_none());
        assert!(out.get("startDate").is_none());
    }

    #[test]
    fn query_promotes_fields_top_level_and_empties_properties() {
        let node = Node::new(
            "query".to_string(),
            "Issues by Status".to_string(),
            serde_json::json!({
                "query": {
                    "target_type": "issue",
                    "filters": [
                        { "type": "property", "operator": "equals", "property": "status", "value": "open" }
                    ],
                    "sorting": [{ "field": "start_date", "direction": "desc" }],
                    "limit": 50,
                    "generated_by": "user",
                    "execution_count": 0,
                    "view_config": { "lastView": "kanban", "kanban": { "groupBy": "status" } },
                    "custom:pinned": true
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "query");
        assert_eq!(out["targetType"], "issue");
        assert_eq!(out["filters"][0]["property"], "status");
        assert_eq!(out["sorting"][0]["direction"], "desc");
        assert_eq!(out["limit"], 50);
        assert_eq!(out["generatedBy"], "user");
        assert_eq!(out["executionCount"], 0);
        assert_eq!(out["viewConfig"]["kanban"]["groupBy"], "status");
        assert!(out.get("generatorContext").is_none());
        assert!(out.get("lastExecuted").is_none());
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:pinned": true }),
            "properties carries only extension fields"
        );
    }

    #[test]
    fn play_promotes_fields_top_level_and_empties_properties() {
        let node = Node::new(
            "play".to_string(),
            "Roll completion up".to_string(),
            serde_json::json!({
                "play": {
                    "description": "When every sub-task is done, mark the parent done",
                    "rules": [{
                        "name": "close-parent",
                        "description": "Test rule",
                        "trigger": {
                            "type": "graph_event",
                            "on": "property_changed",
                            "select": { "target_type": "task" },
                            "property_key": "task.status"
                        },
                        "actions": [{
                            "description": "Test action",
                            "action_type": "update_node",
                            "params": { "node_id": "{trigger.node.child_of.id}" }
                        }]
                    }],
                    "custom:owner": "ada"
                },
                "_seed": { "tier": "core" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "play");
        assert_eq!(out["lifecycleStatus"], "active");
        assert_eq!(
            out["description"],
            "When every sub-task is done, mark the parent done"
        );
        // Rule keys are snake_case on the wire as in storage.
        let rule = &out["rules"][0];
        assert_eq!(rule["trigger"]["property_key"], "task.status");
        assert_eq!(rule["trigger"]["select"]["target_type"], "task");
        assert_eq!(rule["actions"][0]["action_type"], "update_node");
        assert_eq!(
            rule["class"], "reactive",
            "the default class is written out"
        );
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:owner": "ada" }),
            "properties keeps extension fields only"
        );
        assert_eq!(out["isSeeded"], true, "derived from the `_seed` marker");
    }

    #[test]
    fn play_without_a_seed_marker_is_not_seeded() {
        let node = Node::new(
            "play".to_string(),
            "My play".to_string(),
            serde_json::json!({ "play": { "rules": [] } }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["isSeeded"], false);
    }

    #[test]
    fn play_with_malformed_rules_does_not_fail_an_unrelated_batch_read() {
        let bad_play = Node::new(
            "play".to_string(),
            "Broken".to_string(),
            serde_json::json!({ "play": { "rules": [{ "name": "r", "description": "Test rule", "trigger": { "type": "nope" } }] } }),
        );
        let good_task = Node::new(
            "task".to_string(),
            "Fine".to_string(),
            serde_json::json!({ "task": { "status": "open" } }),
        );

        let out = nodes_to_typed_values(vec![bad_play, good_task])
            .expect("one malformed play must not fail the whole batch");
        assert_eq!(out[0]["rules"], serde_json::json!([]));
        assert_eq!(out[1]["status"], "open");
    }

    #[test]
    fn query_with_malformed_filters_does_not_fail_an_unrelated_batch_read() {
        let bad_query = Node::new(
            "query".to_string(),
            "Broken".to_string(),
            serde_json::json!({ "query": { "target_type": "task", "filters": "oops" } }),
        );
        let good_task = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({ "task": { "status": "open" } }),
        );

        let out = nodes_to_typed_values(vec![bad_query, good_task])
            .expect("one malformed query must not fail the whole batch");

        assert_eq!(out[0]["targetType"], "*");
        assert_eq!(out[0]["filters"], serde_json::json!([]));
        assert_eq!(out[1]["status"], "open");
    }

    #[test]
    fn ai_chat_native_promotes_its_chain_and_empties_properties() {
        let node = Node::new(
            "ai-chat-native".to_string(),
            "Chat".to_string(),
            serde_json::json!({
                "ai-chat": { "agent": "nodespace", "model": "gemma-4-e4b" },
                "ai-chat-native": {
                    "turn_status": "processing",
                    "provider": "openai-compat",
                    "context_tokens": 12,
                    "custom:pinned": true
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "ai-chat-native");
        assert_eq!(out["agent"], "nodespace");
        assert_eq!(out["model"], "gemma-4-e4b");
        assert_eq!(out["turnStatus"], "processing");
        assert_eq!(out["provider"], "openai-compat");
        assert_eq!(out["contextTokens"], 12);
        // A native chat has no session state and carries no messages (they
        // are its children), and an unset optional field is absent rather
        // than null.
        assert!(out.get("sessionStatus").is_none());
        assert!(out.get("messages").is_none());
        assert!(out.get("summary").is_none());
        // Each declared field has one home: `properties` keeps extension
        // fields only.
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:pinned": true })
        );
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    #[test]
    fn ai_chat_pty_promotes_its_chain_and_empties_properties() {
        let node = Node::new(
            "ai-chat-pty".to_string(),
            "Session".to_string(),
            serde_json::json!({
                "ai-chat": { "agent": "claude-code", "summary": "Fixed the build" },
                "ai-chat-pty": {
                    "session_status": "ended",
                    "session_id": "s-1",
                    "transcript": "hello",
                    "exit_code": 0
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "ai-chat-pty");
        assert_eq!(out["agent"], "claude-code");
        assert_eq!(out["summary"], "Fixed the build");
        assert_eq!(out["sessionStatus"], "ended");
        assert_eq!(out["sessionId"], "s-1");
        assert_eq!(out["transcript"], "hello");
        assert_eq!(out["exitCode"], 0);
        // A terminal chat has no messages, turn state or provider.
        for absent in ["messages", "turnStatus", "provider", "model"] {
            assert!(out.get(absent).is_none(), "{absent} must be absent");
        }
        assert_eq!(out["properties"], serde_json::json!({}));
    }

    /// Absent closed-enum fields read as the schema's defaults, so the wire
    /// shape always carries a value from the vocabulary.
    #[test]
    fn ai_chat_absent_enums_read_as_the_schema_defaults() {
        let native = node_to_typed_value(Node::new(
            "ai-chat-native".to_string(),
            "Chat".to_string(),
            serde_json::json!({}),
        ))
        .unwrap();
        assert_eq!(native["provider"], "native");
        assert_eq!(native["turnStatus"], "idle");
        assert_eq!(native["contextTokens"], 0);

        let message = node_to_typed_value(Node::new(
            "ai-chat-message".to_string(),
            "hi".to_string(),
            serde_json::json!({}),
        ))
        .unwrap();
        assert_eq!(message["role"], "user");

        let pty = node_to_typed_value(Node::new(
            "ai-chat-pty".to_string(),
            "Session".to_string(),
            serde_json::json!({}),
        ))
        .unwrap();
        assert_eq!(pty["sessionStatus"], "active");
    }

    /// Only the declared snake_case names are read. Writes deep-merge onto
    /// the stored object, so a camelCase key, once written, is never cleaned
    /// up: recognizing it would let a stale value shadow every later write of
    /// the declared name.
    #[test]
    fn ai_chat_ignores_camel_case_field_names() {
        let node = Node::new(
            "ai-chat-native".to_string(),
            "Chat".to_string(),
            serde_json::json!({
                "ai-chat-native": { "turnStatus": "processing" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["turnStatus"], "idle", "camelCase must not be read");
    }

    /// A message's text stays in `content`; its other fields are promoted and
    /// `properties` keeps extension fields only.
    #[test]
    fn ai_chat_message_promotes_its_fields_and_empties_properties() {
        let node = Node::new(
            "ai-chat-message".to_string(),
            "Which one?".to_string(),
            serde_json::json!({
                "ai-chat-message": {
                    "role": "assistant",
                    "timestamp": "2026-10-02T10:00:00Z",
                    "reasoning": "Two match.",
                    "outcome": "clarified",
                    "options": ["A", "B"],
                    "custom:flag": true
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "ai-chat-message");
        assert_eq!(out["content"], "Which one?");
        assert_eq!(out["role"], "assistant");
        assert_eq!(out["timestamp"], "2026-10-02T10:00:00Z");
        assert_eq!(out["reasoning"], "Two match.");
        assert_eq!(out["outcome"], "clarified");
        assert_eq!(out["options"], serde_json::json!(["A", "B"]));
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:flag": true })
        );
        assert_eq!(
            flat_properties_view(&out)["role"],
            serde_json::json!("assistant")
        );
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    /// A chat subtype's promoted fields are exactly its wire struct's own
    /// keys, so the flat property view can fold every one of them back.
    #[test]
    fn ai_chat_promoted_fields_are_the_wire_structs_keys() {
        let full_native = serde_json::json!({
            "agent": "nodespace", "model": "m", "summary": "s",
            "last_active": "2026-01-01T00:00:00Z", "provider": "native",
            "turn_status": "idle", "context_tokens": 1
        });
        let full_pty = serde_json::json!({
            "agent": "codex", "model": "m", "summary": "s",
            "last_active": "2026-01-01T00:00:00Z", "session_status": "active",
            "session_id": "s-1", "transcript": "t", "exit_code": 0
        });
        for (core, props) in [
            (CoreNodeType::AiChatNative, full_native),
            (CoreNodeType::AiChatPty, full_pty),
        ] {
            let node = Node::new(core.as_str().to_string(), "Chat".to_string(), props.clone());
            let out = node_to_typed_value(node).unwrap();
            for field in core_promoted_fields(core) {
                assert!(
                    out.get(field.wire).is_some(),
                    "{core}: {} is not on the wire",
                    field.wire
                );
                assert!(
                    props.get(field.storage).is_some(),
                    "{core}: {} is not stored",
                    field.storage
                );
            }
            assert_eq!(
                core_promoted_fields(core).len(),
                props.as_object().unwrap().len(),
                "{core}: a stored field is not promoted"
            );
            assert_eq!(out["properties"], serde_json::json!({}));
            // The flat view carries every field back under its storage key.
            assert_eq!(flat_properties_view(&out), props);
            // A subtype's list starts with the base's.
            let base = core_promoted_fields(CoreNodeType::AiChat);
            assert_eq!(&core_promoted_fields(core)[..base.len()], base);
        }
    }

    /// A schema node read as a plain node keeps the generic shape: the typed
    /// `SchemaNode` comes only from the store, which fills its relationships
    /// and parent from the declaration edges.
    #[test]
    fn schema_read_as_a_plain_node_keeps_the_generic_shape() {
        let node = Node::new(
            "schema".to_string(),
            "Task".to_string(),
            serde_json::json!({
                "isCore": true,
                "schemaVersion": 2,
                "fields": [],
                "children": { "rule": "none" },
                "parent": { "rule": "must_have_parent_of", "types": ["thread"] },
                "_seed": { "tier": "system" },
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "schema");
        assert_eq!(out["properties"]["isCore"], true);
        assert_eq!(out["properties"]["schemaVersion"], 2);
        // The stored definition comes back whole: a structural rule is an
        // object, and is not mistaken for another type's bucket.
        assert_eq!(
            out["properties"]["children"],
            serde_json::json!({ "rule": "none" })
        );
        assert_eq!(
            out["properties"]["parent"],
            serde_json::json!({ "rule": "must_have_parent_of", "types": ["thread"] })
        );
        assert!(out["properties"].get("_seed").is_none());
        assert!(out.get("isCore").is_none());
        assert!(out.get("relationships").is_none());
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }
}

/// Property tests for the Node → wire-JSON promotion contract.
///
/// The wire conversion is intentionally one-directional (a `Node` becomes a flat,
/// top-level JSON shape for the frontend), so these assert the property that
/// *matters* for silent data loss: **every field a typed node stores under its
/// `properties.<type>` namespace is promoted to a top-level key in the output**.
///
/// If someone adds a field to the stored shape but forgets to model it on the
/// wire struct (`TaskNode` / `AiChatNativeNode`), the promoted field vanishes from the
/// output and the corresponding proptest fails — turning a silent data-drop into
/// a test failure, which is the whole point of this guard.
#[cfg(test)]
mod promotion_proptests {
    use super::*;
    use crate::node::Node;
    use proptest::prelude::*;

    /// Arbitrary task `status` string (both core and user-defined values).
    fn task_status() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("open".to_string()),
            Just("in_progress".to_string()),
            Just("done".to_string()),
            Just("cancelled".to_string()),
            "[a-z][a-z_]{0,15}".prop_map(|s| s),
        ]
    }

    /// Arbitrary task `priority` string (core and user-defined values).
    fn task_priority() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("highest".to_string()),
            Just("high".to_string()),
            Just("medium".to_string()),
            Just("low".to_string()),
            Just("lowest".to_string()),
            "[a-z][a-z_]{0,15}".prop_map(|s| s),
        ]
    }

    /// Arbitrary date-only string (`YYYY-MM-DD`) that `normalize_date_field`
    /// passes through unchanged.
    fn date_string() -> impl Strategy<Value = String> {
        (2000u32..2100, 1u32..=12, 1u32..=28)
            .prop_map(|(y, m, d)| format!("{:04}-{:02}-{:02}", y, m, d))
    }

    proptest! {
        /// Every task field stored under `properties.task` is promoted to a
        /// top-level key, and the `nodespace://` uri is injected.
        #[test]
        fn task_promotes_all_stored_fields(
            status in task_status(),
            priority in task_priority(),
            due_date in date_string(),
            started_at in date_string(),
            completed_at in date_string(),
        ) {
            let node = Node::new(
                "task".to_string(),
                "Some task".to_string(),
                serde_json::json!({
                    "task": {
                        "status": status,
                        "priority": priority,
                        "due_date": due_date,
                        "started_at": started_at,
                        "completed_at": completed_at,
                    }
                }),
            );

            let out = node_to_typed_value(node).unwrap();

            // Each stored field promoted to the top level, verbatim.
            prop_assert_eq!(&out["status"], &serde_json::json!(status));
            prop_assert_eq!(&out["priority"], &serde_json::json!(priority));
            prop_assert_eq!(&out["dueDate"], &serde_json::json!(due_date));
            prop_assert_eq!(&out["startedAt"], &serde_json::json!(started_at));
            prop_assert_eq!(&out["completedAt"], &serde_json::json!(completed_at));
            // Namespace flattened away, uri injected.
            prop_assert!(out["properties"].get("task").is_none());
            prop_assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
        }

        /// Every field a native chat stores, in its own bucket or the inherited
        /// `ai-chat` one, is promoted to a top-level key, and the
        /// `nodespace://` uri is injected.
        #[test]
        fn ai_chat_native_promotes_all_stored_fields(
            processing in any::<bool>(),
            remote in any::<bool>(),
            model in "[a-zA-Z0-9._:-]{1,25}",
            summary in "[ -~]{1,40}",
            context_tokens in 0u64..1_000_000,
        ) {
            let turn_status = if processing { "processing" } else { "idle" };
            let provider = if remote { "openai-compat" } else { "native" };
            let node = Node::new(
                "ai-chat-native".to_string(),
                "A chat".to_string(),
                serde_json::json!({
                    "ai-chat": {
                        "agent": "nodespace",
                        "model": model,
                        "summary": summary,
                        "last_active": "2026-01-01T00:00:00Z",
                    },
                    "ai-chat-native": {
                        "turn_status": turn_status,
                        "provider": provider,
                        "context_tokens": context_tokens,
                    }
                }),
            );

            let out = node_to_typed_value(node).unwrap();

            prop_assert_eq!(&out["agent"], &serde_json::json!("nodespace"));
            prop_assert_eq!(&out["model"], &serde_json::json!(model));
            prop_assert_eq!(&out["summary"], &serde_json::json!(summary));
            prop_assert_eq!(&out["lastActive"], &serde_json::json!("2026-01-01T00:00:00Z"));
            prop_assert_eq!(&out["turnStatus"], &serde_json::json!(turn_status));
            prop_assert_eq!(&out["provider"], &serde_json::json!(provider));
            prop_assert_eq!(&out["contextTokens"], &serde_json::json!(context_tokens));
            prop_assert_eq!(&out["properties"], &serde_json::json!({}));
            prop_assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
        }

        /// Every field a terminal chat stores is promoted to a top-level key.
        #[test]
        fn ai_chat_pty_promotes_all_stored_fields(
            ended in any::<bool>(),
            agent in "[a-z][a-z-]{0,15}",
            session_id in "[a-f0-9-]{1,36}",
            transcript in "[ -~]{0,40}",
            exit_code in -1i64..256,
        ) {
            let session_status = if ended { "ended" } else { "active" };
            let node = Node::new(
                "ai-chat-pty".to_string(),
                "A session".to_string(),
                serde_json::json!({
                    "ai-chat": { "agent": agent },
                    "ai-chat-pty": {
                        "session_status": session_status,
                        "session_id": session_id,
                        "transcript": transcript,
                        "exit_code": exit_code,
                    }
                }),
            );

            let out = node_to_typed_value(node).unwrap();

            prop_assert_eq!(&out["agent"], &serde_json::json!(agent));
            prop_assert_eq!(&out["sessionStatus"], &serde_json::json!(session_status));
            prop_assert_eq!(&out["sessionId"], &serde_json::json!(session_id));
            prop_assert_eq!(&out["transcript"], &serde_json::json!(transcript));
            prop_assert_eq!(&out["exitCode"], &serde_json::json!(exit_code));
            prop_assert_eq!(&out["properties"], &serde_json::json!({}));
        }
    }
}
