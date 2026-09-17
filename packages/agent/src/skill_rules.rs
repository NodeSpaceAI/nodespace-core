//! Shared source of truth for core-type rules that must stay in sync between
//! [`crate::skill_pipeline::seed_skill_nodes`] (guidance for the in-app,
//! instance-aware local agent) and the generated sections of
//! `packages/skill/SKILL.md` (a static, instance-blind reference installed by
//! external PTY agents).
//!
//! Each rule here covers a constraint that is true regardless of which
//! surface acts on it — schema-authoring conventions, or interaction habits
//! like "search before acting on an ID". Instance-specific guidance (custom
//! schemas actually registered on a running daemon) and CLI-only reference
//! material (flags, `--database` selection, output shapes) do not belong
//! here — they have no analog on the other side and are not a drift risk.
//!
//! Two renderers read these rules: [`SchemaRule::imperative`] /
//! [`InteractionRule::imperative`] produce the terse, ALL-CAPS-header style
//! `seed_skill_nodes()` uses for LLM prompt content, and
//! [`SchemaRule::prose`] / [`InteractionRule::prose`] produce the
//! bold-lead-sentence markdown style the skill uses.
//! `packages/cli/examples/gen_skill_md.rs` renders the prose form into the
//! shipped skill content; a checked-in copy is verified against that output
//! so the file cannot silently go stale.

/// A schema-authoring convention (field naming, enums, relationships,
/// title templates, request-scoping).
pub struct SchemaRule {
    pub id: &'static str,
    /// Terse imperative form for the LLM prompt (seed_skill_nodes()).
    pub imperative: &'static str,
    /// Flowing markdown prose form for SKILL.md, including its own
    /// **Bold lead.** phrase.
    pub prose: &'static str,
}

/// A generic interaction habit repeated across multiple skills/CLI verbs
/// (find-then-act, ask-one-clarifying-question, success-means-stop).
pub struct InteractionRule {
    pub id: &'static str,
    pub imperative: &'static str,
    pub prose: &'static str,
    /// A short, distinctive substring that must appear somewhere in
    /// `packages/skill/SKILL.md` for this rule to be considered present.
    /// Unlike [`SchemaRule`]'s `prose`, `InteractionRule::prose` is woven
    /// mid-sentence into hand-written CLI prose rather than generated
    /// verbatim, so presence is checked via this shorter, drift-resistant
    /// phrase instead of an exact match on `prose` itself.
    pub skill_md_key_phrase: &'static str,
}

pub const ONE_SCHEMA_PER_REQUEST: SchemaRule = SchemaRule {
    id: "one-schema-per-request",
    imperative: "ONE SCHEMA PER REQUEST: Create exactly the type the user named, in a single create_schema call, then stop and report it. Do NOT proactively invent or create related types (e.g. asked for \"ADR\", do not also create \"Ticket\" or \"Sprint\"), and do NOT follow up with update_schema to wire relationships unless the user explicitly asked for them. A relationship's targetType must already exist in EXISTING SCHEMAS, or be the type you are creating in this call (self-reference); if it is neither, omit the relationship rather than creating the other type.",
    prose: "**One schema per request.** Create exactly the type asked for, in a single `schema create` call, then stop and report it. Don't proactively create related types the user didn't ask for (e.g. asked for \"ADR\" — don't also create \"Ticket\" or \"Sprint\"), and don't follow up with `schema update` to wire relationships unless explicitly asked. A relationship's `targetType` must already exist (check `nodespace schema list`) or be the type this call is creating; if it is neither, omit the relationship rather than creating the other type as a side effect.",
};

pub const SCHEMA_ALREADY_EXISTS: SchemaRule = SchemaRule {
    id: "schema-already-exists",
    imperative: "SUCCESS: After create_schema returns a schema object (with fields, type_id, etc.), the schema was created. Respond to the user immediately — do NOT call create_schema again. If create_schema returns an error saying the schema already exists, stop and tell the user the type already exists and they can create instances with create_node.",
    prose: "If `create` reports the schema already exists, stop and tell the user — they can create instances with `node create` against the existing type.",
};

pub const SCHEMA_VALIDATION_ERROR_RETRY: SchemaRule = SchemaRule {
    id: "schema-validation-error-retry",
    imperative: "VALIDATION ERROR: If create_schema returns an error other than \"already exists\" (e.g. a title_template placeholder missing from fields, an invalid field type), the error names the specific problem — fix exactly that and call create_schema again in this same turn with the corrected payload. Do NOT ask the user to clarify and do NOT give up after one rejection; a validation error is fixable from the error message alone.",
    prose: "If `create` rejects the schema with a validation error (not \"already exists\") — for example a `title_template` placeholder missing from `fields`, or an invalid field type — the error names the specific problem. Fix exactly that and retry immediately with the corrected payload; don't ask the user to clarify and don't give up after one rejection.",
};

pub const EDIT_DONT_RECREATE: SchemaRule = SchemaRule {
    id: "edit-dont-recreate",
    imperative: "EDITING A SCHEMA — call update_schema: When the user wants to add a field, remove a field, rename a field, add a value to an existing enum field, or change a relationship on an existing schema, call update_schema with the schema_id and only the fields that need changing. Do NOT re-create the whole schema. Use add_fields, remove_fields, rename_fields, add_field_values, or update the description/title_template as needed.",
    prose: "**Editing:** to add, remove, or rename a field, add a value to an existing enum field, or change a relationship on an existing schema, use `schema update` with only the fields that need changing (`add_fields`/`remove_fields`/`rename_fields`/`add_field_values`, or an updated `description`/`title_template`). Don't re-create the whole schema for a small change.",
};

pub const RENAME_VS_RELABEL: SchemaRule = SchemaRule {
    id: "rename-vs-relabel",
    // Mechanics (which 'from'/'to'/'friendlyName' shape does which thing)
    // are argument shape — the rename_fields tool-schema description
    // (local_agent/tools.rs) is the one place that spells them out per
    // ADR-064 rule 1. This rule states only the procedural judgment call a
    // schema description cannot: which of the two the user actually means.
    imperative: "RENAME VS RELABEL: rename_fields can rename a field's storage key OR relabel its display name (see the tool schema for the 'from'/'to'/'friendlyName' shape of each). A user asking to call a field something else on screen almost always means the display label, not a storage rename — do not conflate the two.",
    prose: "**Rename vs. relabel:** `rename_fields` can rename a field's storage key or relabel its display name only — see the tool schema for the `from`/`to`/`friendlyName` shape of each. A user asking to relabel what a field is called on screen almost always means the display label, not a storage rename.",
};

/// Adding a value to an existing enum is a different write path from adding a
/// field, and the two are easy to confuse: both are "add something to this
/// schema", and `add_fields` is the one an agent already knows about. Reaching
/// for `add_fields` here declares a redundant second field rather than
/// extending the vocabulary the user meant, and redeclaring the existing field
/// is rejected outright — so the rule leads with the discrimination (existing
/// field's vocabulary vs. new field declaration) rather than with the
/// mechanics.
///
/// The two forms diverge on how to establish eligibility, because the two
/// surfaces genuinely differ. `nodespace schema get` prints the schema node's
/// whole flattened property blob, `extensible` included, so the prose form
/// says to look first. The local agent has no equivalent: its only route to a
/// schema's fields is `get_node`'s `available_properties`, which
/// `build_available_properties` builds from name/type/set/allowed_values and
/// never carries `extensible`. So the imperative form tells the model to call
/// and read the rejection instead — naming a pre-check it cannot perform
/// would be worse than none, since a model that treats it as a precondition
/// declines the operation outright, which is exactly the "capability reads as
/// missing" failure this family of rules exists to prevent.
pub const ADD_ENUM_VALUES: SchemaRule = SchemaRule {
    id: "add-enum-values",
    imperative: "ADDING A VALUE TO AN EXISTING ENUM \u{2014} use add_field_values, NOT add_fields: When the user wants a new choice on a field that already exists (a \"backlog\" status on task, a new priority level), call update_schema with add_field_values: `{\"schema_id\": \"task\", \"add_field_values\": [{\"field\": \"status\", \"values\": [{\"value\": \"backlog\", \"label\": \"Backlog\"}]}]}`. This appends to the EXISTING field's vocabulary. Do NOT use add_fields \u{2014} that declares a NEW field, which is the wrong operation and leaves the original field unchanged. Do NOT try to redeclare the field with a fuller coreValues list either; that is rejected. ELIGIBILITY: only a field with extensible: true AND type enum can be extended. Do NOT try to pre-verify this before calling \u{2014} no tool on this surface reports a field's extensible flag (get_node's available_properties lists a field's type and allowed_values, not whether it is extensible). Just make the call: a rejection names the exact reason, and nothing is partially applied. New values land in user_values; core_values is never touched. COLLISIONS: the whole call is rejected if the field does not exist, is not extensible, is not an enum, or if any value string already exists in core_values or user_values \u{2014} nothing is merged or overwritten. The check is on the value string, never the label: two values may share a label, so a rejection naming a colliding value means choose a different value string, not a different label.",
    prose: "**Adding a value to an existing enum.** To give a field that already exists a new choice \u{2014} a `backlog` status on `task`, another priority level \u{2014} use `add_field_values`, not `add_fields`:\n\n```bash\nnodespace schema update --params '{\"schema_id\":\"task\",\"add_field_values\":[{\"field\":\"status\",\"values\":[{\"value\":\"backlog\",\"label\":\"Backlog\"}]}]}'\n```\n\n`add_fields` is the wrong tool here: it declares a *new* field and leaves the original one's vocabulary untouched. Redeclaring the existing field with a fuller `coreValues` list is rejected outright, so extending in place is the only route.\n\nOnly a field declared `extensible: true` **and** typed `enum` can be extended \u{2014} `nodespace schema get <schema_id>` shows both, so check before calling rather than discovering it through a rejection. Added values land in `user_values`; `core_values` is never written.\n\nThe operation is all-or-nothing: it is rejected if the field doesn't exist, isn't extensible, isn't an enum, or if any value string already exists on `core_values` or `user_values` \u{2014} nothing is merged or overwritten. Collision is checked on the `value` string and never on `label` (two values may legitimately share a label), so a rejection naming a colliding value means pick a different `value`, not a different `label`.",
};

/// The recognition trigger matters more than the conclusion here: an agent
/// that has just created a throwaway schema, or is asked to "remove"/"undo"/
/// "clean up" a type, is the one that needs this. Without it, `schema --help`
/// reads as a closed set of verbs the agent already knows, and it reports
/// deletion as unsupported rather than looking for it. The relationship
/// prerequisite is stated up front because discovering it only through the
/// runtime rejection reads as a second dead end.
pub const DELETE_A_SCHEMA: SchemaRule = SchemaRule {
    id: "delete-a-schema",
    imperative: "DELETING A SCHEMA \u{2014} the capability exists, use it: When the user asks to remove, drop, undo or clean up a node type \u{2014} including a throwaway type you created yourself this session \u{2014} delete it. Do NOT report deletion as unsupported and do NOT propose stripping the schema to an empty shell as a workaround. PREREQUISITE: a schema carrying relationship declarations cannot be deleted; call update_schema with remove_relationships first, on THIS type for the relationships it declares AND on every other type that declares a relationship targeting it, then delete the schema node. This is the mirror of the targetType rule: a relationship's target must EXIST to declare it, and must be ABSENT to delete the type it points at. The rejection names the remaining declaration count \u{2014} act on it rather than giving up. Only declarations BETWEEN SCHEMAS block the delete: edges between ordinary nodes are instance data and do not count, so do not go hunting for those. Deleting the type does NOT delete its existing instances \u{2014} they remain as nodes; delete those separately with delete_node if the user wants them gone too.",
    prose: "**Deleting a schema.** A node type can be removed \u{2014} `nodespace schema delete <schema_id>`. Reach for it whenever the user asks to remove, drop, undo or clean up a type, including a throwaway type created earlier in the session; never report deletion as unsupported, and never propose stripping a schema to an empty shell as a substitute.\n\nRelationship declarations are the one prerequisite: a schema that still declares relationships, or is still targeted by another type's declaration, is rejected with `schema_has_declarations` and the remaining count. Clear them with `schema update` first, then delete:\n\n```bash\n# 1. Drop the relationships this type declares\nnodespace schema update --params '{\"schema_id\":\"adr\",\"remove_relationships\":[\"decided_by\",\"supersedes\"]}'\n# 2. Drop declarations on OTHER types that target it (`schema list --json` shows them)\nnodespace schema update --params '{\"schema_id\":\"ticket\",\"remove_relationships\":[\"related_adr\"]}'\n# 3. Delete the schema\nnodespace schema delete adr\n```\n\nThis is the mirror of the `targetType` rule above: a relationship's target must **exist** before the relationship can be declared, and must be **absent** before the type it points at can be deleted.\n\nTwo scoping notes. Only declarations *between schemas* block the delete \u{2014} relationship edges between ordinary nodes are instance data and are not counted, so there is no need to unpick those first. And deleting the type does not delete its instances: they remain as nodes of that type, so remove them with `node delete` separately if the user wants them gone too.",
};

pub const NO_NAME_TITLE_FIELD: SchemaRule = SchemaRule {
    id: "no-name-title-field",
    imperative: "Do NOT add a 'name' or 'title' field — every node already has a built-in content/title field.",
    prose: "define only type-specific fields — don't add a `name` or `title` field; every node already has a built-in content/title field.",
};

/// Measured on its own (isolated daemon, live model) to have ZERO effect on
/// the #1846 contamination this rule targets — the rule was confirmed present
/// in the seeded prompt, yet the model still copied an unrelated schema's
/// fields verbatim. The actual fix is `EXISTING_SCHEMAS_HEADER`'s inline
/// anti-copy clause (`context_ops.rs`), which measured a real reduction (4/5
/// clean trials vs. 0/1 for this rule alone). Kept anyway as defense-in-depth
/// — cheap, doesn't conflict with anything, may matter for other models or
/// paths the live-daemon trials didn't cover — but it is not a component of
/// that measured 4/5 result, and should not be credited as one.
pub const FIELDS_FROM_REQUEST_ONLY: SchemaRule = SchemaRule {
    id: "fields-from-request-only",
    imperative: "FIELD SOURCE: derive every field from what the user's OWN request describes wanting to track — never from another schema shown in EXISTING SCHEMAS. That block lists types that already exist so you don't recreate them; it is not a shape to copy fields from for a new, different type. A new type about releases does not inherit fields from an unrelated ticket or adr schema just because one is listed there.",
    prose: "**Field source:** derive every field from what the user's own request describes wanting to track — never from another schema shown in the entity-types context. That listing exists so you don't recreate a type that already exists; it is not a shape to copy fields from for a new, unrelated type.",
};

pub const NAME_PLACEHOLDER_EXCEPTION: SchemaRule = SchemaRule {
    id: "name-placeholder-exception",
    imperative: "EXCEPTION: if you use a 'name' placeholder in title_template (e.g. \"{name} ({status})\"), you MUST define 'name' as a text field so title generation works.",
    prose: "Exception: if `title_template` uses a `{name}` placeholder, `name` must be defined as a field (any placeholder in `title_template` must have a matching field).",
};

pub const ENUM_FORMAT: SchemaRule = SchemaRule {
    id: "enum-format",
    imperative: "ENUMS: Use lowercase values with readable labels, e.g. `{\"value\": \"in_progress\", \"label\": \"In Progress\"}`.",
    prose: "**Enums:** lowercase values with readable labels — `{\"value\":\"in_progress\",\"label\":\"In Progress\"}`.",
};

pub const RELATIONSHIP_VS_FIELD: SchemaRule = SchemaRule {
    id: "relationship-vs-field",
    imperative: "RELATIONSHIPS: Use relationships (not fields) when a field references another node type. RECOGNITION IS THE HARD PART: a field naming a person, team, project, or any other entity is a reference even when it reads naturally as text — a plain string has no integrity (\"M. Alibio\" and \"m alibio\" are different values to a query engine), no reverse lookup, and no rename path (renaming a person means rewriting every node that names them). Examples, from how a request is phrased: \"who signed off\" -> decided_by (targetType: person), not a deciders: array field. \"who it's assigned to\" -> assignee (targetType: person), not an assignee: text field. \"which project it affects\" -> affects_project (targetType: project). FALSE FRIENDS — field names that read as plain attributes but are usually references: deciders, assignee, owner, author, reviewer, reported_by, members. Before defaulting one of these to a text field, check whether the target type already exists in EXISTING SCHEMAS. ESCAPE HATCH: free text is fine for a one-off external party who will never be a node in this graph — use a relationship when the party is, or could become, a first-class entity here.",
    prose: "**Relationships vs. fields:** use a relationship (not a field) when a value references another node type.",
};

/// The write-cost argument against collections was, in practice, the argument
/// for a `tags: []` field: an agent priced a collection at a lookup, a create
/// and a link per node, against one property write for an array element. A
/// collection path is now a single argument on the create call, so the two
/// cost the same and only the durable differences remain.
pub const GROUPING_IS_COLLECTIONS: SchemaRule = SchemaRule {
    id: "grouping-is-collections",
    imperative: "GROUPING FIELDS: Do NOT declare a tags, categories, topics, labels, areas or groups field. NodeSpace already has one mechanism for tagging and grouping — collections — and a flat label and a nested path (\"docs:rust\") are that same mechanism at two depths. COST: a collection is not more expensive to write than an array element. Both are one argument on the create call: node create --collection docs:rust, repeatable for several collections, with every missing path segment created for you and no lookup first. So there is no cheap-versus-thorough tradeoff to weigh. What differs is durability: an array value renders in no UI, must be edited on every member to rename, cannot nest, and is invisible to collection queries; a collection does all four. member_of is structural, so joining one needs no schema change and no relationship declaration. ESCAPE HATCH: if the user explicitly asks for a plain tags field, give them one without arguing.",
    prose: "**Grouping is collections, not an array field.** Don't declare a `tags`, `categories`, `topics`, `labels`, `areas` or `groups` field — collections already are the tagging and grouping mechanism, with a flat label and a nested path (`docs:rust`) as the same mechanism at two depths. They also cost the same to write: `node create --collection docs:rust` is one argument, repeatable, with missing path segments created for you and no lookup first — exactly the cost of setting one array element. What differs is what you get. An array value renders in no UI, has to be edited on every member to rename, cannot nest, and is invisible to collection queries; a collection does all four, and `member_of` is structural so joining one needs no schema change. If the user explicitly asks for a plain tags field, give them one without arguing.",
};

pub const TARGET_TYPE_MUST_EXIST: SchemaRule = SchemaRule {
    id: "target-type-must-exist",
    imperative: "The targetType MUST be an existing schema ID from the EXISTING SCHEMAS list in the system prompt, or the schema ID of the type you are creating in this same call — do NOT invent types that aren't listed. If the target type doesn't exist yet, omit the relationship entirely. reverseName and reverseCardinality are REQUIRED on every relationship — a declaration missing either is rejected. One edge is stored and read from BOTH ends, so name it from both: reverseName is what the edge is called read from the target (plural where that end may hold many — \"invoices\", not \"Invoice (Customer)\"), and reverseCardinality is \"one\" or \"many\", saying how many sources may point at one target. Examples:\n- ADR supersedes adr (one), read back as superseded_by: `{\"name\": \"supersedes\", \"targetType\": \"adr\", \"direction\": \"out\", \"cardinality\": \"one\", \"reverseName\": \"superseded_by\", \"reverseCardinality\": \"one\"}`\n- Ticket has_task task (many), read back as ticket: `{\"name\": \"has_task\", \"targetType\": \"task\", \"direction\": \"out\", \"cardinality\": \"many\", \"reverseName\": \"ticket\", \"reverseCardinality\": \"one\"}`\n- ADR decided_by person, readable back as the person's decisions: `{\"name\": \"decided_by\", \"targetType\": \"person\", \"direction\": \"out\", \"cardinality\": \"one\", \"reverseName\": \"decisions\", \"reverseCardinality\": \"many\"}`\n\nSELF-REFERENCE: a type may point at itself in the same schema create call — use its own schema ID (the snake_case form of the name), no second call needed. The required reverseName is what names the other direction, so never declare a second relationship for it: one stored edge, readable from both ends. Example, on `schema create` for ADR: `{\"name\": \"supersedes\", \"targetType\": \"adr\", \"direction\": \"out\", \"cardinality\": \"one\", \"reverseName\": \"superseded_by\", \"reverseCardinality\": \"one\"}`. Same for `blocks`/`blocked_by` on a task or `parent`/`child` on a category.",
    prose: "`targetType` must be an existing schema ID, or the schema ID of the type being created in the same call. `reverseName` and `reverseCardinality` are **required** on every relationship — a declaration missing either is rejected. One edge is stored and read from both ends, so name it from both: `reverseName` is what the edge is called read from the target (plural where that end may hold many — `invoices`, not `Invoice (Customer)`), and `reverseCardinality` is `one` or `many`, saying how many sources may point at one target. Examples: `{\"name\":\"supersedes\",\"targetType\":\"adr\",\"direction\":\"out\",\"cardinality\":\"one\",\"reverseName\":\"superseded_by\",\"reverseCardinality\":\"one\"}`, `{\"name\":\"has_task\",\"targetType\":\"task\",\"direction\":\"out\",\"cardinality\":\"many\",\"reverseName\":\"ticket\",\"reverseCardinality\":\"one\"}`, `{\"name\":\"decided_by\",\"targetType\":\"person\",\"direction\":\"out\",\"cardinality\":\"one\",\"reverseName\":\"decisions\",\"reverseCardinality\":\"many\"}`.\n\n**Self-referential relationships:** a type may point at itself in the same `schema create` call — give its own schema ID (the snake_case form of the name); no follow-up `schema update` is needed. The required `reverseName` is what names the other direction, so never declare a second relationship for it — one stored edge, readable from both ends: `{\"name\":\"supersedes\",\"targetType\":\"adr\",\"direction\":\"out\",\"cardinality\":\"one\",\"reverseName\":\"superseded_by\",\"reverseCardinality\":\"one\"}`. The same shape covers `blocks`/`blocked_by` on a task and `parent`/`child` on a category.",
};

pub const ENUM_EDGE_FIELDS: SchemaRule = SchemaRule {
    id: "enum-edge-fields",
    imperative: "EDGE FIELDS: A relationship may carry attributes on the edge itself via edgeFields — use them for facts about the CONNECTION rather than about either node (an access level on a membership, a billing date on an invoice link). When an edge field has a fixed vocabulary, declare it as an enum with coreValues, exactly like a node field: `{\"name\": \"access\", \"type\": \"enum\", \"coreValues\": [{\"value\": \"owner\", \"label\": \"Owner\"}, {\"value\": \"editor\", \"label\": \"Editor\"}, {\"value\": \"viewer\", \"label\": \"Viewer\"}]}`. RULES: coreValues is REQUIRED on an enum edge field and REJECTED on any other type; a default MUST be one of the declared values; values must be unique. Edge enums are closed — there is no userValues or extensible on an edge field. Creating or editing an edge validates the value against the declared set, so an undeclared value is rejected rather than stored. LIMITS: only relationships YOU declare can carry edgeFields — the built-in structural names (member_of, has_child, mentions, has_role) are reserved and rejected as declarations, so you cannot attach an edge field to them. `required` and `default` on an edge field are recorded but NOT enforced when an edge is written: an omitted enum key is stored absent, not filled in from default. Do not rely on a default to supply a value.",
    prose: "**Edge fields.** A relationship can carry attributes on the edge itself via `edgeFields` — facts about the *connection*, not about either node (an access level on a membership, a billing date on an invoice link). Give an edge field a fixed vocabulary by declaring it as an enum with `coreValues`, the same shape a node field uses:\n\n```json\n{\"name\": \"access\", \"type\": \"enum\",\n \"coreValues\": [{\"value\": \"owner\", \"label\": \"Owner\"},\n                {\"value\": \"editor\", \"label\": \"Editor\"},\n                {\"value\": \"viewer\", \"label\": \"Viewer\"}]}\n```\n\n`coreValues` is required on an enum edge field and rejected on any other type; a `default` must be one of the declared values; values must be unique. Edge enums are closed — no `userValues`/`extensible` half. Creating or editing an edge validates the value against the declared set (including via `--edge-data`), and the relationships UI renders a picker instead of a free-text box.\n\nTwo limits worth knowing. Only relationships you declare can carry `edgeFields`: the built-in structural names (`member_of`, `has_child`, `mentions`, `has_role`) are reserved and rejected as declarations, so an edge field cannot be attached to them. And `required`/`default` on an edge field are recorded but not enforced at write time — an omitted enum key is stored absent rather than filled in from `default`, so don't rely on a default to supply a value.",
};

pub const TITLE_TEMPLATE_PLACEHOLDERS: SchemaRule = SchemaRule {
    id: "title-template-placeholders",
    imperative: "TITLE TEMPLATE: Set title_template when a node's identity comes from its fields rather than free-form content. Use {field_name} placeholders. CRITICAL: every placeholder in title_template MUST be defined as a field in the fields array. Omit title_template if the content/title field alone identifies the node.",
    prose: "**Title template:** set `title_template` when a node's identity comes from its fields rather than free-form content, using `{field_name}` placeholders — every placeholder must be a defined field. Omit it if the content/title field alone identifies the node.",
};

pub const UNIQUE_FIELD_FLAGS: SchemaRule = SchemaRule {
    id: "unique-field-flags",
    imperative: "UNIQUE FIELDS: Set \"unique\": true on a field when the user's request implies each instance should have a distinct value for it (e.g. \"each ticket should have a unique key\" -> flag key unique). Use \"unique_case_insensitive\": true instead of \"unique\" when case shouldn't matter (e.g. email, username). ADVISORY ONLY: this does NOT prevent duplicates from being created — it only lets the system suggest a likely existing match (e.g. surface the existing node) when a new value collides. Never tell the user a unique flag will block or reject a duplicate; describe it as a duplicate warning/suggestion, not an enforced constraint. Example: {\"name\": \"key\", \"type\": \"text\", \"unique_case_insensitive\": true}.",
    prose: "**Unique fields:** set `\"unique\": true` on a field when the user's request implies each instance should have a distinct value for it (e.g. \"each ticket should have a unique key\" → flag `key` unique). Use `\"unique_case_insensitive\": true` instead when case shouldn't matter — email and username are the common case. This is advisory only: it does not prevent duplicates from being created, it only lets the system surface a likely existing match when a new value collides. Never describe it to the user as blocking or rejecting duplicates — it's a suggestion, not an enforced constraint. Example: `{\"name\":\"key\",\"type\":\"text\",\"unique_case_insensitive\":true}`.",
};

/// All schema-authoring rules, in the order they should be rendered.
pub const SCHEMA_RULES: &[SchemaRule] = &[
    ONE_SCHEMA_PER_REQUEST,
    SCHEMA_ALREADY_EXISTS,
    SCHEMA_VALIDATION_ERROR_RETRY,
    EDIT_DONT_RECREATE,
    ADD_ENUM_VALUES,
    RENAME_VS_RELABEL,
    DELETE_A_SCHEMA,
    NO_NAME_TITLE_FIELD,
    FIELDS_FROM_REQUEST_ONLY,
    NAME_PLACEHOLDER_EXCEPTION,
    ENUM_FORMAT,
    RELATIONSHIP_VS_FIELD,
    GROUPING_IS_COLLECTIONS,
    TARGET_TYPE_MUST_EXIST,
    ENUM_EDGE_FIELDS,
    TITLE_TEMPLATE_PLACEHOLDERS,
    UNIQUE_FIELD_FLAGS,
];

pub const FIND_THEN_ACT: InteractionRule = InteractionRule {
    id: "find-then-act",
    imperative: "If you don't have the node's ID, call search_semantic or search_nodes first to locate it. Then act on the resolved ID — do not guess IDs.",
    prose: "if you don't already have the target node's ID, search for it first, then act on the resolved ID.",
    skill_md_key_phrase: "if you don't have its ID",
};

pub const AMBIGUITY_CLARIFY: InteractionRule = InteractionRule {
    id: "ambiguity-clarify",
    // Retargeted from prose ("ask one specific clarifying question") to
    // calling route_clarify: the golden corpus measured calling the tool as
    // more reliable than answering in prose, since prose is invisible to
    // anything downstream that expects a tool call. `prose` and
    // `skill_md_key_phrase` below are unchanged — they serve
    // packages/skill/SKILL.md, a separate external-agent-facing document
    // with no route_clarify tool of its own to point at.
    imperative: "AMBIGUITY: If search returns 0 results or multiple results that don't clearly match what the user described, call route_clarify with one specific question and concrete options rather than retrying or answering in prose.",
    prose: "if the search comes back with zero matches or several equally plausible matches, ask the user one specific clarifying question rather than retrying.",
    skill_md_key_phrase: "ask the user one specific clarifying question rather than retrying",
};

pub const SUCCESS_NO_REVERIFY: InteractionRule = InteractionRule {
    id: "success-no-reverify",
    imperative: "SUCCESS: After the mutating call returns, confirm the change to the user. Do NOT re-fetch or re-search to confirm — the response itself is the confirmation.",
    prose: "confirm the change to the user from the response — don't re-fetch or re-search afterward just to double-check it landed.",
    skill_md_key_phrase: "don't re-fetch",
};

/// The dedicated-verb instruction is a stable API constraint and is stated
/// outright. The *value list* deliberately is not: `task.status` is declared
/// `extensible` and ADR-076's `add_field_values` lets a methodology bundle
/// append real values to it, so any list written here is only correct until
/// the first such install. The guidance points at `update_task_status`'s own
/// `status` enum instead, which `tools::with_live_task_statuses` rewrites
/// from the stored schema each turn.
///
/// It cannot point at the `EXISTING SCHEMAS` block: that block excludes core
/// types by construction (`context_ops::parse_and_filter_non_core_schemas`
/// filters on `!is_core`, pinned by
/// `parse_and_filter_non_core_schemas_excludes_core_types`), because presence
/// there is what tells the model a type is user-defined and takes bare
/// property keys. `task` is a core type, so `task.status` never appears in it.
pub const TASK_STATUS_DEDICATED_VERB: InteractionRule = InteractionRule {
    id: "task-status-dedicated-verb",
    imperative: "TASK STATUS: To change a task's status, call update_task_status with the task ID and the new status string. Use one of the values listed on update_task_status's own status parameter — that list is the task type's current vocabulary and can be longer than the built-in four. Do NOT use update_node for task status changes.",
    prose: "task status changes must go through the dedicated status-update verb, not a generic property update.",
    skill_md_key_phrase: "for task status changes",
};

pub const SINGLE_ITEM_PER_CALL: InteractionRule = InteractionRule {
    id: "single-item-per-call",
    imperative: "SINGLE DELETE: Call delete_node once per node. Confirm each deletion before proceeding to the next.",
    prose: "act on one node per call; confirm each individually before moving to the next.",
    skill_md_key_phrase: "Delete one node per call; confirm each deletion before moving to the next",
};

/// Collection assignment is a create-time argument, not a follow-up write.
/// The path resolves and auto-creates every missing segment in one call, so
/// the lookup-create-link sequence agents reach for by default is pure cost —
/// and so is the `tags: []` array they choose instead when they price that
/// sequence into the decision.
pub const COLLECTION_AT_CREATE_TIME: InteractionRule = InteractionRule {
    id: "collection-at-create-time",
    imperative: "COLLECTIONS: Pass the collection when you create the node — create_node takes a collection path directly. Never look the collection up first, and never ask the user to pre-create it: a path is resolved in one call and every missing segment is created for you, including nested ones (\"docs:rust\" creates both `docs` and `rust`). The parameter is repeatable, so one call can file a node under several collections. Because of that, a collection costs no more to write than one element of a tags/categories/topics array — and unlike an array it is visible in the UI, renamed once instead of per member, and needs no schema change to join. Prefer a collection for any durable grouping.",
    prose: "collection membership is an argument to the create call, not a follow-up write — pass `--collection <path>` and every missing segment of the path is created for you.",
    skill_md_key_phrase: "collection membership is an argument to the create call",
};

/// A relationship is declared once, on the source type, but is legitimately
/// read from both ends. Whichever end you start from, the traversal exists —
/// the only question is which name spells it. Getting that wrong used to return
/// an empty result, which reads as "there is nothing there" and talks callers
/// out of the reverse query entirely; it now errors and names the working
/// spellings, but the mapping is cheap to state up front.
pub const RELATIONSHIP_REVERSE_TRAVERSAL: InteractionRule = InteractionRule {
    id: "relationship-reverse-traversal",
    imperative: "REVERSE TRAVERSAL: A relationship declared as {\"name\": \"decided_by\", \"targetType\": \"person\", \"direction\": \"out\", \"cardinality\": \"one\", \"reverseName\": \"decisions\", \"reverseCardinality\": \"many\"} on the adr schema is readable from BOTH ends. From an adr, call get_related_nodes with relationship_type 'decided_by' and direction 'out'. From the person, use the declared reverseName — relationship_type 'decisions' — or equivalently 'decided_by' with direction 'in'; both return the same ADRs. An empty result means no edges exist, NOT that the reverse direction is unsupported. A relationship name that is declared in neither direction is now an error naming the spellings that do work — read it and retry rather than reporting the capability as missing.",
    prose: "**Traversing the reverse direction.** A relationship is declared once, on the source type, but reads from both ends. Given `{\"name\":\"decided_by\",\"targetType\":\"person\",\"direction\":\"out\",\"cardinality\":\"one\",\"reverseName\":\"decisions\",\"reverseCardinality\":\"many\"}` on `adr`: from the ADR, `nodespace relationship get <adr-id> --type decided_by --direction out`; from the person, use the declared `reverseName` — `nodespace relationship get <person-id> --type decisions` — or the equivalent `--type decided_by --direction in`. Both spellings return the same ADRs, and the output line's arrow shows the direction actually traversed. An empty result means no edges exist, not that reverse traversal is unsupported. A name declared in neither direction is rejected with an error naming the spellings that do work — read it and retry rather than concluding the capability is missing.",
    skill_md_key_phrase: "use the declared `reverseName`",
};

pub const BULK_IMPORT_NO_FOLLOWUP_SEARCH: InteractionRule = InteractionRule {
    id: "bulk-import-no-followup-search",
    imperative: "SUCCESS: After create_nodes_from_markdown returns, report the number of nodes created. Do NOT follow up with search calls.",
    prose: "report the number of nodes created; don't follow up with search calls to verify.",
    skill_md_key_phrase: "don't follow up with search calls to verify",
};

/// All generic interaction-pattern rules, in no particular required order —
/// each is consumed independently by whichever skill/CLI section needs it.
pub const INTERACTION_RULES: &[InteractionRule] = &[
    FIND_THEN_ACT,
    AMBIGUITY_CLARIFY,
    SUCCESS_NO_REVERIFY,
    TASK_STATUS_DEDICATED_VERB,
    SINGLE_ITEM_PER_CALL,
    COLLECTION_AT_CREATE_TIME,
    RELATIONSHIP_REVERSE_TRAVERSAL,
    BULK_IMPORT_NO_FOLLOWUP_SEARCH,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_schema_rule_ids_are_unique() {
        let mut ids: Vec<&str> = SCHEMA_RULES.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SCHEMA_RULES.len(), "duplicate SchemaRule id");
    }

    #[test]
    fn all_interaction_rule_ids_are_unique() {
        let mut ids: Vec<&str> = INTERACTION_RULES.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            INTERACTION_RULES.len(),
            "duplicate InteractionRule id"
        );
    }

    #[test]
    fn no_rule_text_is_empty() {
        for r in SCHEMA_RULES {
            assert!(!r.imperative.is_empty(), "{} imperative is empty", r.id);
            assert!(!r.prose.is_empty(), "{} prose is empty", r.id);
        }
        for r in INTERACTION_RULES {
            assert!(!r.imperative.is_empty(), "{} imperative is empty", r.id);
            assert!(!r.prose.is_empty(), "{} prose is empty", r.id);
            assert!(
                !r.skill_md_key_phrase.is_empty(),
                "{} skill_md_key_phrase is empty",
                r.id
            );
        }
    }

    /// Interaction rules are woven mid-sentence into hand-written SKILL.md
    /// prose (unlike SchemaRule, which is regenerated verbatim by
    /// packages/cli/examples/gen_skill_md.rs — see its own staleness test). This
    /// only catches outright removal of a rule's substance; it does not
    /// guarantee SKILL.md's wording matches `prose` exactly.
    #[test]
    fn skill_md_still_mentions_every_interaction_rule() {
        // Read the whole shipped skill, not `SKILL.md` alone. The body is kept
        // within the Agent Skills size recommendation by moving the CLI
        // reference into `references/`, which the standard defines as the
        // on-demand tier. Guidance that moved there is still shipped and still
        // reachable by an agent, so scanning only the body would report drift
        // for content that simply changed tier.
        let skill_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skill");

        let read = |p: &std::path::Path| -> String {
            std::fs::read_to_string(p)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", p.display()))
        };

        let mut skill_md = read(&skill_dir.join("SKILL.md"));
        let refs_dir = skill_dir.join("references");
        let entries = std::fs::read_dir(&refs_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", refs_dir.display()));
        for entry in entries {
            let path = entry.expect("bad dir entry").path();
            if path.extension().is_some_and(|x| x == "md") {
                skill_md.push('\n');
                skill_md.push_str(&read(&path));
            }
        }

        for r in INTERACTION_RULES {
            assert!(
                skill_md.contains(r.skill_md_key_phrase),
                "the shipped skill (packages/skill/SKILL.md + references/) no \
                 longer mentions the '{}' rule (expected to find the phrase \
                 {:?}) — if this rule's guidance moved or was reworded, update \
                 skill_md_key_phrase in skill_rules.rs to match; if the rule's \
                 substance was removed, that's the drift this test exists to catch",
                r.id,
                r.skill_md_key_phrase
            );
        }
    }
}
