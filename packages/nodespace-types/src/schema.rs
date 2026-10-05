use serde::{Deserialize, Serialize};

use crate::core_type::{ChildrenRule, CoreNodeType, ParentRule};
use crate::node::NodeEnvelope;
use crate::relationship_path::RelationshipPath;

fn default_schema_version() -> u32 {
    1
}

/// Derive a display label for a field whose `friendlyName` was omitted at
/// `create_schema`/`update_schema` time, e.g. `due_date` -> `Due date`,
/// `custom:capacity` -> `Capacity`, `estimatedHours` -> `Estimated hours`,
/// `employeeIDNumber` -> `Employee id number`.
///
/// Namespace prefixes (`custom:`, `org:`, `plugin:`, ...) are stripped before
/// humanizing — a display-only operation with no effect on the stored name.
/// This is reachable *today*, not just as a future hazard: adding
/// `custom:status` next to an existing core `status` field derives "Status"
/// for both, since the two have different storage keys but the same
/// stripped/humanized text. This function does not resolve that on its own —
/// callers (`apply_friendly_name_defaults` in `packages/core/src/schema/mod.rs`)
/// are responsible for disambiguating a derived value that collides with
/// another field already in the schema.
pub fn derive_friendly_name(name: &str) -> String {
    let base = name.rsplit(':').next().unwrap_or(name);
    let chars: Vec<char> = base.chars().collect();

    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }

        // A word boundary precedes `ch` when either:
        // - the previous char is lowercase/digit and this one is uppercase
        //   (`dueX` -> `due|X`), or
        // - this is the last uppercase letter of an acronym run immediately
        //   followed by a lowercase letter (`IDNumber` -> `ID|Number`, not
        //   `IDN|umber`) — without this second rule, an acronym directly
        //   adjacent to the next word (`employeeIDNumber`) merges into one
        //   unsplit blob instead of three words.
        let boundary = match chars.get(i.wrapping_sub(1)) {
            Some(&prev) if i > 0 => {
                let prev_lower = prev.is_lowercase() || prev.is_ascii_digit();
                let cur_upper = ch.is_uppercase();
                let next_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
                (prev_lower && cur_upper) || (prev.is_uppercase() && cur_upper && next_lower)
            }
            _ => false,
        };

        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }

    if words.is_empty() {
        return name.to_string();
    }

    words
        .into_iter()
        .enumerate()
        .map(|(i, w)| {
            let lower = w.to_lowercase();
            if i == 0 {
                let mut chars = lower.chars();
                match chars.next() {
                    Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                    None => lower,
                }
            } else {
                lower
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnumValue {
    pub value: String,
    pub label: String,
    /// For a value added to a field this schema *inherited* via `extends`:
    /// which pre-existing value it collapses to at an ancestor's scope
    /// (ADR-078).
    ///
    /// An `issue` extending `task` may add `backlog` to the inherited
    /// `status`, mapping to `todo`. A consumer reading at `task`'s scope — a
    /// Play condition, a query filter, any CEL expression written against the
    /// base type — then sees `todo`, the value it was written to understand,
    /// rather than a `backlog` it has never heard of. Jira's status-category
    /// model is the direct precedent.
    ///
    /// Required on every value appended to an inherited field, and neither
    /// required nor meaningful on a field the schema declares itself: an own
    /// field has no ancestor scope whose meaning needs preserving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maps_to: Option<String>,
}

impl EnumValue {
    /// An enum value with no `maps_to` — the shape every value on a schema's
    /// own field takes, and what the core schema definitions construct.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            maps_to: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum SchemaProtectionLevel {
    Core,
    #[default]
    User,
    System,
}

impl std::fmt::Display for SchemaProtectionLevel {
    /// Mirrors the wire form (`#[serde(rename_all = "lowercase")]`) so error
    /// messages naming a protection level match what the same value
    /// serializes to over the API.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SchemaProtectionLevel::Core => "core",
            SchemaProtectionLevel::User => "user",
            SchemaProtectionLevel::System => "system",
        };
        write!(f, "{s}")
    }
}

/// The type of a schema field: the one field-type vocabulary, shared by the
/// wire type, the schema validator and the frontend (ADR-086 §7a).
///
/// `text` is the string type. `string` is not a field type and is refused,
/// with a message that names `text`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename_all = "lowercase"))]
pub enum SchemaFieldType {
    #[default]
    Text,
    Number,
    Boolean,
    Date,
    Datetime,
    Enum,
    Array,
    Object,
    /// A web link: a [`LinkValue`], a title and an absolute URL.
    Link,
}

impl SchemaFieldType {
    /// Every field type, in the order they are listed to a user.
    pub const ALL: [SchemaFieldType; 9] = [
        SchemaFieldType::Text,
        SchemaFieldType::Number,
        SchemaFieldType::Boolean,
        SchemaFieldType::Date,
        SchemaFieldType::Datetime,
        SchemaFieldType::Enum,
        SchemaFieldType::Array,
        SchemaFieldType::Object,
        SchemaFieldType::Link,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SchemaFieldType::Text => "text",
            SchemaFieldType::Number => "number",
            SchemaFieldType::Boolean => "boolean",
            SchemaFieldType::Date => "date",
            SchemaFieldType::Datetime => "datetime",
            SchemaFieldType::Enum => "enum",
            SchemaFieldType::Array => "array",
            SchemaFieldType::Object => "object",
            SchemaFieldType::Link => "link",
        }
    }
}

/// The value of a `link` field: a title and the URL it opens.
///
/// A nested stored value, so its keys are snake_case and an unknown key is
/// refused (ADR-086 §7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct LinkValue {
    pub title: String,
    /// An absolute URL: one with a scheme and a host. Any scheme is stored;
    /// a client decides which schemes it opens.
    pub url: String,
}

impl LinkValue {
    /// Read a link from a stored or submitted value. The error says what is
    /// wrong with the value and does not name the field holding it.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, String> {
        const SHAPE: &str = "an object with exactly the keys 'title' and 'url'";
        let Some(object) = value.as_object() else {
            return Err(match value.as_str() {
                Some(s) => format!("must be {SHAPE}, but received the string '{s}'"),
                None => format!("must be {SHAPE}"),
            });
        };
        if let Some(extra) = object.keys().find(|k| *k != "title" && *k != "url") {
            return Err(format!("has an unknown key '{extra}'; a link is {SHAPE}"));
        }
        let part = |key: &str| match object.get(key) {
            None => Err(format!("is missing '{key}'; a link is {SHAPE}")),
            Some(serde_json::Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(format!("has a '{key}' that is not a string")),
        };
        let link = LinkValue {
            title: part("title")?,
            url: part("url")?,
        };
        // Parsing trims and drops whitespace, and the URL is stored as
        // written: one carrying any would not be the URL that was checked.
        if link
            .url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(format!(
                "has a 'url' that contains whitespace: '{}'",
                link.url
            ));
        }
        match url::Url::parse(&link.url) {
            Ok(parsed) if parsed.has_host() => Ok(link),
            _ => Err(format!(
                "has a 'url' that is not an absolute URL (a scheme and a host, e.g. \
                 https://example.com/page): '{}'",
                link.url
            )),
        }
    }

    /// What the link shows as plain text: its title, or its URL when it has
    /// none.
    pub fn label(&self) -> &str {
        match self.title.trim() {
            "" => &self.url,
            title => title,
        }
    }
}

impl std::fmt::Display for SchemaFieldType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SchemaFieldType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(found) = Self::ALL.into_iter().find(|t| t.as_str() == s) {
            return Ok(found);
        }
        let valid = Self::ALL.map(SchemaFieldType::as_str).join(", ");
        if s == "string" {
            return Err(format!(
                "'string' is not a field type; use 'text'. Valid field types: {valid}"
            ));
        }
        Err(format!(
            "unknown field type '{s}'. Valid field types: {valid}"
        ))
    }
}

impl Serialize for SchemaFieldType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SchemaFieldType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaField {
    /// Unique key within the schema: storage/query key, CEL selector,
    /// titleTemplate token. Changing it is a breaking change to every call
    /// site that references the field.
    pub name: String,
    /// Display label shown in every UI surface (table/kanban headers, query
    /// editor, property forms). Always populated in storage — every reader
    /// uses it unconditionally, with no fallback to `description` and no
    /// null-branching. Not required on input to `create_schema`/
    /// `update_schema`: when omitted (empty string), the write boundary
    /// derives it from `name` via [`derive_friendly_name`] before the field
    /// is persisted.
    #[serde(default)]
    pub friendly_name: String,
    #[serde(rename = "type")]
    pub field_type: SchemaFieldType,
    #[serde(default)]
    pub protection: SchemaProtectionLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<EnumValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_values: Option<Vec<EnumValue>>,
    #[serde(default)]
    pub indexed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub default: Option<serde_json::Value>,
    /// What the field is for: meaning, purpose, usage, an example where
    /// helpful. Consumed by the model for schema comprehension (schema
    /// retrieval embeds this text) — NOT rendered as a UI label. Prefer more
    /// detail over less; there is no UI-brevity cost to a longer description
    /// now that [`SchemaField::friendly_name`] carries the display label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The element type of an `array` field, from the same vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_type: Option<SchemaFieldType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<SchemaField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_fields: Option<Vec<SchemaField>>,
    /// Marks this field as a uniqueness hint: values are expected to be unique
    /// among active nodes of the same type. This is a suggest-don't-block rule,
    /// not an enforced constraint — writes are never rejected on a collision
    /// (two offline devices can each validly create the same value). Uniqueness
    /// is scoped per-database (ADR-053) and surfaced via a read-only lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique: Option<bool>,
    /// When paired with `unique`, compares values case-insensitively (e.g. an
    /// email is a claim, not an identity key, and casing should not distinguish
    /// two otherwise-identical claims).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique_case_insensitive: Option<bool>,
    /// Marks this property as machine-bound. A `localOnly` property is persisted
    /// and read locally like any other — normal for local reads, writes, and the
    /// UI — but is never included in a sync push, and is ignored if it arrives in
    /// a pull. It survives its own device's restarts and is simply absent on other
    /// devices (never a stale value from elsewhere). Use it when a value denotes
    /// state on a particular machine, such that transporting it means nothing or
    /// something false elsewhere (a resume handle, an absolute path, a device id,
    /// a local port), or when the content is not safe to transport as-is. Enforced
    /// by the sync engine, which consults this classification when building the
    /// push payload and when applying a pull.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub local_only: bool,
}

/// Serde `skip_serializing_if` helper: omit a `bool` field when it is `false`,
/// so the flag only appears in serialized schemas where it is actually set.
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: SchemaFieldType,
    /// The closed set of values an `enum` edge field admits, each with a display
    /// label. Required on an `enum` field and rejected on any other type.
    ///
    /// Deliberately narrower than [`SchemaField`], which also carries
    /// `user_values` and `extensible`: an edge enum is a fixed vocabulary. The
    /// motivating case is an access-control role on an edge (owner/editor/viewer),
    /// where a user-extensible value set would mean a permission level nothing
    /// downstream knows how to check. Add the extensible half only if a real
    /// use case for it appears.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<EnumValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum RelationshipDirection {
    Out,
    In,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum RelationshipCardinality {
    One,
    Many,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaRelationship {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_type: Option<String>,
    pub direction: RelationshipDirection,
    pub cardinality: RelationshipCardinality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// The name this edge reads by from the target's end. REQUIRED.
    ///
    /// A relationship is declared once — the storage model keeps a single
    /// `relationship` row between the two schema nodes — but it is read from
    /// both ends. Leaving this unset left that one stored edge only
    /// half-declared: the target's side fell back to a synthesized
    /// `"{SourceType} ({Relationship Name})"` label, so an invoice declaring
    /// `billed_to → customer` surfaced on the customer as
    /// "Invoice (Customer)" rather than "Invoices".
    ///
    /// Naming the inverse is a modeling decision only the author can make, so
    /// it is required rather than derived. See the type-level note on
    /// [`SchemaRelationship::reverse_cardinality`] for why both live in the
    /// type rather than in a validator alone.
    pub reverse_name: String,
    /// The cardinality governing the target's end — how many sources may point
    /// at one target. REQUIRED, and the counterpart to
    /// [`SchemaRelationship::reverse_name`].
    ///
    /// Unset, the inbound group carried no cardinality at all, so nothing
    /// downstream could reason about how many sources may point at a node.
    ///
    /// Carrying both halves in the type — not merely in a validator — is what
    /// makes "every stored edge is named from both ends" an invariant every
    /// reader can rely on instead of a convention that holds only on the paths
    /// which happen to route through validation. `handle_create_schema` and
    /// `handle_update_schema` reject a payload missing either field before
    /// serde sees it, so a caller gets an actionable error naming the field
    /// rather than a bare `missing field` message.
    pub reverse_cardinality: RelationshipCardinality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_fields: Option<Vec<EdgeField>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Which children a type's nodes may have, as a schema declares it (ADR-089).
///
/// A named type covers its subtypes. A subtype inherits its base's rule and
/// may only tighten it: `any` declares nothing, and an `any_except` list adds
/// to the base's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "rule", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchemaChildrenRule {
    #[default]
    Any,
    None,
    /// Any child except these types and their subtypes.
    AnyExcept {
        types: Vec<String>,
    },
}

/// Where a type's nodes may sit in the `has_child` tree, as a schema declares
/// it (ADR-089). A named type covers its subtypes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "rule", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchemaParentRule {
    #[default]
    Any,
    MustBeRoot,
    /// Only under one of these types or their subtypes.
    MustHaveParentOf {
        types: Vec<String>,
    },
}

impl SchemaChildrenRule {
    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    /// The types the rule names.
    pub fn named_types(&self) -> &[String] {
        match self {
            Self::AnyExcept { types } => types,
            Self::Any | Self::None => &[],
        }
    }

    /// Whether declaring this rule under a base whose rule in force is `base`
    /// only tightens it. An `any_except` list adds to the base's, so the one
    /// declaration that reads as a relaxation is a list under a base that
    /// refuses every child.
    pub fn tightens(&self, base: &Self) -> bool {
        !matches!((base, self), (Self::None, Self::AnyExcept { .. }))
    }

    /// The rule in force for a type declaring this rule under a base whose
    /// rule in force is `base`.
    pub fn over(&self, base: &Self) -> Self {
        match (base, self) {
            (Self::None, _) | (_, Self::None) => Self::None,
            (Self::Any, own) => own.clone(),
            (base, Self::Any) => base.clone(),
            (Self::AnyExcept { types: inherited }, Self::AnyExcept { types: own }) => {
                let mut types = inherited.clone();
                for t in own {
                    if !types.contains(t) {
                        types.push(t.clone());
                    }
                }
                Self::AnyExcept { types }
            }
        }
    }
}

impl SchemaParentRule {
    pub fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    /// The types the rule names.
    pub fn named_types(&self) -> &[String] {
        match self {
            Self::MustHaveParentOf { types } => types,
            Self::Any | Self::MustBeRoot => &[],
        }
    }

    /// Whether declaring this rule under a base whose rule in force is `base`
    /// only tightens it. A `must_have_parent_of` list may only narrow: each
    /// type it names must be one the base names or a subtype of one, which
    /// `is_a(type, base_type)` answers.
    pub fn tightens(&self, base: &Self, is_a: impl Fn(&str, &str) -> bool) -> bool {
        match (base, self) {
            (_, Self::Any) | (Self::Any, _) => true,
            (Self::MustBeRoot, Self::MustBeRoot) => true,
            (
                Self::MustHaveParentOf { types: allowed },
                Self::MustHaveParentOf { types: named },
            ) => named
                .iter()
                .all(|t| allowed.iter().any(|base| is_a(t, base))),
            (Self::MustBeRoot, Self::MustHaveParentOf { .. })
            | (Self::MustHaveParentOf { .. }, Self::MustBeRoot) => false,
        }
    }

    /// The rule in force for a type declaring this rule under a base whose
    /// rule in force is `base`: the nearest declaration, since a subtype's can
    /// only be tighter.
    pub fn over(&self, base: &Self) -> Self {
        if self.is_any() {
            base.clone()
        } else {
            self.clone()
        }
    }
}

fn type_ids(types: &[CoreNodeType]) -> Vec<String> {
    types.iter().map(|t| t.as_str().to_string()).collect()
}

impl From<ChildrenRule> for SchemaChildrenRule {
    fn from(rule: ChildrenRule) -> Self {
        match rule {
            ChildrenRule::Any => Self::Any,
            ChildrenRule::None => Self::None,
            ChildrenRule::AnyExcept(types) => Self::AnyExcept {
                types: type_ids(types),
            },
        }
    }
}

impl From<ParentRule> for SchemaParentRule {
    fn from(rule: ParentRule) -> Self {
        match rule {
            ParentRule::Any => Self::Any,
            ParentRule::MustBeRoot => Self::MustBeRoot,
            ParentRule::MustHaveParentOf(types) => Self::MustHaveParentOf {
                types: type_ids(types),
            },
        }
    }
}

/// A schema: the definition of a node type (ADR-086 §1).
///
/// The one wire shape of a schema, on every surface. The store fills it: the
/// fields, the structural rules and the templates come from the schema node's
/// row, and `relationships` and `extends` from the schema's declaration edges
/// in the `relationship` table (ADR-070, ADR-078). Every schema read returns
/// one the store built from both, so a `SchemaNode` a reader receives always
/// carries its relationships and its parent.
///
/// A schema's description is not a field: it is the schema node's child
/// subtree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct SchemaNode {
    /// The fields every node carries. A schema's stored properties are all
    /// typed fields below, so `properties` here is empty. `id` is the type id
    /// (`task`, `invoice`) and `content` the type's display name.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    #[serde(default)]
    pub is_core: bool,
    /// An abstract type is never instantiated: no node is created with it as
    /// its `node_type` or retyped into it. It stays a valid `extends` target
    /// and query scope (ADR-086 §6).
    #[serde(default, rename = "abstract", skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub is_abstract: bool,
    /// The schema id of the type this one extends (ADR-078). Stored as the
    /// schema's `extends` edge, never as an entry in `relationships`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Which children this type's nodes may have: the rule this type itself
    /// declares, on top of what it inherits (ADR-089).
    #[serde(default, skip_serializing_if = "SchemaChildrenRule::is_any")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub children: SchemaChildrenRule,
    /// Where this type's nodes may sit in the tree: the rule this type itself
    /// declares, on top of what it inherits (ADR-089).
    #[serde(default, skip_serializing_if = "SchemaParentRule::is_any")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub parent: SchemaParentRule,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// The fields this schema itself declares. A read of one schema's
    /// definition reports the effective set instead: these, then the ones
    /// inherited through `extends`.
    #[serde(default)]
    pub fields: Vec<SchemaField>,
    /// The relationships this schema declares to other types, stored as
    /// declaration edges between schema nodes (ADR-070). A read of one
    /// schema's definition adds the inherited ones, like `fields`.
    #[serde(default)]
    pub relationships: Vec<SchemaRelationship>,
    /// Template for a node's indexed title, with `{field_name}` tokens, e.g.
    /// `"{first_name} {last_name}"`. When set, the title is interpolated from
    /// the node's fields rather than taken from its content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_template: Option<String>,
    /// Template for the property summary shown under a node's title, in the
    /// same `{field_name}` syntax. Evaluated by the client and never stored
    /// on a node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties_header_summary_template: Option<String>,
    /// The paths from a node of this type to the nodes that govern it: what
    /// a context read follows when it is given no paths (ADR-094 §2). The
    /// ones this schema itself declares; a type's context paths are its
    /// ancestors' and then its own, and a read of one schema's definition
    /// reports that set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub context_paths: Vec<RelationshipPath>,
}

impl SchemaNode {
    /// A user-defined schema with nothing declared: the base a caller fills
    /// in with struct-update syntax.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            envelope: NodeEnvelope::new_with_id(
                id.into(),
                CoreNodeType::Schema.as_str().to_string(),
                name.into(),
                serde_json::json!({}),
            ),
            is_core: false,
            is_abstract: false,
            extends: None,
            children: SchemaChildrenRule::default(),
            parent: SchemaParentRule::default(),
            schema_version: default_schema_version(),
            fields: Vec::new(),
            relationships: Vec::new(),
            title_template: None,
            properties_header_summary_template: None,
            context_paths: Vec::new(),
        }
    }

    /// The field this schema itself declares under `name`.
    pub fn get_field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Every value of an enum field, core values first. `None` when the
    /// schema declares no such field or it is not an enum.
    pub fn get_enum_values(&self, field_name: &str) -> Option<Vec<EnumValue>> {
        let field = self.get_field(field_name)?;
        if field.field_type != SchemaFieldType::Enum {
            return None;
        }
        Some(
            field
                .core_values
                .iter()
                .chain(&field.user_values)
                .flatten()
                .cloned()
                .collect(),
        )
    }

    /// The value strings of an enum field, without their labels.
    pub fn get_enum_value_strings(&self, field_name: &str) -> Option<Vec<String>> {
        self.get_enum_values(field_name)
            .map(|values| values.into_iter().map(|v| v.value).collect())
    }

    /// Whether the field may be removed: only a `User`-protected field may.
    pub fn can_delete_field(&self, field_name: &str) -> bool {
        self.is_user_field(field_name)
    }

    /// Whether the field may be renamed or relabelled: only a
    /// `User`-protected field may. Core and System fields are immutable.
    pub fn can_modify_field(&self, field_name: &str) -> bool {
        self.is_user_field(field_name)
    }

    fn is_user_field(&self, field_name: &str) -> bool {
        self.get_field(field_name)
            .is_some_and(|f| f.protection == SchemaProtectionLevel::User)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The wire schema carries the envelope, and `abstract` and `extends` are
    /// serialized only when declared.
    #[test]
    fn test_wire_schema_carries_the_envelope_abstract_and_extends() {
        let schema = SchemaNode {
            is_abstract: true,
            extends: Some("task".to_string()),
            ..SchemaNode::new("issue", "Issue")
        };

        let wire = serde_json::to_value(&schema).unwrap();
        assert_eq!(wire["id"], "issue");
        assert_eq!(wire["nodeType"], "schema");
        assert_eq!(wire["lifecycleStatus"], "active");
        assert_eq!(wire["properties"], json!({}));
        assert_eq!(wire["abstract"], true);
        assert_eq!(wire["extends"], "task");
        assert_eq!(wire["relationships"], json!([]));
        assert!(wire.get("description").is_none());

        let wire = serde_json::to_value(SchemaNode::new("invoice", "Invoice")).unwrap();
        assert!(wire.get("abstract").is_none());
        assert!(wire.get("extends").is_none());
    }

    /// A schema read off the wire is the schema that was sent: relationships,
    /// parent, rules and templates included.
    #[test]
    fn test_wire_schema_round_trips() {
        let schema = SchemaNode {
            extends: Some("task".to_string()),
            children: SchemaChildrenRule::None,
            parent: SchemaParentRule::MustHaveParentOf {
                types: vec!["thread".to_string()],
            },
            fields: vec![create_test_field()],
            relationships: vec![SchemaRelationship {
                name: "billed_to".to_string(),
                target_type: Some("customer".to_string()),
                direction: RelationshipDirection::Out,
                cardinality: RelationshipCardinality::One,
                required: None,
                reverse_name: "invoices".to_string(),
                reverse_cardinality: RelationshipCardinality::Many,
                edge_fields: None,
                description: None,
            }],
            title_template: Some("{status}".to_string()),
            properties_header_summary_template: Some("{status}".to_string()),
            context_paths: vec![
                RelationshipPath::from_names(["billed_to"]),
                "child_of*.project".parse().unwrap(),
            ],
            ..SchemaNode::new("issue", "Issue")
        };

        let wire = serde_json::to_value(&schema).unwrap();
        assert_eq!(
            wire["contextPaths"],
            json!([["billed_to"], [{ "name": "child_of", "open_ended": true }, "project"]])
        );
        assert_eq!(wire["children"], json!({ "rule": "none" }));
        assert_eq!(
            wire["parent"],
            json!({ "rule": "must_have_parent_of", "types": ["thread"] })
        );
        assert_eq!(wire["titleTemplate"], "{status}");
        assert_eq!(wire["relationships"][0]["name"], "billed_to");

        let read: SchemaNode = serde_json::from_value(wire).unwrap();
        assert_eq!(read.envelope, schema.envelope);
        assert_eq!(read.extends, schema.extends);
        assert_eq!(read.children, schema.children);
        assert_eq!(read.parent, schema.parent);
        assert_eq!(read.relationships, schema.relationships);
        assert_eq!(read.fields.len(), 1);
        assert_eq!(read.title_template, schema.title_template);
        assert_eq!(read.context_paths, schema.context_paths);

        // `any` is not serialized, and neither is an empty list of paths.
        let wire = serde_json::to_value(SchemaNode::new("invoice", "Invoice")).unwrap();
        assert!(wire.get("children").is_none());
        assert!(wire.get("parent").is_none());
        assert!(wire.get("contextPaths").is_none());
    }

    #[test]
    fn test_enum_values_and_field_protection() {
        let schema = SchemaNode {
            fields: vec![
                create_test_field(),
                SchemaField {
                    name: "notes".to_string(),
                    ..Default::default()
                },
            ],
            ..SchemaNode::new("ticket", "Ticket")
        };

        let values: Vec<String> = schema
            .get_enum_values("status")
            .unwrap()
            .into_iter()
            .map(|v| v.value)
            .collect();
        assert_eq!(values, ["open", "done", "blocked"]);
        assert!(schema.get_enum_values("notes").is_none());
        assert!(schema.get_enum_values("missing").is_none());

        // Only a User-protected field may be removed or changed.
        assert!(!schema.can_delete_field("status"));
        assert!(!schema.can_modify_field("status"));
        assert!(schema.can_delete_field("notes"));
        assert!(schema.can_modify_field("notes"));
        assert!(!schema.can_delete_field("missing"));
    }

    #[test]
    fn test_a_structural_rule_rejects_an_unknown_shape() {
        assert!(serde_json::from_value::<SchemaChildrenRule>(json!({ "rule": "some" })).is_err());
        assert!(
            serde_json::from_value::<SchemaChildrenRule>(json!({ "rule": "any_except" })).is_err()
        );
        assert!(serde_json::from_value::<SchemaParentRule>(json!({ "rule": "anywhere" })).is_err());
        assert_eq!(
            serde_json::from_value::<SchemaChildrenRule>(
                json!({ "rule": "any_except", "types": ["collection"] })
            )
            .unwrap(),
            SchemaChildrenRule::AnyExcept {
                types: vec!["collection".to_string()]
            }
        );
    }

    fn except(types: &[&str]) -> SchemaChildrenRule {
        SchemaChildrenRule::AnyExcept {
            types: types.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn parent_of(types: &[&str]) -> SchemaParentRule {
        SchemaParentRule::MustHaveParentOf {
            types: types.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// `issue extends task`, for the tests below.
    fn is_a(node_type: &str, base: &str) -> bool {
        node_type == base || (node_type == "issue" && base == "task")
    }

    #[test]
    fn test_a_subtype_rule_only_tightens() {
        use SchemaChildrenRule as C;
        use SchemaParentRule as P;

        // `any` on a subtype declares nothing: it inherits.
        assert!(C::Any.tightens(&C::None));
        assert!(C::None.tightens(&C::Any));
        assert!(except(&["task"]).tightens(&C::Any));
        assert!(C::None.tightens(&except(&["task"])));
        assert!(except(&["person"]).tightens(&except(&["task"])));
        assert!(!except(&["task"]).tightens(&C::None));

        assert!(P::Any.tightens(&P::MustBeRoot, is_a));
        assert!(P::MustBeRoot.tightens(&P::Any, is_a));
        assert!(parent_of(&["task"]).tightens(&P::Any, is_a));
        assert!(P::MustBeRoot.tightens(&P::MustBeRoot, is_a));
        assert!(!parent_of(&["task"]).tightens(&P::MustBeRoot, is_a));
        assert!(!P::MustBeRoot.tightens(&parent_of(&["task"]), is_a));
        // A list may narrow to a subtype of a type the base names, not widen.
        assert!(parent_of(&["issue"]).tightens(&parent_of(&["task"]), is_a));
        assert!(!parent_of(&["task"]).tightens(&parent_of(&["issue"]), is_a));
        assert!(!parent_of(&["task", "person"]).tightens(&parent_of(&["task"]), is_a));
    }

    #[test]
    fn test_the_rule_in_force_composes_down_the_chain() {
        use SchemaChildrenRule as C;
        use SchemaParentRule as P;

        assert_eq!(C::Any.over(&C::None), C::None);
        assert_eq!(C::None.over(&except(&["task"])), C::None);
        assert_eq!(C::Any.over(&except(&["task"])), except(&["task"]));
        assert_eq!(
            except(&["person", "task"]).over(&except(&["task"])),
            except(&["task", "person"])
        );

        assert_eq!(P::Any.over(&P::MustBeRoot), P::MustBeRoot);
        assert_eq!(
            parent_of(&["issue"]).over(&parent_of(&["task"])),
            parent_of(&["issue"])
        );
    }

    /// The registry's own subtypes obey the rule every schema is held to.
    #[test]
    fn test_no_core_subtype_relaxes_its_bases_structural_rules() {
        let core_is_a =
            |t: &str, base: &str| match (CoreNodeType::from_id(t), CoreNodeType::from_id(base)) {
                (Some(t), Some(base)) => t.is_a(base),
                _ => false,
            };
        for t in CoreNodeType::ALL {
            let Some(parent) = t.parent() else { continue };
            let base = parent.structure();
            let own = t.declared_structure();
            assert!(
                SchemaChildrenRule::from(own.children).tightens(&base.children.into()),
                "{t} relaxes the children rule of {parent}"
            );
            assert!(
                SchemaParentRule::from(own.parent).tightens(&base.parent.into(), core_is_a),
                "{t} relaxes the parent rule of {parent}"
            );
        }
    }

    #[test]
    fn test_field_type_vocabulary_is_closed() {
        for field_type in SchemaFieldType::ALL {
            let parsed: SchemaFieldType =
                serde_json::from_value(json!(field_type.as_str())).unwrap();
            assert_eq!(parsed, field_type);
            assert_eq!(
                serde_json::to_value(field_type).unwrap(),
                json!(field_type.as_str())
            );
        }
        let error = serde_json::from_value::<SchemaFieldType>(json!("string"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("use 'text'"), "{error}");
        assert!(serde_json::from_value::<SchemaFieldType>(json!("varchar")).is_err());
    }

    #[test]
    fn test_link_value_shape_is_closed() {
        let link =
            LinkValue::from_json(&json!({ "title": "Core", "url": "https://github.com/a/b" }))
                .unwrap();
        assert_eq!(link.title, "Core");
        assert_eq!(link.label(), "Core");
        // The scheme is not restricted where a link is stored.
        let untitled =
            LinkValue::from_json(&json!({ "title": " ", "url": "ssh://git@host/a.git" })).unwrap();
        assert_eq!(untitled.label(), "ssh://git@host/a.git");

        let refused = |value: serde_json::Value| LinkValue::from_json(&value).unwrap_err();
        assert!(refused(json!("https://example.com")).contains("received the string"));
        assert!(refused(json!({ "url": "https://example.com" })).contains("missing 'title'"));
        assert!(refused(json!({ "title": "t" })).contains("missing 'url'"));
        assert!(
            refused(json!({ "title": "t", "url": "https://example.com", "label": "x" }))
                .contains("unknown key 'label'")
        );
        assert!(refused(json!({ "title": 1, "url": "https://example.com" }))
            .contains("'title' that is not a string"));
        for relative in ["example.com/page", "/page", "mailto:a@example.com", ""] {
            assert!(
                refused(json!({ "title": "t", "url": relative })).contains("not an absolute URL"),
                "{relative}"
            );
        }

        for spaced in [
            " https://example.com",
            "https://example.com/a\n",
            "https://example.com/a b",
            "https://exa\tmple.com",
        ] {
            assert!(
                refused(json!({ "title": "t", "url": spaced })).contains("contains whitespace"),
                "{spaced:?}"
            );
        }

        // The serde shape is the same closed one.
        assert!(serde_json::from_value::<LinkValue>(
            json!({ "title": "t", "url": "https://example.com", "label": "x" })
        )
        .is_err());
    }

    fn create_test_field() -> SchemaField {
        SchemaField {
            name: "status".to_string(),
            friendly_name: "Status".to_string(),
            field_type: SchemaFieldType::Enum,
            protection: SchemaProtectionLevel::Core,
            local_only: false,
            core_values: Some(vec![
                EnumValue::new("open".to_string(), "Open".to_string()),
                EnumValue::new("done".to_string(), "Done".to_string()),
            ]),
            user_values: Some(vec![EnumValue::new(
                "blocked".to_string(),
                "Blocked".to_string(),
            )]),
            indexed: true,
            required: Some(true),
            extensible: Some(true),
            default: Some(json!("open")),
            description: Some("Task status".to_string()),
            item_type: None,
            fields: None,
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
        }
    }

    #[test]
    fn test_schema_field_serialization() {
        let field = create_test_field();
        let json = serde_json::to_value(&field).unwrap();

        assert_eq!(json["name"], "status");
        assert_eq!(json["friendlyName"], "Status");
        assert_eq!(json["protection"], "core");
        // field_type serializes to "type" due to #[serde(rename = "type")]
        assert_eq!(json["type"], "enum");
        // core_values serializes to coreValues
        assert!(json["coreValues"].is_array());
        assert_eq!(json["indexed"], true);
    }

    #[test]
    fn test_schema_field_friendly_name_defaults_to_empty_when_omitted() {
        // The write boundary (create_schema/update_schema) is the only place
        // friendly_name gets derived from `name` — the bare wire type accepts
        // an absent friendlyName as "" so a caller (including the agent) is
        // never forced to supply it, and never rejected for omitting it.
        let json = json!({
            "name": "due_date",
            "type": "date",
        });
        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.friendly_name, "");
    }

    #[test]
    fn test_schema_field_friendly_name_round_trips() {
        let json = json!({
            "name": "due_date",
            "friendlyName": "Due date",
            "type": "date",
        });
        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.friendly_name, "Due date");

        let out = serde_json::to_value(&field).unwrap();
        assert_eq!(out["friendlyName"], "Due date");
    }

    #[test]
    fn test_derive_friendly_name_snake_case() {
        assert_eq!(derive_friendly_name("due_date"), "Due date");
        assert_eq!(derive_friendly_name("started_at"), "Started at");
        assert_eq!(derive_friendly_name("status"), "Status");
    }

    #[test]
    fn test_derive_friendly_name_strips_namespace_prefix() {
        // ADR-063: a prefix only ever appears on a field added to a core
        // type; stripping it for display cannot collide with the storage key
        // of a bare core field (see derive_friendly_name's doc comment).
        assert_eq!(derive_friendly_name("custom:capacity"), "Capacity");
        assert_eq!(derive_friendly_name("org:cost_center"), "Cost center");
    }

    #[test]
    fn test_derive_friendly_name_splits_camel_case() {
        assert_eq!(derive_friendly_name("estimatedHours"), "Estimated hours");
    }

    #[test]
    fn test_derive_friendly_name_hyphenated() {
        assert_eq!(
            derive_friendly_name("capture-session-id"),
            "Capture session id"
        );
    }

    #[test]
    fn test_derive_friendly_name_empty_falls_back_to_raw_name() {
        assert_eq!(derive_friendly_name(""), "");
        assert_eq!(derive_friendly_name(":"), ":");
    }

    #[test]
    fn test_derive_friendly_name_prefix_with_empty_base_falls_back_to_raw_name() {
        // Unreachable in practice (the field-name validator rejects an empty
        // bare segment before this ever runs), but pinned explicitly so the
        // fallback behavior is documented rather than incidental.
        assert_eq!(derive_friendly_name("custom:"), "custom:");
    }

    #[test]
    fn test_derive_friendly_name_splits_acronym_adjacent_to_next_word() {
        // The classic "XMLHttpRequest" splitting case: an acronym run
        // (`ID`) directly followed by another capitalized word (`Number`)
        // must not merge into one unsplit blob ("Idnumber").
        assert_eq!(
            derive_friendly_name("employeeIDNumber"),
            "Employee id number"
        );
        assert_eq!(derive_friendly_name("userIDStatus"), "User id status");
    }

    #[test]
    fn test_derive_friendly_name_all_uppercase_is_treated_as_one_word() {
        // No lowercase run anywhere to anchor a boundary against, so this is
        // one word, sentence-cased like every other single-word input.
        assert_eq!(derive_friendly_name("URL"), "Url");
    }

    #[test]
    fn test_schema_field_deserialization() {
        let json = json!({
            "name": "status",
            "type": "enum",
            "protection": "core",
            "coreValues": [
                { "value": "open", "label": "Open" },
                { "value": "done", "label": "Done" }
            ],
            "indexed": true
        });

        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "status");
        assert_eq!(field.field_type, SchemaFieldType::Enum);
        assert_eq!(field.protection, SchemaProtectionLevel::Core);
        assert!(field.indexed);

        let core_values = field.core_values.unwrap();
        assert_eq!(core_values.len(), 2);
        assert_eq!(core_values[0].value, "open");
        assert_eq!(core_values[0].label, "Open");
    }

    #[test]
    fn test_schema_field_rejects_snake_case_core_values() {
        // core_values is the Rust field name; the wire key is coreValues
        // (rename_all = "camelCase"). A payload using the snake_case name must
        // be rejected outright, not silently dropped as an unknown field.
        let json = json!({
            "name": "status",
            "type": "enum",
            "core_values": [
                { "value": "open", "label": "Open" },
                { "value": "done", "label": "Done" }
            ]
        });

        let err = serde_json::from_value::<SchemaField>(json).unwrap_err();
        assert!(
            err.to_string().contains("core_values"),
            "expected error naming the unknown field `core_values`, got: {}",
            err
        );
    }

    #[test]
    fn test_protection_level_serialization() {
        assert_eq!(
            serde_json::to_value(SchemaProtectionLevel::Core).unwrap(),
            "core"
        );
        assert_eq!(
            serde_json::to_value(SchemaProtectionLevel::User).unwrap(),
            "user"
        );
        assert_eq!(
            serde_json::to_value(SchemaProtectionLevel::System).unwrap(),
            "system"
        );
    }

    #[test]
    fn test_nested_field_serialization() {
        let address_field = SchemaField {
            name: "address".to_string(),
            friendly_name: "Address".to_string(),
            field_type: SchemaFieldType::Object,
            protection: SchemaProtectionLevel::User,
            local_only: false,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Address information".to_string()),
            item_type: None,
            fields: Some(vec![
                SchemaField {
                    name: "street".to_string(),
                    friendly_name: "Street".to_string(),
                    field_type: SchemaFieldType::Text,
                    protection: SchemaProtectionLevel::User,
                    local_only: false,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Street address".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "city".to_string(),
                    friendly_name: "City".to_string(),
                    field_type: SchemaFieldType::Text,
                    protection: SchemaProtectionLevel::User,
                    local_only: false,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("City".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
            ]),
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
        };

        let json = serde_json::to_value(&address_field).unwrap();
        assert_eq!(json["name"], "address");
        assert_eq!(json["type"], "object");
        assert_eq!(json["fields"][0]["name"], "street");
        assert_eq!(json["fields"][1]["name"], "city");
        assert_eq!(json["fields"][1]["indexed"], true);
    }

    #[test]
    fn test_nested_field_deserialization() {
        let json = json!({
            "name": "address",
            "type": "object",
            "protection": "user",
            "indexed": false,
            "fields": [
                {
                    "name": "city",
                    "type": "text",
                    "protection": "user",
                    "indexed": true
                }
            ]
        });

        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "address");
        assert_eq!(field.field_type, SchemaFieldType::Object);

        let nested_fields = field.fields.as_ref().unwrap();
        assert_eq!(nested_fields.len(), 1);
        assert_eq!(nested_fields[0].name, "city");
        assert!(nested_fields[0].indexed);
    }

    #[test]
    fn test_array_of_objects_serialization() {
        let contacts_field = SchemaField {
            name: "contacts".to_string(),
            friendly_name: "Contacts".to_string(),
            field_type: SchemaFieldType::Array,
            protection: SchemaProtectionLevel::User,
            local_only: false,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: None,
            description: Some("Contact list".to_string()),
            item_type: Some(SchemaFieldType::Object),
            fields: None,
            item_fields: Some(vec![SchemaField {
                name: "email".to_string(),
                friendly_name: "Email".to_string(),
                field_type: SchemaFieldType::Text,
                protection: SchemaProtectionLevel::User,
                local_only: false,
                core_values: None,
                user_values: None,
                indexed: true,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Email address".to_string()),
                item_type: None,
                fields: None,
                item_fields: None,
                unique: None,
                unique_case_insensitive: None,
            }]),
            unique: None,
            unique_case_insensitive: None,
        };

        let json = serde_json::to_value(&contacts_field).unwrap();
        assert_eq!(json["name"], "contacts");
        assert_eq!(json["type"], "array");
        // item_type serializes to itemType with camelCase
        assert_eq!(json["itemType"], "object");
        // item_fields serializes to itemFields with camelCase
        assert_eq!(json["itemFields"][0]["name"], "email");
        assert_eq!(json["itemFields"][0]["indexed"], true);
    }

    #[test]
    fn test_edge_field_serialization() {
        let field = EdgeField {
            name: "role".to_string(),
            field_type: SchemaFieldType::Text,
            core_values: None,
            indexed: Some(true),
            required: Some(false),
            default: Some(json!("member")),
            target_type: None,
            description: Some("Assignment role".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["name"], "role");
        assert_eq!(json["type"], "text");
        assert_eq!(json["indexed"], true);
        assert_eq!(json["required"], false);
        assert_eq!(json["default"], "member");
        assert_eq!(json["description"], "Assignment role");
        // target_type should be absent (skip_serializing_if = None)
        assert!(json.get("targetType").is_none());
    }

    #[test]
    fn test_edge_field_deserialization() {
        let json = json!({
            "name": "billing_date",
            "type": "date",
            "required": true,
            "indexed": true
        });

        let field: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "billing_date");
        assert_eq!(field.field_type, SchemaFieldType::Date);
        assert_eq!(field.required, Some(true));
        assert_eq!(field.indexed, Some(true));
        assert!(field.default.is_none());
        assert!(field.target_type.is_none());
        assert!(field.description.is_none());
    }

    /// An edge field's type is the node field-type vocabulary: a type outside
    /// it is refused when the declaration is read, and `string` is told to use
    /// `text`.
    #[test]
    fn test_edge_field_type_outside_the_vocabulary_is_refused() {
        for field_type in SchemaFieldType::ALL {
            let field: EdgeField =
                serde_json::from_value(json!({ "name": "f", "type": field_type.as_str() }))
                    .unwrap();
            assert_eq!(field.field_type, field_type);
        }

        let unknown =
            serde_json::from_value::<EdgeField>(json!({ "name": "approved_by", "type": "record" }))
                .unwrap_err()
                .to_string();
        assert!(unknown.contains("unknown field type 'record'"), "{unknown}");

        let string = serde_json::from_value::<EdgeField>(json!({ "name": "f", "type": "string" }))
            .unwrap_err()
            .to_string();
        assert!(string.contains("use 'text'"), "{string}");
    }

    #[test]
    fn test_edge_field_minimal() {
        // Test minimal edge field (only required fields)
        let json = json!({
            "name": "simple",
            "type": "text"
        });

        let field: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "simple");
        assert_eq!(field.field_type, SchemaFieldType::Text);
        assert!(field.indexed.is_none());
        assert!(field.required.is_none());
        assert!(field.default.is_none());
        assert!(field.core_values.is_none());
    }

    #[test]
    fn test_edge_field_enum_round_trip() {
        // The RBAC-style case: a closed role vocabulary declared on the edge.
        // Serializing must emit `coreValues` (camelCase) with value+label, and
        // deserializing the same JSON must reproduce the field exactly — this is
        // the property that fails if `core_values` is ever dropped at the
        // conversion boundary.
        let field = EdgeField {
            name: "role".to_string(),
            field_type: SchemaFieldType::Enum,
            core_values: Some(vec![
                EnumValue::new("owner".to_string(), "Owner".to_string()),
                EnumValue::new("editor".to_string(), "Editor".to_string()),
                EnumValue::new("viewer".to_string(), "Viewer".to_string()),
            ]),
            indexed: Some(true),
            required: Some(true),
            default: Some(json!("viewer")),
            target_type: None,
            description: Some("Access level on this membership".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["type"], "enum");
        // snake_case `core_values` must serialize to the camelCase wire key
        assert!(json.get("core_values").is_none());
        assert_eq!(json["coreValues"].as_array().unwrap().len(), 3);
        assert_eq!(json["coreValues"][0]["value"], "owner");
        assert_eq!(json["coreValues"][0]["label"], "Owner");
        assert_eq!(json["default"], "viewer");

        let round_tripped: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, field);
    }

    #[test]
    fn test_edge_field_omits_core_values_when_absent() {
        // A non-enum edge field must not gain an empty `coreValues` key.
        let field = EdgeField {
            name: "billing_date".to_string(),
            field_type: SchemaFieldType::Date,
            core_values: None,
            indexed: None,
            required: None,
            default: None,
            target_type: None,
            description: None,
        };

        let json = serde_json::to_value(&field).unwrap();
        assert!(json.get("coreValues").is_none());
    }

    #[test]
    fn test_relationship_direction_serialization() {
        assert_eq!(
            serde_json::to_value(RelationshipDirection::Out).unwrap(),
            "out"
        );
        assert_eq!(
            serde_json::to_value(RelationshipDirection::In).unwrap(),
            "in"
        );
    }

    #[test]
    fn test_relationship_direction_deserialization() {
        let out: RelationshipDirection = serde_json::from_value(json!("out")).unwrap();
        assert_eq!(out, RelationshipDirection::Out);

        let r#in: RelationshipDirection = serde_json::from_value(json!("in")).unwrap();
        assert_eq!(r#in, RelationshipDirection::In);
    }

    #[test]
    fn test_relationship_cardinality_serialization() {
        assert_eq!(
            serde_json::to_value(RelationshipCardinality::One).unwrap(),
            "one"
        );
        assert_eq!(
            serde_json::to_value(RelationshipCardinality::Many).unwrap(),
            "many"
        );
    }

    #[test]
    fn test_relationship_cardinality_deserialization() {
        let one: RelationshipCardinality = serde_json::from_value(json!("one")).unwrap();
        assert_eq!(one, RelationshipCardinality::One);

        let many: RelationshipCardinality = serde_json::from_value(json!("many")).unwrap();
        assert_eq!(many, RelationshipCardinality::Many);
    }

    #[test]
    fn test_schema_relationship_serialization() {
        let relationship = SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: Some(true),
            reverse_name: "invoices".to_string(),
            reverse_cardinality: RelationshipCardinality::Many,
            edge_fields: Some(vec![
                EdgeField {
                    name: "billing_date".to_string(),
                    field_type: SchemaFieldType::Date,
                    core_values: None,
                    indexed: Some(true),
                    required: Some(true),
                    default: None,
                    target_type: None,
                    description: None,
                },
                EdgeField {
                    name: "payment_terms".to_string(),
                    field_type: SchemaFieldType::Text,
                    core_values: None,
                    indexed: None,
                    required: None,
                    default: Some(json!("net-30")),
                    target_type: None,
                    description: None,
                },
            ]),
            description: Some("Customer this invoice is billed to".to_string()),
        };

        let json = serde_json::to_value(&relationship).unwrap();

        assert_eq!(json["name"], "billed_to");
        assert_eq!(json["targetType"], "customer");
        assert_eq!(json["direction"], "out");
        assert_eq!(json["cardinality"], "one");
        assert_eq!(json["required"], true);
        assert_eq!(json["reverseName"], "invoices");
        assert_eq!(json["reverseCardinality"], "many");
        assert_eq!(json["edgeFields"].as_array().unwrap().len(), 2);
        assert_eq!(json["edgeFields"][0]["name"], "billing_date");
        assert_eq!(json["edgeFields"][1]["default"], "net-30");
    }

    #[test]
    fn test_schema_relationship_deserialization() {
        let json = json!({
            "name": "assigned_to",
            "targetType": "person",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "tasks",
            "reverseCardinality": "many",
            "edgeFields": [
                {
                    "name": "role",
                    "type": "text",
                    "indexed": true
                },
                {
                    "name": "assigned_at",
                    "type": "date",
                    "required": true
                }
            ]
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();

        assert_eq!(relationship.name, "assigned_to");
        assert_eq!(relationship.target_type, Some("person".to_string()));
        assert_eq!(relationship.direction, RelationshipDirection::Out);
        assert_eq!(relationship.cardinality, RelationshipCardinality::Many);
        assert_eq!(relationship.reverse_name, "tasks");
        assert_eq!(
            relationship.reverse_cardinality,
            RelationshipCardinality::Many
        );
        assert!(relationship.required.is_none());

        let edge_fields = relationship.edge_fields.unwrap();
        assert_eq!(edge_fields.len(), 2);
        assert_eq!(edge_fields[0].name, "role");
        assert_eq!(edge_fields[1].name, "assigned_at");
    }

    #[test]
    fn test_schema_relationship_minimal() {
        // The smallest legal declaration: the reverse half is part of it, so a
        // "minimal" relationship still names the edge from both ends.
        let json = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "children_of",
            "reverseCardinality": "one"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();

        assert_eq!(relationship.name, "parent_of");
        assert_eq!(relationship.target_type, Some("document".to_string()));
        assert_eq!(relationship.direction, RelationshipDirection::Out);
        assert_eq!(relationship.cardinality, RelationshipCardinality::Many);
        assert_eq!(relationship.reverse_name, "children_of");
        assert_eq!(
            relationship.reverse_cardinality,
            RelationshipCardinality::One
        );
        assert!(relationship.required.is_none());
        assert!(relationship.edge_fields.is_none());
        assert!(relationship.description.is_none());
    }

    /// The reverse half is carried by the type, not merely by a validator.
    ///
    /// The schema-op validators produce the actionable, example-bearing error a
    /// caller acts on; this asserts the floor beneath them — a payload missing
    /// either field cannot become a `SchemaRelationship` at all, so no other
    /// deserialization path can smuggle in a half-named edge.
    #[test]
    fn test_schema_relationship_requires_both_reverse_fields() {
        let missing_name = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many",
            "reverseCardinality": "one"
        });
        let err = serde_json::from_value::<SchemaRelationship>(missing_name)
            .expect_err("reverseName must be required");
        assert!(
            err.to_string().contains("reverseName"),
            "error should name the missing field, got: {err}"
        );

        let missing_cardinality = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "children_of"
        });
        let err = serde_json::from_value::<SchemaRelationship>(missing_cardinality)
            .expect_err("reverseCardinality must be required");
        assert!(
            err.to_string().contains("reverseCardinality"),
            "error should name the missing field, got: {err}"
        );
    }

    /// An unknown or misspelled key must be rejected, not silently dropped —
    /// mirrors [`SchemaField`]'s existing `deny_unknown_fields` coverage.
    /// Two shapes, both genuinely silent pre-fix:
    ///
    /// - a redundant, misspelled key coexisting with the correctly spelled
    ///   one (`reverseName` present and valid, plus a leftover
    ///   `reverse_name`) — every required field is already satisfied, so
    ///   nothing else catches the mistake and the relationship used to be
    ///   created exactly as the caller (correctly) authored it, silently
    ///   dropping the stray key
    /// - a typo of an *optional* field (`target_type` instead of
    ///   `targetType`) — the misspelled key isn't a known field, so it used
    ///   to vanish and `target_type` stayed silently `None` rather than the
    ///   value the caller intended
    ///
    /// A typo of a *required* field with no correctly spelled counterpart
    /// present (e.g. `reverse_name` alone, no `reverseName`) is deliberately
    /// NOT covered here: serde already rejects that case pre-fix too, via the
    /// standard "missing field `reverseName`" error — a real error, just a
    /// differently shaped one than `deny_unknown_fields` produces. See
    /// [`test_schema_relationship_requires_both_reverse_fields`] above for
    /// that floor.
    #[test]
    fn test_schema_relationship_rejects_unknown_field() {
        let redundant_typo = json!({
            "name": "assigned_to",
            "targetType": "person",
            "direction": "out",
            "cardinality": "one",
            "reverseName": "tasks",
            "reverseCardinality": "many",
            "reverse_name": "tasks"
        });
        let err = serde_json::from_value::<SchemaRelationship>(redundant_typo)
            .expect_err("a redundant snake_case key must be rejected, not silently dropped");
        assert!(
            err.to_string().contains("reverse_name"),
            "error should name the offending key, got: {err}"
        );

        let optional_field_typo = json!({
            "name": "assigned_to",
            "target_type": "person",
            "direction": "out",
            "cardinality": "one",
            "reverseName": "tasks",
            "reverseCardinality": "many"
        });
        let err = serde_json::from_value::<SchemaRelationship>(optional_field_typo)
            .expect_err("a typo of an optional field must be rejected, not silently dropped");
        assert!(
            err.to_string().contains("target_type"),
            "error should name the offending key, got: {err}"
        );

        let bogus_key = json!({
            "name": "assigned_to",
            "targetType": "person",
            "direction": "out",
            "cardinality": "one",
            "reverseName": "tasks",
            "reverseCardinality": "many",
            "bogusRel": 1
        });
        let err = serde_json::from_value::<SchemaRelationship>(bogus_key)
            .expect_err("an entirely unknown key must be rejected, not dropped");
        assert!(
            err.to_string().contains("bogusRel"),
            "error should name the offending key, got: {err}"
        );
    }

    #[test]
    fn test_schema_relationship_incoming_direction() {
        // Test "in" direction (less common but valid)
        let json = json!({
            "name": "owned_by",
            "targetType": "organization",
            "direction": "in",
            "cardinality": "one",
            "reverseName": "owns",
            "reverseCardinality": "many"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(relationship.direction, RelationshipDirection::In);
    }

    #[test]
    fn test_schema_relationship_untyped_deserialization() {
        // target_type absent → None (untyped/generic relationship)
        let json = json!({
            "name": "related",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "related_from",
            "reverseCardinality": "many"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(relationship.name, "related");
        assert!(relationship.target_type.is_none());
    }

    #[test]
    fn test_schema_relationship_untyped_serialization() {
        let relationship = SchemaRelationship {
            name: "related".to_string(),
            target_type: None,
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: "related_from".to_string(),
            reverse_cardinality: RelationshipCardinality::Many,
            edge_fields: None,
            description: None,
        };

        let json = serde_json::to_value(&relationship).unwrap();
        assert_eq!(json["name"], "related");
        // targetType absent when None
        assert!(json.get("targetType").is_none());
    }
}
