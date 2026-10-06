//! The rules the seeded skills ([`crate::skill_pipeline::SKILL_SEEDS`]) are
//! built from, each stated once per audience.
//!
//! A seeded skill is read by two audiences. The in-app local agent acts
//! through its tools (`create_node`, `search_nodes`). An agent outside the
//! app (Claude Code, Codex) fetches the same skill with `nodespace skill
//! guidance` and acts through the CLI. A skill's body states its procedure in
//! words both can follow, and includes a rule wherever the step has to name
//! how it is done.
//!
//! # Rules are Markdown fragments
//!
//! A rule's text lives in `seeds/rules/`, one `.md` fragment per form:
//!
//! - `seeds/rules/agent/<id>.md` — the form the local agent reads, naming its
//!   tools ([`SchemaRule::imperative`], [`InteractionRule::imperative`],
//!   [`ProcedureRule::imperative`]).
//! - `seeds/rules/skill-md/<id>.md` — the form an agent outside the app
//!   reads, naming CLI commands ([`SchemaRule::prose`],
//!   [`InteractionRule::prose`], [`ProcedureRule::prose`]).
//!
//! A seed body or a generated region includes a rule by id, with an
//! `<!-- include: <id> -->` marker that [`resolve_includes`] replaces with the
//! fragment for the audience being served. Nothing restates a rule's text: a
//! skill body, a guidance section and the shipped command reference all read
//! the same files, and `packages/cli/examples/gen_skill_md.rs` renders
//! [`skill_md_schema_rules`] into `packages/skill/references/cli.md`, where a
//! checked-in copy is verified against that output so the file cannot
//! silently go stale.
//!
//! There are three kinds. A [`SchemaRule`] is a schema- or play-authoring
//! convention. An [`InteractionRule`] is a habit several skills share. A
//! [`ProcedureRule`] is a step of one skill's own procedure.
//!
//! The two forms of a [`SchemaRule`] are still two hand-written fragments —
//! they differ on purpose, since each names its own surface's verbs
//! (`update_schema` vs. `schema update`) and some rules genuinely diverge
//! (see [`ADD_ENUM_VALUES`]). What ties them is [`SchemaRule::anchors`] plus
//! a JSON-example check: a wording fix that drops a fact or changes an
//! example in one form fails the tests until the other form follows.

/// A rule's local-agent form: its fragment under `seeds/rules/agent/`.
macro_rules! agent_form {
    ($id:literal) => {
        include_str!(concat!("seeds/rules/agent/", $id, ".md")).trim_ascii_end()
    };
}

/// A rule's shipped-skill form: its fragment under `seeds/rules/skill-md/`.
macro_rules! skill_md_form {
    ($id:literal) => {
        include_str!(concat!("seeds/rules/skill-md/", $id, ".md")).trim_ascii_end()
    };
}

/// A schema-authoring convention (field naming, enums, relationships,
/// title templates, request-scoping), or a play-authoring one
/// ([`PLAY_RULES`]): a rule each surface states in its own words, held
/// together by its anchors and its JSON examples.
pub struct SchemaRule {
    /// Names the rule's fragments, and is what an include marker cites.
    pub id: &'static str,
    /// Terse imperative form for the LLM prompt (the seeded skills).
    pub imperative: &'static str,
    /// Flowing markdown prose form for SKILL.md, including its own
    /// **Bold lead.** phrase.
    pub prose: &'static str,
    /// The rule's load-bearing facts — the names, values and phrases both
    /// forms must carry. Each must appear, case-insensitively, in both
    /// `imperative` and `prose`. When you reword one form, a missing anchor
    /// means the other form (or this list, if the fact itself changed) needs
    /// the same edit.
    pub anchors: &'static [&'static str],
}

/// A generic interaction habit repeated across several skills
/// (find-then-act, ask-one-clarifying-question, success-means-stop).
///
/// Both forms reach their audience through the skills that include the rule:
/// the local agent in a skill's stored body, an agent outside the app in the
/// same skill fetched with `nodespace skill guidance`.
pub struct InteractionRule {
    pub id: &'static str,
    pub imperative: &'static str,
    pub prose: &'static str,
}

/// A step of one skill's own procedure, where the step has to name how it is
/// done: the tool the local agent calls (`imperative`), or the command an
/// agent outside the app runs (`prose`).
///
/// The two forms are written separately because the two surfaces differ in
/// more than names. A tool takes its values as arguments and the CLI takes
/// them as flags; `delete_node` is confirmed by the app, and `node delete`
/// previews first and is confirmed by the agent. What a skill body says
/// outside its includes is common to both.
pub struct ProcedureRule {
    pub id: &'static str,
    pub imperative: &'static str,
    pub prose: &'static str,
}

/// The [`ProcedureRule`] whose two fragments are named `$id`.
macro_rules! procedure_rule {
    ($id:literal) => {
        ProcedureRule {
            id: $id,
            imperative: agent_form!($id),
            prose: skill_md_form!($id),
        }
    };
}

pub const ONE_SCHEMA_PER_REQUEST: SchemaRule = SchemaRule {
    id: "one-schema-per-request",
    imperative: agent_form!("one-schema-per-request"),
    prose: skill_md_form!("one-schema-per-request"),
    anchors: &[
        "no more",
        "ADR",
        "Ticket",
        "Sprint",
        "restraint",
        "create all of them",
    ],
};

pub const CREATING_TWO_LINKED_TYPES: SchemaRule = SchemaRule {
    id: "creating-two-linked-types",
    imperative: agent_form!("creating-two-linked-types"),
    prose: skill_md_form!("creating-two-linked-types"),
    anchors: &[
        "Customer and Invoice",
        "not one",
        "already exist",
        "referencing",
        "invoices",
        "one stored edge, readable from both ends",
        "two unlinked types",
    ],
};

pub const SCHEMA_ALREADY_EXISTS: SchemaRule = SchemaRule {
    id: "schema-already-exists",
    imperative: agent_form!("schema-already-exists"),
    prose: skill_md_form!("schema-already-exists"),
    anchors: &[
        "already exists",
        "stop and tell the user",
        "create instances",
    ],
};

pub const SCHEMA_VALIDATION_ERROR_RETRY: SchemaRule = SchemaRule {
    id: "schema-validation-error-retry",
    imperative: agent_form!("schema-validation-error-retry"),
    prose: skill_md_form!("schema-validation-error-retry"),
    anchors: &[
        "already exists",
        "placeholder missing from",
        "invalid field type",
        "names the specific problem",
        "corrected payload",
        "give up after one rejection",
    ],
};

pub const EDIT_DONT_RECREATE: SchemaRule = SchemaRule {
    id: "edit-dont-recreate",
    imperative: agent_form!("edit-dont-recreate"),
    prose: skill_md_form!("edit-dont-recreate"),
    anchors: &[
        "add a value to an existing enum field",
        "change a relationship",
        "add_fields",
        "remove_fields",
        "rename_fields",
        "add_field_values",
        "title_template",
        "re-create the whole schema",
    ],
};

pub const RENAME_VS_RELABEL: SchemaRule = SchemaRule {
    id: "rename-vs-relabel",
    // Mechanics (which 'from'/'to'/'friendlyName' shape does which thing)
    // are argument shape — the rename_fields tool-schema description
    // (local_agent/tools.rs) is the one place that spells them out per
    // ADR-064 rule 1. This rule states only the procedural judgment call a
    // schema description cannot: which of the two the user actually means.
    imperative: agent_form!("rename-vs-relabel"),
    prose: skill_md_form!("rename-vs-relabel"),
    anchors: &[
        "rename_fields",
        "storage key",
        "display name",
        "friendlyName",
        "display label, not a storage rename",
    ],
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
    imperative: agent_form!("add-enum-values"),
    prose: skill_md_form!("add-enum-values"),
    anchors: &[
        "add_field_values",
        "add_fields",
        "backlog",
        "coreValues",
        "extensible: true",
        "enum",
        "user_values",
        "core_values",
        "nothing is merged or overwritten",
        "label",
    ],
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
    imperative: agent_form!("delete-a-schema"),
    prose: skill_md_form!("delete-a-schema"),
    anchors: &[
        "remove, drop, undo or clean up",
        "unsupported",
        "empty shell",
        "remove_relationships",
        "mirror of the",
        "instance data",
        "extends",
        "re-target",
        "instances",
    ],
};

pub const NO_NAME_TITLE_FIELD: SchemaRule = SchemaRule {
    id: "no-name-title-field",
    imperative: agent_form!("no-name-title-field"),
    prose: skill_md_form!("no-name-title-field"),
    anchors: &["built-in content/title field"],
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
    imperative: agent_form!("fields-from-request-only"),
    prose: skill_md_form!("fields-from-request-only"),
    anchors: &[
        "own request describes wanting to track",
        "so you don't recreate",
        "not a shape to copy fields from",
    ],
};

pub const NAME_PLACEHOLDER_EXCEPTION: SchemaRule = SchemaRule {
    id: "name-placeholder-exception",
    imperative: agent_form!("name-placeholder-exception"),
    prose: skill_md_form!("name-placeholder-exception"),
    anchors: &["title_template", "{name}"],
};

pub const ENUM_FORMAT: SchemaRule = SchemaRule {
    id: "enum-format",
    imperative: agent_form!("enum-format"),
    prose: skill_md_form!("enum-format"),
    anchors: &["lowercase values with readable labels"],
};

pub const RELATIONSHIP_VS_FIELD: SchemaRule = SchemaRule {
    id: "relationship-vs-field",
    imperative: agent_form!("relationship-vs-field"),
    prose: skill_md_form!("relationship-vs-field"),
    anchors: &["references another node type"],
};

/// The write-cost argument against collections was, in practice, the argument
/// for a `tags: []` field: an agent priced a collection at a lookup, a create
/// and a link per node, against one property write for an array element. A
/// collection path is now a single argument on the create call, so the two
/// cost the same and only the durable differences remain.
pub const GROUPING_IS_COLLECTIONS: SchemaRule = SchemaRule {
    id: "grouping-is-collections",
    imperative: agent_form!("grouping-is-collections"),
    prose: skill_md_form!("grouping-is-collections"),
    anchors: &[
        "tags",
        "categories",
        "topics",
        "labels",
        "areas",
        "groups",
        "two depths",
        "node create --collection docs:rust",
        "repeatable",
        "missing path segment",
        "no lookup first",
        "renders in no UI",
        "to rename",
        "cannot nest",
        "invisible to collection queries",
        "member_of",
        "no schema change",
        "explicitly asks for a plain tags field",
        "without arguing",
    ],
};

pub const TARGET_TYPE_MUST_EXIST: SchemaRule = SchemaRule {
    id: "target-type-must-exist",
    imperative: agent_form!("target-type-must-exist"),
    prose: skill_md_form!("target-type-must-exist"),
    anchors: &[
        "existing schema ID",
        "same call",
        "create the target type first",
        "never asked for",
        "reverseName",
        "reverseCardinality",
        "rejected",
        "invoices",
        "Invoice (Customer)",
        "snake_case",
        "never declare a second relationship",
        "blocked_by",
        "parent",
    ],
};

/// The rejected-shape warning is the single most important sentence in this
/// rule (see the issue this rule closes): every other relationship an agent
/// has ever declared goes in the `relationships` array with `direction`/
/// `cardinality`/`reverseName`, so an agent inferring `extends`'s shape from
/// that surrounding pattern would write exactly the form `create_schema`/
/// `update_schema` reject. Stating the correct form as a concrete example
/// isn't decoration here — it's the one thing standing between this rule and
/// the same false-friend failure `RELATIONSHIP_VS_FIELD` and `ADD_ENUM_VALUES`
/// each exist to prevent.
///
/// Scope projection is stated because it inverts the naive expectation twice
/// over: querying the base type returns MORE rows (every extending instance)
/// but each row shows FEWER fields (only the base type's) than an agent
/// skimming ADR-078 might assume. An agent that queries `task` and then reads
/// `severity` off a result will find nothing and may wrongly conclude the
/// field write failed, rather than that it queried at the wrong scope.
///
/// `maps_to` is folded into this same rule rather than given its own —
/// `add_field_values` already has ADD_ENUM_VALUES covering the ordinary case,
/// and the inherited-field variant is a corollary of extends existing at all,
/// not a separate capability an agent would reach for independently. It is
/// stated here as "the same verb, one more required key" rather than
/// duplicating ADD_ENUM_VALUES's full mechanics.
pub const EXTENDS_SCHEMA_COMPOSITION: SchemaRule = SchemaRule {
    id: "extends-schema-composition",
    imperative: agent_form!("extends-schema-composition"),
    prose: skill_md_form!("extends-schema-composition"),
    anchors: &[
        "first-class key",
        "relationships",
        "no way to clear one once set",
        "additive only",
        "single parent only",
        "node_type: \"issue\"",
        "projected",
        "mapsTo",
        "issue_status",
        "custom:",
        "org:",
        "plugin:",
        "stored bare",
    ],
};

pub const ENUM_EDGE_FIELDS: SchemaRule = SchemaRule {
    id: "enum-edge-fields",
    imperative: agent_form!("enum-edge-fields"),
    prose: skill_md_form!("enum-edge-fields"),
    anchors: &[
        "edgeFields",
        "connection",
        "coreValues",
        "rejected on any other type",
        "must be one of the declared values",
        "unique",
        "closed",
        "userValues",
        "extensible",
        "member_of",
        "has_child",
        "mentions",
        "has_role",
        "reserved",
        "not enforced",
        "stored absent",
    ],
};

/// The premise this rule leads with is not decoration: without it, "identity
/// comes from its fields rather than free-form content" reads as a contrast
/// between structured data and a prose blob, so any entity with fields at all
/// looks like it needs a template. The real contrast is one field vs. several
/// — `content` already IS the identity for an entity type, so a single field
/// holding the whole title belongs there directly, never duplicated into a
/// second field. Measured failing both directions in one real session before
/// this rewrite: a single-field identity got a pointless template AND a
/// redundant field, and a genuinely composed identity got hand-assembled into
/// `content` instead of templated, because the rule as originally written
/// gave no way to tell those two cases apart.
///
/// Two precision notes verified against the actual title-computation path
/// (`NodeService::compute_title`, `packages/core/src/services/node_service/crud.rs`):
/// the "surfaces it as the title automatically" claim only holds for a node
/// created with no parent — a nested, non-`task`/`collection` node with no
/// `title_template` gets no computed title at all — so the rule scopes that
/// claim to "a node created without a parent" rather than stating it
/// unconditionally. And the markdown-primitive examples (`text`, `header`,
/// `quote-block`, `code-block`) are illustrative, not exhaustive — several
/// other built-ins (`ordered-list`, `checkbox`, `horizontal-line`, `table`)
/// share the same content-is-not-identity behavior — so both forms end that
/// list with "etc." rather than implying a closed set.
pub const TITLE_TEMPLATE_PLACEHOLDERS: SchemaRule = SchemaRule {
    id: "title-template-placeholders",
    imperative: agent_form!("title-template-placeholders"),
    prose: skill_md_form!("title-template-placeholders"),
    anchors: &[
        "Customer",
        "Person",
        "Invoice",
        "surfaces it as the title automatically",
        "markdown primitives",
        "single-field identity",
        "two or more fields",
        "{first_name} {last_name}",
        "{field_name}",
        "company_name",
    ],
};

pub const UNIQUE_FIELD_FLAGS: SchemaRule = SchemaRule {
    id: "unique-field-flags",
    imperative: agent_form!("unique-field-flags"),
    prose: skill_md_form!("unique-field-flags"),
    anchors: &[
        "\"unique\": true",
        "\"uniqueCaseInsensitive\": true",
        "each ticket should have a unique key",
        "email",
        "username",
        "advisory only",
        "does not prevent duplicates from being created",
        "not an enforced constraint",
    ],
};

/// All schema-authoring rules, in the order they should be rendered.
pub const SCHEMA_RULES: &[SchemaRule] = &[
    ONE_SCHEMA_PER_REQUEST,
    CREATING_TWO_LINKED_TYPES,
    SCHEMA_ALREADY_EXISTS,
    SCHEMA_VALIDATION_ERROR_RETRY,
    EDIT_DONT_RECREATE,
    ADD_ENUM_VALUES,
    RENAME_VS_RELABEL,
    DELETE_A_SCHEMA,
    EXTENDS_SCHEMA_COMPOSITION,
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

/// The shape a play's rules are written in (ADR-090 §1). The example is a
/// whole rule, so both surfaces copy the same payload: a described rule, a
/// condition object and a described action.
pub const PLAY_RULE_DESCRIPTIONS: SchemaRule = SchemaRule {
    id: "play-rule-descriptions",
    imperative: agent_form!("play-rule-descriptions"),
    prose: skill_md_form!("play-rule-descriptions"),
    anchors: &[
        "required `description`",
        "in the same write",
        "an object with `expr` and `description`",
        "never a bare expression",
        "beside `action_type`, `params` and `for_each`",
        "trigger takes no description",
        "missing or blank",
    ],
};

/// What a rejected rules write means and how to repair it. The quoted error
/// is the text `playbook::descriptions` renders, so an agent recognizes the
/// rejection when it meets one.
pub const PLAY_STALE_DESCRIPTION: SchemaRule = SchemaRule {
    id: "play-stale-description",
    imperative: agent_form!("play-stale-description"),
    prose: skill_md_form!("play-stale-description"),
    anchors: &[
        "in the same write",
        "condition `expr`",
        "`action_type`, `params` or `for_each`",
        "`trigger` or `class`",
        "keeps its stored description is rejected",
        "same `name`",
        "by position",
        "a renamed rule is a new rule",
        "its expression changed and its description didn't",
        "corrected payload",
    ],
};

/// The play-authoring rules, in the order they should be rendered.
pub const PLAY_RULES: &[SchemaRule] = &[PLAY_RULE_DESCRIPTIONS, PLAY_STALE_DESCRIPTION];

/// Every rule whose two forms are tied by anchors and shared JSON examples.
fn anchored_rules() -> impl Iterator<Item = &'static SchemaRule> {
    SCHEMA_RULES.iter().chain(PLAY_RULES)
}

pub const FIND_THEN_ACT: InteractionRule = InteractionRule {
    id: "find-then-act",
    imperative: agent_form!("find-then-act"),
    prose: skill_md_form!("find-then-act"),
};

/// A name is not an id, and whether a named record already exists is decided
/// by whether one of the lookup's results IS that record, not by whether the
/// lookup came back empty.
///
/// The two surfaces get the lookup from different places. The local agent is
/// handed it: the entity tier resolves names into MENTIONED ENTITIES before
/// the turn starts. An external agent has to run it, and the command matters:
/// `nodespace search` ranks by meaning, so it returns records that are only
/// similar and cannot show that a record is absent, and a task has no
/// embedding to be found by. `node query --title-contains` is the lexical
/// title lookup.
///
/// What both surfaces share is the judgment on the result — same name, same
/// type. The judgment is on sameness rather than emptiness because both
/// lookups also return records that merely share a word with the name.
///
/// On the local surface only "none found" is complete. A populated
/// MENTIONED ENTITIES list can be cut two ways. A cut by the tier's cap is
/// announced by the "more matches not shown" note, which does not say which
/// records were cut. A cut by its score cutoff is not announced, because most
/// of what the cutoff drops is filler sharing one word with a name. When the
/// cutoff drops a real common-word name beside a rare-word one, the one sign
/// is that name's absence from the list. That is why the imperative has the
/// model check every name in the message against the list.
pub const NAMED_RECORD_RESOLUTION: InteractionRule = InteractionRule {
    id: "named-record-resolution",
    imperative: agent_form!("named-record-resolution"),
    prose: skill_md_form!("named-record-resolution"),
};

pub const AMBIGUITY_CLARIFY: InteractionRule = InteractionRule {
    id: "ambiguity-clarify",
    // The local form calls route_clarify rather than asking in prose: the
    // golden corpus measured calling the tool as more reliable, since prose
    // is invisible to anything downstream that expects a tool call. An agent
    // outside the app has no such tool, so its form asks the user directly.
    imperative: agent_form!("ambiguity-clarify"),
    prose: skill_md_form!("ambiguity-clarify"),
};

pub const SUCCESS_NO_REVERIFY: InteractionRule = InteractionRule {
    id: "success-no-reverify",
    imperative: agent_form!("success-no-reverify"),
    prose: skill_md_form!("success-no-reverify"),
};

/// The dedicated-verb instruction is a stable API constraint and is stated
/// outright. The *value list* deliberately is not: `task.status` is declared
/// `extensible` and ADR-076's `add_field_values` lets a user
/// append real values to it, so any list written here is only correct until
/// the first such install. The guidance points at `update_task_status`'s own
/// `status` enum instead, which `tools::with_live_task_statuses` rewrites
/// from the stored schema each turn.
///
/// It cannot point at the `EXISTING SCHEMAS` block: that block excludes core
/// types by construction (`context_ops::non_core_schema_hits`
/// filters on `!is_core`, pinned by
/// `non_core_schema_hits_excludes_core_types`), because presence
/// there is what tells the model a type is user-defined and takes bare
/// property keys. `task` is a core type, so `task.status` never appears in it.
pub const TASK_STATUS_DEDICATED_VERB: InteractionRule = InteractionRule {
    id: "task-status-dedicated-verb",
    imperative: agent_form!("task-status-dedicated-verb"),
    prose: skill_md_form!("task-status-dedicated-verb"),
};

/// The two forms differ on who asks for confirmation. The app asks the user
/// before a local agent's delete removes anything, so the local form tells
/// the model not to ask. `node delete` previews and leaves the asking to the
/// agent that ran it.
pub const SINGLE_ITEM_PER_CALL: InteractionRule = InteractionRule {
    id: "single-item-per-call",
    imperative: agent_form!("single-item-per-call"),
    prose: skill_md_form!("single-item-per-call"),
};

/// Collection assignment is a create-time argument, not a follow-up write.
/// The path resolves and auto-creates every missing segment in one call, so
/// the lookup-create-link sequence agents reach for by default is pure cost —
/// and so is the `tags: []` array they choose instead when they price that
/// sequence into the decision.
pub const COLLECTION_AT_CREATE_TIME: InteractionRule = InteractionRule {
    id: "collection-at-create-time",
    imperative: agent_form!("collection-at-create-time"),
    prose: skill_md_form!("collection-at-create-time"),
};

/// A relationship is declared once, on the source type, but is legitimately
/// read from both ends. Whichever end you start from, the traversal exists —
/// the only question is which name spells it. Getting that wrong used to return
/// an empty result, which reads as "there is nothing there" and talks callers
/// out of the reverse query entirely; it now errors and names the working
/// spellings, but the mapping is cheap to state up front.
///
/// A reverse name already fixes the direction — it is the forward edge read
/// from the target end — so `direction` is ignored for it; only the forward
/// name lets `direction` choose an end. Without that sentence a caller pairing
/// the reverse name with `in` expects a second, further-reversed traversal.
pub const RELATIONSHIP_REVERSE_TRAVERSAL: InteractionRule = InteractionRule {
    id: "relationship-reverse-traversal",
    imperative: agent_form!("relationship-reverse-traversal"),
    prose: skill_md_form!("relationship-reverse-traversal"),
};

/// Which end of an edge is which. A reversed edge is accepted and records the
/// opposite fact, so nothing downstream reports the mistake: the rule is the
/// only thing between a caller and a silently wrong graph. The local form
/// names the tool's `from_id`/`to_id`, the other the CLI's `--from`/`--to`.
///
/// `create_relationship`'s own parameter descriptions state it as well: a
/// turn routed to Organization holds the tool without this skill's guidance.
pub const RELATIONSHIP_DIRECTION: InteractionRule = InteractionRule {
    id: "relationship-direction",
    imperative: agent_form!("relationship-direction"),
    prose: skill_md_form!("relationship-direction"),
};

pub const BULK_IMPORT_NO_FOLLOWUP_SEARCH: InteractionRule = InteractionRule {
    id: "bulk-import-no-followup-search",
    imperative: agent_form!("bulk-import-no-followup-search"),
    prose: skill_md_form!("bulk-import-no-followup-search"),
};

/// A rejected write is resolved at its cause (ADR-092 §6): the rules are
/// Plays, a rejection names what is missing, and the same write made another
/// way meets the same rule. Every skill that changes a status the rules guard
/// carries it.
pub const REJECTED_WRITE: InteractionRule = InteractionRule {
    id: "rejected-write",
    imperative: agent_form!("rejected-write"),
    prose: skill_md_form!("rejected-write"),
};

/// What a refused write that named a version means (ADR-094 §6): read again,
/// and stop if the task is no longer the reader's to work on. Shared by the
/// procedures that claim a task by the version they read.
pub const VERSION_CONFLICT: InteractionRule = InteractionRule {
    id: "version-conflict",
    imperative: agent_form!("version-conflict"),
    prose: skill_md_form!("version-conflict"),
};

/// A criterion or a checklist item is a checkbox directly under its spec or
/// task (ADR-092 §2), written unchecked.
pub const CHECKBOX_ITEM_CREATE: InteractionRule = InteractionRule {
    id: "checkbox-item-create",
    imperative: agent_form!("checkbox-item-create"),
    prose: skill_md_form!("checkbox-item-create"),
};

/// Approval is the user's (ADR-092 §6). The local form asks through
/// route_clarify for the reason [`AMBIGUITY_CLARIFY`] does.
pub const APPROVAL_ASK_FIRST: InteractionRule = InteractionRule {
    id: "approval-ask-first",
    imperative: agent_form!("approval-ask-first"),
    prose: skill_md_form!("approval-ask-first"),
};

/// Reading one task with what governs it, for a procedure that is given a
/// task by name instead of taking the next one from its queue.
pub const TASK_CONTEXT_READ: InteractionRule = InteractionRule {
    id: "task-context-read",
    imperative: agent_form!("task-context-read"),
    prose: skill_md_form!("task-context-read"),
};

/// All generic interaction-pattern rules, in no particular required order —
/// each is consumed independently by whichever skill needs it.
pub const INTERACTION_RULES: &[InteractionRule] = &[
    REJECTED_WRITE,
    VERSION_CONFLICT,
    CHECKBOX_ITEM_CREATE,
    APPROVAL_ASK_FIRST,
    TASK_CONTEXT_READ,
    FIND_THEN_ACT,
    NAMED_RECORD_RESOLUTION,
    AMBIGUITY_CLARIFY,
    SUCCESS_NO_REVERIFY,
    TASK_STATUS_DEDICATED_VERB,
    SINGLE_ITEM_PER_CALL,
    COLLECTION_AT_CREATE_TIME,
    RELATIONSHIP_DIRECTION,
    RELATIONSHIP_REVERSE_TRAVERSAL,
    BULK_IMPORT_NO_FOLLOWUP_SEARCH,
];

/// Every step of a seeded skill's procedure that names a tool or a command,
/// grouped by the skill that includes it.
pub const PROCEDURE_RULES: &[ProcedureRule] = &[
    // Research & Search
    procedure_rule!("research-search-first"),
    procedure_rule!("research-search-reference"),
    // Node Creation
    procedure_rule!("node-creation-new-or-existing"),
    procedure_rule!("node-creation-steps"),
    // Schema Creation
    procedure_rule!("schema-creation-call"),
    // Graph Editing
    procedure_rule!("graph-editing-existing-or-new"),
    procedure_rule!("graph-editing-call-now"),
    procedure_rule!("graph-editing-update-call"),
    procedure_rule!("graph-editing-indirect-reference"),
    procedure_rule!("graph-editing-carry-the-change"),
    procedure_rule!("graph-editing-content-vs-fields"),
    // Relationship Management
    procedure_rule!("relationship-create"),
    procedure_rule!("relationship-find-before-link"),
    // Node Deletion
    procedure_rule!("node-deletion-wrong-skill"),
    procedure_rule!("node-deletion-delete-call"),
    // Conflict Journal
    procedure_rule!("conflict-journal-find"),
    procedure_rule!("conflict-journal-dismiss-vs-adopt"),
    // Node Merge
    procedure_rule!("node-merge-not-always"),
    procedure_rule!("node-merge-steps"),
    // Play Workflow State
    procedure_rule!("play-workflow-call"),
    procedure_rule!("play-workflow-rerun"),
    procedure_rule!("play-workflow-find-first"),
    // Play Authoring
    procedure_rule!("play-authoring-read"),
    procedure_rule!("play-authoring-find-first"),
    procedure_rule!("play-authoring-schemas"),
    procedure_rule!("play-authoring-ask-first"),
    procedure_rule!("play-authoring-write"),
    procedure_rule!("play-authoring-switch"),
    // Bulk Import
    procedure_rule!("bulk-import-steps"),
    // Organization
    procedure_rule!("organization-existing-only"),
    procedure_rule!("organization-add-existing"),
    // Writing a Spec
    procedure_rule!("spec-find-first"),
    procedure_rule!("spec-create"),
    procedure_rule!("spec-approve"),
    // Writing a Plan
    procedure_rule!("plan-read-spec"),
    procedure_rule!("plan-existing-plans"),
    procedure_rule!("plan-create-and-link"),
    procedure_rule!("plan-approve"),
    // Breaking a Plan into Tasks
    procedure_rule!("task-breakdown-read"),
    procedure_rule!("task-breakdown-existing"),
    procedure_rule!("task-breakdown-create"),
    procedure_rule!("task-breakdown-link"),
    procedure_rule!("task-breakdown-blocks"),
    // Implementing a Task
    procedure_rule!("task-implement-take"),
    procedure_rule!("task-implement-start"),
    procedure_rule!("task-implement-tick"),
    procedure_rule!("task-implement-links"),
    procedure_rule!("task-implement-review"),
    // Reviewing a Task
    procedure_rule!("task-review-take"),
    procedure_rule!("task-review-read"),
    procedure_rule!("task-review-note"),
    procedure_rule!("task-review-pass"),
    procedure_rule!("task-review-return"),
    // Completing a Task
    procedure_rule!("task-complete-read"),
    procedure_rule!("task-complete-done"),
    // Recording a Decision
    procedure_rule!("decision-find-first"),
    procedure_rule!("decision-create"),
    procedure_rule!("decision-accept"),
    procedure_rule!("decision-link"),
    procedure_rule!("decision-supersede"),
    // Authoring a Skill
    procedure_rule!("skill-authoring-find-first"),
    procedure_rule!("skill-authoring-tools"),
    procedure_rule!("skill-authoring-create"),
    procedure_rule!("skill-authoring-link"),
    procedure_rule!("skill-authoring-edit"),
];

/// Which audience a body is being built for, and so which form of a rule an
/// include marker resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleForm {
    /// The local agent: seeded guidance and skills, naming its tools.
    Agent,
    /// An agent outside the app: a skill fetched with `nodespace skill
    /// guidance`, and the shipped command reference. Names CLI commands.
    SkillMd,
}

/// The text of rule `id` in `form`, or `None` when no rule has that id.
pub fn rule_text(id: &str, form: RuleForm) -> Option<&'static str> {
    let pick = |imperative, prose| match form {
        RuleForm::Agent => imperative,
        RuleForm::SkillMd => prose,
    };
    anchored_rules()
        .find(|r| r.id == id)
        .map(|r| pick(r.imperative, r.prose))
        .or_else(|| {
            INTERACTION_RULES
                .iter()
                .find(|r| r.id == id)
                .map(|r| pick(r.imperative, r.prose))
        })
        .or_else(|| {
            PROCEDURE_RULES
                .iter()
                .find(|r| r.id == id)
                .map(|r| pick(r.imperative, r.prose))
        })
}

const INCLUDE_OPEN: &str = "<!-- include: ";
const INCLUDE_CLOSE: &str = " -->";

/// `body` with every `<!-- include: <rule-id> -->` marker replaced by that
/// rule's text in `form`. A marker may sit on a line of its own or in the
/// middle of a sentence.
///
/// # Panics
///
/// If a marker is unterminated or names no rule. Bodies are `include_str!`
/// constants, so this is a build-content error, surfaced by the tests that
/// build every seed.
pub fn resolve_includes(body: &str, form: RuleForm) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find(INCLUDE_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + INCLUDE_OPEN.len()..];
        let end = after
            .find(INCLUDE_CLOSE)
            .unwrap_or_else(|| panic!("unterminated include marker in seed body: {after:?}"));
        let id = &after[..end];
        let text = rule_text(id, form)
            .unwrap_or_else(|| panic!("seed body includes `{id}`, which is not a rule id"));
        out.push_str(text);
        rest = &after[end + INCLUDE_CLOSE.len()..];
    }
    out.push_str(rest);
    out
}

/// The schema-rules region of the shipped skill: the schema rules in their
/// prose form, in the order and grouping `seeds/skill-md/schema-rules.md`
/// lays out.
pub fn skill_md_schema_rules() -> String {
    resolve_includes(
        include_str!("seeds/skill-md/schema-rules.md").trim_ascii_end(),
        RuleForm::SkillMd,
    )
}

/// The relationship-rules region of the shipped command reference: which end
/// of an edge is which, and how to read one from its other end, in the same
/// words the Relationship Management skill serves an agent outside the app.
pub fn skill_md_relationship_rules() -> String {
    format!(
        "{}\n\n{}",
        RELATIONSHIP_DIRECTION.prose, RELATIONSHIP_REVERSE_TRAVERSAL.prose
    )
}

/// The play-rules region of the shipped skill: the play-authoring rules in
/// their prose form, as `seeds/skill-md/play-rules.md` lays them out.
pub fn skill_md_play_rules() -> String {
    resolve_includes(
        include_str!("seeds/skill-md/play-rules.md").trim_ascii_end(),
        RuleForm::SkillMd,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every form of every rule, of every kind, as `(id, text)`.
    fn every_rule_form() -> impl Iterator<Item = (&'static str, &'static str)> {
        anchored_rules()
            .flat_map(|r| [(r.id, r.imperative), (r.id, r.prose)])
            .chain(
                INTERACTION_RULES
                    .iter()
                    .flat_map(|r| [(r.id, r.imperative), (r.id, r.prose)]),
            )
            .chain(
                PROCEDURE_RULES
                    .iter()
                    .flat_map(|r| [(r.id, r.imperative), (r.id, r.prose)]),
            )
    }

    #[test]
    fn an_include_marker_resolves_to_the_form_for_its_surface() {
        let body = "FIND THEN UPDATE: <!-- include: find-then-act --> Then stop.";
        assert_eq!(
            resolve_includes(body, RuleForm::Agent),
            format!("FIND THEN UPDATE: {} Then stop.", FIND_THEN_ACT.imperative)
        );
        assert_eq!(
            resolve_includes(body, RuleForm::SkillMd),
            format!("FIND THEN UPDATE: {} Then stop.", FIND_THEN_ACT.prose)
        );
        assert_eq!(
            resolve_includes("no markers", RuleForm::Agent),
            "no markers"
        );
    }

    #[test]
    #[should_panic(expected = "which is not a rule id")]
    fn an_include_of_an_unknown_rule_is_a_build_error() {
        resolve_includes("<!-- include: no-such-rule -->", RuleForm::Agent);
    }

    /// A fragment is included mid-sentence, so it carries no trailing line
    /// break of its own, and it is plain Markdown: no frontmatter, and no
    /// include of another fragment.
    #[test]
    fn rule_fragments_are_plain_markdown() {
        for (id, text) in every_rule_form() {
            assert_eq!(text, text.trim_end(), "{id} ends in whitespace");
            assert!(!text.starts_with("---"), "{id} starts with frontmatter");
            assert!(
                !text.contains(INCLUDE_OPEN),
                "{id} includes another fragment"
            );
        }
    }

    /// Every schema rule reaches the shipped skill through the one template,
    /// exactly once.
    #[test]
    fn the_skill_md_region_includes_every_schema_rule_once() {
        let template = include_str!("seeds/skill-md/schema-rules.md");
        for r in SCHEMA_RULES {
            assert_eq!(
                template
                    .matches(&format!("{INCLUDE_OPEN}{}{INCLUDE_CLOSE}", r.id))
                    .count(),
                1,
                "{} must be included once in seeds/skill-md/schema-rules.md",
                r.id
            );
        }
        let rendered = skill_md_schema_rules();
        assert!(!rendered.contains(INCLUDE_OPEN), "{rendered}");
    }

    /// Every play rule reaches the shipped skill through its own template,
    /// exactly once.
    #[test]
    fn the_skill_md_region_includes_every_play_rule_once() {
        let template = include_str!("seeds/skill-md/play-rules.md");
        for r in PLAY_RULES {
            assert_eq!(
                template
                    .matches(&format!("{INCLUDE_OPEN}{}{INCLUDE_CLOSE}", r.id))
                    .count(),
                1,
                "{} must be included once in seeds/skill-md/play-rules.md",
                r.id
            );
        }
        let rendered = skill_md_play_rules();
        assert!(!rendered.contains(INCLUDE_OPEN), "{rendered}");
    }

    #[test]
    fn all_schema_rule_ids_are_unique() {
        let mut ids: Vec<&str> = anchored_rules().map(|r| r.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate SchemaRule id");
    }

    /// The rule example both surfaces copy is a rule the play types decode,
    /// and one the description checks accept.
    #[test]
    fn the_play_rule_example_is_a_valid_described_rule() {
        use nodespace_core::playbook::types::RuleDefinition;
        for text in [
            PLAY_RULE_DESCRIPTIONS.imperative,
            PLAY_RULE_DESCRIPTIONS.prose,
        ] {
            let example = json_examples(text)
                .into_iter()
                .find(|example| example.get("trigger").is_some())
                .unwrap_or_else(|| panic!("no rule example in: {text}"));
            let rule: RuleDefinition = serde_json::from_value(example)
                .unwrap_or_else(|e| panic!("the rule example does not decode: {e}"));
            assert_eq!(rule.conditions.len(), 2);
            assert_eq!(rule.actions.len(), 1);
            nodespace_core::playbook::descriptions::check_descriptions(&[rule], None)
                .expect("the example's descriptions pass");
        }
    }

    /// `has_child` reaches every child, a checkbox or a note included, so a
    /// comprehension over it that reads a task field fails on them. No
    /// shipped rule shows one, and the guidance for conditions teaches
    /// `type_chain` for asking whether a node is a task.
    #[test]
    fn no_rule_shows_a_task_field_read_on_every_child() {
        for r in anchored_rules() {
            for text in [r.imperative, r.prose] {
                assert!(!text.contains("has_child.all(c, c.status"), "{}", r.id);
            }
        }
        for text in [
            agent_form!("play-authoring-schemas"),
            skill_md_form!("play-authoring-schemas"),
        ] {
            assert!(text.contains("'task' in node.type_chain"), "{text}");
        }
    }

    /// The stale rule quotes the rejection an agent will meet, so it has to
    /// quote what the check actually renders.
    #[test]
    fn the_stale_rule_quotes_the_rejection_the_check_renders() {
        use nodespace_core::playbook::descriptions::{
            DescribedComponent, DescriptionError, DescriptionProblem,
        };
        let rendered = DescriptionError {
            rule: "settle done task".to_string(),
            component: DescribedComponent::Condition(1),
            problem: DescriptionProblem::Stale,
        }
        .to_string();
        let quoted = "rule `settle done task`, condition 2: its expression changed and its \
                      description didn't";
        assert!(rendered.starts_with(quoted), "{rendered}");
        for text in [
            PLAY_STALE_DESCRIPTION.imperative,
            PLAY_STALE_DESCRIPTION.prose,
        ] {
            assert!(text.contains(quoted), "{text}");
        }
    }

    /// The rule tells the model what the truncation note means, so it has to
    /// name the note by the phrase the entity tier renders. A reworded note
    /// the rule no longer quotes leaves the model with a line it has no
    /// instruction for.
    #[test]
    fn named_record_rule_quotes_the_truncation_marker() {
        assert!(
            NAMED_RECORD_RESOLUTION
                .imperative
                .contains(nodespace_core::ops::context_ops::ENTITIES_NOT_SHOWN_MARKER),
            "ALREADY IN THE GRAPH must quote the entity tier's truncation marker"
        );
    }

    /// An include marker names a rule by id alone, whatever its kind, so an
    /// id shared by two rules would resolve to whichever kind is looked up
    /// first.
    #[test]
    fn rule_ids_are_unique_across_every_kind() {
        let mut ids: Vec<&str> = every_rule_form().map(|(id, _)| id).collect();
        let forms = ids.len();
        ids.sort_unstable();
        ids.dedup();
        // Each rule contributes two forms under one id.
        assert_eq!(ids.len() * 2, forms, "duplicate rule id");
    }

    #[test]
    fn no_rule_text_is_empty() {
        for (id, text) in every_rule_form() {
            assert!(!text.is_empty(), "{id} has an empty form");
        }
    }

    /// Both forms of every schema rule carry each of its anchors. The prose
    /// form is rendered verbatim into the shipped skill (and held there by
    /// `checked_in_skill_md_is_up_to_date`), so this is the one check that
    /// ties the in-app agent's wording to what external agents read.
    #[test]
    fn schema_rule_forms_share_their_anchors() {
        let mut drift = Vec::new();
        for r in anchored_rules() {
            assert!(!r.anchors.is_empty(), "{} declares no anchors", r.id);
            let imperative = r.imperative.to_lowercase();
            let prose = r.prose.to_lowercase();
            for anchor in r.anchors {
                let needle = anchor.to_lowercase();
                for (form, text) in [("imperative", &imperative), ("prose", &prose)] {
                    if !text.contains(&needle) {
                        drift.push(format!("{}: {form} is missing {anchor:?}", r.id));
                    }
                }
            }
        }
        assert!(
            drift.is_empty(),
            "schema rule forms have drifted apart — carry the change into the \
             other form, or update `anchors` if the fact itself changed:\n{}",
            drift.join("\n")
        );
    }

    /// Rules whose prose walks through complete CLI commands the imperative
    /// form has no counterpart for: the local agent issues those steps as
    /// tool calls it is told about in words, not as copied payloads. Only the
    /// prose-side examples are exempt — an example added to the imperative
    /// form must still appear in the prose.
    const PROSE_ONLY_EXAMPLES: &[&str] = &["delete-a-schema"];

    /// Every JSON object embedded in `text`, including nested ones, parsed so
    /// that formatting differences between the forms (`"a": 1` vs `"a":1`)
    /// don't count as drift. Anything that doesn't parse is skipped: that is
    /// placeholder braces such as `{name}`, and also the deliberately elided
    /// rejected shape both forms of `EXTENDS_SCHEMA_COMPOSITION` show
    /// (`{"relationships": [{"name": "extends", ...}]}`), which is an
    /// illustration of what not to send rather than a payload to copy.
    fn json_examples(text: &str) -> Vec<serde_json::Value> {
        let mut found = Vec::new();
        for (i, _) in text.match_indices('{') {
            let Some(raw) = crate::local_agent::tools::extract_json_object(&text[i..]) else {
                continue;
            };
            if let Ok(value @ serde_json::Value::Object(_)) = serde_json::from_str(raw) {
                if !found.contains(&value) {
                    found.push(value);
                }
            }
        }
        found
    }

    /// Both forms of a schema rule show the same JSON examples. An example is
    /// a wire shape an agent copies verbatim, so an edit to one form's
    /// example that the other form doesn't follow teaches the two surfaces
    /// different payloads.
    #[test]
    fn schema_rule_forms_share_their_json_examples() {
        let mut drift = Vec::new();
        for r in anchored_rules() {
            let prose_only_allowed = PROSE_ONLY_EXAMPLES.contains(&r.id);
            let imperative = json_examples(r.imperative);
            let prose = json_examples(r.prose);
            for example in &imperative {
                if !prose.contains(example) {
                    drift.push(format!(
                        "{}: only the imperative form shows {example}",
                        r.id
                    ));
                }
            }
            for example in &prose {
                if !prose_only_allowed && !imperative.contains(example) {
                    drift.push(format!("{}: only the prose form shows {example}", r.id));
                }
            }
        }
        assert!(
            drift.is_empty(),
            "schema rule examples differ between forms:\n{}",
            drift.join("\n")
        );
    }

    #[test]
    fn prose_only_examples_names_real_rules() {
        for id in PROSE_ONLY_EXAMPLES {
            assert!(
                SCHEMA_RULES.iter().any(|r| r.id == *id),
                "PROSE_ONLY_EXAMPLES names {id:?}, which is not in SCHEMA_RULES"
            );
        }
    }

    /// Extracts the JSON object following "Example: " in a rule's doc text
    /// (optionally wrapped in a single pair of markdown backticks). Brace
    /// balancing is delegated to
    /// [`crate::local_agent::tools::extract_json_object`] rather than
    /// reimplemented here — it already tracks string-literal state, so a
    /// `{`/`}` inside a quoted value (e.g. a description quoting a
    /// `{placeholder}`) doesn't throw off the match the way a naive counter
    /// would.
    fn extract_example_json(text: &str) -> &str {
        let marker = "Example: ";
        let after = text
            .rfind(marker)
            .map(|i| &text[i + marker.len()..])
            .unwrap_or_else(|| panic!("no \"Example: \" marker found in: {text:?}"));
        let after = after.strip_prefix('`').unwrap_or(after);
        crate::local_agent::tools::extract_json_object(after)
            .unwrap_or_else(|| panic!("no JSON object after \"Example: \" in: {text:?}"))
    }

    /// `UNIQUE_FIELD_FLAGS`'s documented `uniqueCaseInsensitive` example
    /// (both the imperative form seeded into the in-app agent prompt and the
    /// prose form rendered into the shipped skill) must deserialize as a real
    /// `SchemaField` through the exact wire format `create_schema`/
    /// `update_schema` accept. `SchemaField` is `#[serde(rename_all =
    /// "camelCase", deny_unknown_fields)]` (`nodespace-types::schema`), so a
    /// doc example written in the struct's own snake_case field name — as
    /// this rule's text once was — is rejected as an unknown field the
    /// moment it's copied verbatim, rather than caught here.
    #[test]
    fn unique_field_flags_example_matches_schema_field_wire_format() {
        for text in [UNIQUE_FIELD_FLAGS.imperative, UNIQUE_FIELD_FLAGS.prose] {
            let json = extract_example_json(text);
            let field: nodespace_core::models::SchemaField = serde_json::from_str(json)
                .unwrap_or_else(|e| {
                    panic!(
                        "documented example {json} failed to deserialize as SchemaField \
                         (wire key mismatch?): {e}"
                    )
                });
            assert_eq!(
                field.unique_case_insensitive,
                Some(true),
                "documented example should set uniqueCaseInsensitive: true"
            );
        }
    }

    /// `packages/skill/references/cli.md`'s "Create a schema with a unique
    /// field" worked example is hand-maintained prose living outside the
    /// `<!-- BEGIN GENERATED: schema-rules -->` marker `gen_skill_md.rs`
    /// regenerates from `UNIQUE_FIELD_FLAGS` — regenerating the skill (or
    /// `checked_in_skill_md_is_up_to_date`, `skill_md_generation.rs`) cannot
    /// catch this example drifting on its own, since it's never produced
    /// from source. This is the exact artifact the tracking issue reported:
    /// a complete `nodespace schema create` command that fails if copied
    /// verbatim.
    #[test]
    fn cli_md_unique_field_worked_example_matches_schema_field_wire_format() {
        let cli_md_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skill/references/cli.md");
        let cli_md = std::fs::read_to_string(&cli_md_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", cli_md_path.display()));

        let marker = "# Create a schema with a unique field";
        let after_comment = cli_md.find(marker).map(|i| &cli_md[i..]).unwrap_or_else(|| {
            panic!("cli.md no longer has a {marker:?} worked example — update this test if it moved")
        });

        let command_line = after_comment
            .lines()
            .find(|l| l.starts_with("nodespace schema create"))
            .unwrap_or_else(|| panic!("no `nodespace schema create` line follows {marker:?}"));

        let params_marker = "--params '";
        let quote_start = command_line
            .find(params_marker)
            .map(|i| i + params_marker.len())
            .unwrap_or_else(|| panic!("no {params_marker:?} found in: {command_line}"));
        let quote_end = command_line[quote_start..]
            .rfind('\'')
            .map(|i| quote_start + i)
            .unwrap_or_else(|| panic!("unterminated --params string in: {command_line}"));
        let json = &command_line[quote_start..quote_end];

        let params: nodespace_core::schema::CreateSchemaParams = serde_json::from_str(json)
            .unwrap_or_else(|e| {
                panic!(
                    "cli.md worked example failed to deserialize as CreateSchemaParams \
                     (wire key mismatch?): {e}"
                )
            });
        let key_field = params
            .fields
            .as_ref()
            .and_then(|fields| fields.iter().find(|f| f.name == "key"))
            .unwrap_or_else(|| panic!("worked example no longer declares a \"key\" field"));
        assert_eq!(
            key_field.unique_case_insensitive,
            Some(true),
            "cli.md worked example should set uniqueCaseInsensitive: true on the \"key\" field"
        );
    }
}
