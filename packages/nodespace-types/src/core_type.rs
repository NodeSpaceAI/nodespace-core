//! The core node type registry (ADR-086 §3).
//!
//! [`CoreNodeType`] has one variant per type NodeSpace itself ships. It is the
//! one list of core types: the seeded schemas, the behaviour registry, the
//! wire conversion and the frontend plugin list are each checked against it,
//! so a layer that forgets a type fails a test instead of drifting.
//!
//! Two kinds of type are deliberately absent: subtypes another build registers
//! on top of a core type, and types a user defines through the schema API.
//! Both resolve to a core type through their `extends` chain
//! ([`CoreNodeType::nearest`]), which is how a base type's rules reach them.

use serde::{Deserialize, Serialize};

/// A type NodeSpace ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum CoreNodeType {
    Text,
    Header,
    CodeBlock,
    QuoteBlock,
    OrderedList,
    Checkbox,
    HorizontalLine,
    Table,
    Date,
    AgentGuidance,
    Task,
    Project,
    Spec,
    Plan,
    Decision,
    Person,
    Collection,
    Skill,
    DatabaseSettings,
    Query,
    Schema,
    Play,
    AiChat,
    AiChatNative,
    AiChatPty,
    AiChatMessage,
    Tool,
    ToolNative,
}

/// Which of the three kinds of core type a variant is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CoreTypeKind {
    /// A core type with no core parent.
    Concrete,
    /// A core type that is never instantiated; only its subtypes are.
    AbstractBase,
    /// A core type that `extends` another core type.
    CoreSubtype,
}

/// What "strongly typed" amounts to for a type, computed over its whole
/// `extends` chain (ADR-086 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TypeCategory {
    /// The chain declares no fields; the data is `content` (or the id).
    Primitive,
    /// Fields are scalars, enums, dates or arrays of scalars.
    Flat,
    /// At least one field holds objects.
    Structured,
}

/// What a node's `content` means for its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ContentRole {
    /// Markup body; any attributes are derived from it.
    Body,
    /// The node's name; its body is in its children.
    Name,
    /// The conversation title.
    Title,
    /// A one-line description.
    DescriptionLine,
    /// Not displayed: the name comes from the `title_template`.
    Ignored,
}

/// Which children a type's nodes may have (ADR-089).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildrenRule {
    Any,
    None,
    /// Any child except these types and their subtypes.
    AnyExcept(&'static [CoreNodeType]),
}

/// Where a type's nodes may sit in the `has_child` tree (ADR-089).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentRule {
    Any,
    MustBeRoot,
    /// Only under one of these types or their subtypes.
    MustHaveParentOf(&'static [CoreNodeType]),
}

/// A type's structural rules. A subtype inherits them and may only tighten
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuralRules {
    pub children: ChildrenRule,
    pub parent: ParentRule,
}

impl StructuralRules {
    pub const ANY: Self = Self {
        children: ChildrenRule::Any,
        parent: ParentRule::Any,
    };
    const ROOT_ONLY: Self = Self {
        children: ChildrenRule::Any,
        parent: ParentRule::MustBeRoot,
    };
    const LEAF: Self = Self {
        children: ChildrenRule::None,
        parent: ParentRule::Any,
    };
}

/// Which surfaces a type's nodes take part in (ADR-087 §2). Each rule follows
/// the `extends` chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipationRules {
    /// Embedded for semantic search when it is a root.
    pub embedded: bool,
    /// Offered by the `@` mention picker.
    pub mentionable: bool,
    /// Left out of default queries, counts and lists.
    pub excluded_from_default_queries: bool,
}

impl ParticipationRules {
    const EMBEDDED: Self = Self {
        embedded: true,
        mentionable: true,
        excluded_from_default_queries: false,
    };
    const NOT_EMBEDDED: Self = Self {
        embedded: false,
        mentionable: true,
        excluded_from_default_queries: false,
    };
    const fn not_mentionable(self) -> Self {
        Self {
            mentionable: false,
            ..self
        }
    }
    const fn excluded_from_default_queries(self) -> Self {
        Self {
            excluded_from_default_queries: true,
            ..self
        }
    }
}

/// How a core type travels on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireShape {
    /// The envelope alone. A primitive has no fields, so no struct or update.
    Envelope,
    /// The generic node: the type's fields travel inside `properties`.
    Generic,
    /// A typed struct that promotes the type's fields to the top level.
    Typed {
        /// Whether the type has a typed update.
        update: bool,
    },
}

/// The type of value a derived attribute computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedValueType {
    Boolean,
}

/// An attribute computed from a node's `content` and never stored
/// (ADR-086 §4, ADR-094 §5).
///
/// A type declares its derived attributes in its registry entry, and its
/// subtypes inherit them. Each has one Rust function ([`Self::derive`]) and
/// the same computation as a SQL expression ([`Self::sql`]); every reader
/// goes through one of the two, so nothing else matches on a content prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DerivedAttribute {
    /// Whether a checkbox is ticked.
    Checked,
}

impl DerivedAttribute {
    /// Every derived attribute any core type declares.
    pub const ALL: [DerivedAttribute; 1] = [DerivedAttribute::Checked];

    /// The name the attribute is read by, in a rule and in a query filter.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Checked => "checked",
        }
    }

    pub const fn value_type(self) -> DerivedValueType {
        match self {
            Self::Checked => DerivedValueType::Boolean,
        }
    }

    /// The attribute's value for a node with this `content`.
    pub fn derive(self, content: &str) -> serde_json::Value {
        match self {
            Self::Checked => serde_json::Value::Bool(checkbox_is_checked(content)),
        }
    }

    /// [`Self::derive`] as a SQL expression over `content`, a column or
    /// expression holding the node's content. A boolean is `1` or `0`.
    pub fn sql(self, content: &str) -> String {
        match self {
            Self::Checked => {
                format!("(substr({content}, 1, 6) IN ('- [x] ', '- [X] '))")
            }
        }
    }

    /// The core types that declare an attribute of this name, each with the
    /// attribute it declares. Subtypes are reached through the `extends`
    /// chain, not listed.
    pub fn declared_as(name: &str) -> Vec<(CoreNodeType, DerivedAttribute)> {
        CoreNodeType::ALL
            .into_iter()
            .flat_map(|t| {
                t.info()
                    .derived
                    .iter()
                    .filter(|attribute| attribute.name() == name)
                    .map(move |attribute| (t, *attribute))
            })
            .collect()
    }
}

/// Whether a checkbox with this `content` is ticked: the content starts with
/// `- [x] ` or `- [X] `. The one definition of a checkbox's checked state.
pub fn checkbox_is_checked(content: &str) -> bool {
    content.starts_with("- [x] ") || content.starts_with("- [X] ")
}

/// Everything the registry records for one core type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreTypeInfo {
    /// The stored `node_type`.
    pub id: &'static str,
    /// The core type this one `extends`.
    pub parent: Option<CoreNodeType>,
    /// Whether nodes of exactly this type may exist.
    pub is_abstract: bool,
    pub category: TypeCategory,
    pub content_role: ContentRole,
    pub structure: StructuralRules,
    pub participation: ParticipationRules,
    /// Titled by its content at any depth, not only as a root.
    pub always_titled: bool,
    /// The template the title is interpolated from, when the type has one.
    pub title_template: Option<&'static str>,
    pub wire: WireShape,
    /// The attributes derived from this type's content.
    pub derived: &'static [DerivedAttribute],
}

impl CoreNodeType {
    /// Every core type, in registry order.
    pub const ALL: [CoreNodeType; 28] = [
        CoreNodeType::Text,
        CoreNodeType::Header,
        CoreNodeType::CodeBlock,
        CoreNodeType::QuoteBlock,
        CoreNodeType::OrderedList,
        CoreNodeType::Checkbox,
        CoreNodeType::HorizontalLine,
        CoreNodeType::Table,
        CoreNodeType::Date,
        CoreNodeType::AgentGuidance,
        CoreNodeType::Task,
        CoreNodeType::Project,
        CoreNodeType::Spec,
        CoreNodeType::Plan,
        CoreNodeType::Decision,
        CoreNodeType::Person,
        CoreNodeType::Collection,
        CoreNodeType::Skill,
        CoreNodeType::DatabaseSettings,
        CoreNodeType::Query,
        CoreNodeType::Schema,
        CoreNodeType::Play,
        CoreNodeType::AiChat,
        CoreNodeType::AiChatNative,
        CoreNodeType::AiChatPty,
        CoreNodeType::AiChatMessage,
        CoreNodeType::Tool,
        CoreNodeType::ToolNative,
    ];

    /// The registry entry for this type.
    pub const fn info(self) -> CoreTypeInfo {
        use ContentRole::{Body, DescriptionLine, Ignored, Name, Title};
        use TypeCategory::{Flat, Primitive, Structured};

        const fn entry(
            id: &'static str,
            category: TypeCategory,
            content_role: ContentRole,
            participation: ParticipationRules,
            wire: WireShape,
        ) -> CoreTypeInfo {
            CoreTypeInfo {
                id,
                parent: None,
                is_abstract: false,
                category,
                content_role,
                structure: StructuralRules::ANY,
                participation,
                always_titled: false,
                title_template: None,
                wire,
                derived: &[],
            }
        }
        /// A type whose nodes take no children.
        const fn leaf(info: CoreTypeInfo) -> CoreTypeInfo {
            CoreTypeInfo {
                structure: StructuralRules::LEAF,
                ..info
            }
        }
        /// A type whose nodes are always roots.
        const fn root_only(info: CoreTypeInfo) -> CoreTypeInfo {
            CoreTypeInfo {
                structure: StructuralRules::ROOT_ONLY,
                ..info
            }
        }
        const EMBEDDED: ParticipationRules = ParticipationRules::EMBEDDED;
        const NOT_EMBEDDED: ParticipationRules = ParticipationRules::NOT_EMBEDDED;
        const TYPED: WireShape = WireShape::Typed { update: true };

        match self {
            Self::Text => entry("text", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::Header => entry("header", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::CodeBlock => leaf(entry(
                "code-block",
                Primitive,
                Body,
                EMBEDDED,
                WireShape::Envelope,
            )),
            Self::QuoteBlock => entry(
                "quote-block",
                Primitive,
                Body,
                EMBEDDED,
                WireShape::Envelope,
            ),
            Self::OrderedList => leaf(entry(
                "ordered-list",
                Primitive,
                Body,
                EMBEDDED,
                WireShape::Envelope,
            )),
            Self::Checkbox => CoreTypeInfo {
                derived: &[DerivedAttribute::Checked],
                ..entry("checkbox", Primitive, Body, EMBEDDED, WireShape::Envelope)
            },
            Self::HorizontalLine => leaf(entry(
                "horizontal-line",
                Primitive,
                Body,
                NOT_EMBEDDED,
                WireShape::Envelope,
            )),
            Self::Table => leaf(entry(
                "table",
                Primitive,
                Body,
                EMBEDDED,
                WireShape::Envelope,
            )),
            // A date page is always top-level; it joins collections through
            // `member_of`.
            Self::Date => root_only(entry(
                "date",
                Primitive,
                Name,
                NOT_EMBEDDED,
                WireShape::Envelope,
            )),
            Self::AgentGuidance => entry(
                "agent-guidance",
                Primitive,
                Name,
                NOT_EMBEDDED,
                WireShape::Envelope,
            ),
            Self::Task => CoreTypeInfo {
                always_titled: true,
                ..entry("task", Flat, DescriptionLine, NOT_EMBEDDED, TYPED)
            },
            Self::Project => entry("project", Flat, Name, NOT_EMBEDDED, TYPED),
            // What is built and why, how, and what was decided (ADR-092). The
            // content is the title; a spec's criteria are its checkbox
            // children and a decision's body is its children.
            Self::Spec => entry("spec", Flat, Name, NOT_EMBEDDED, TYPED),
            Self::Plan => entry("plan", Flat, Name, NOT_EMBEDDED, TYPED),
            Self::Decision => entry("decision", Flat, Name, NOT_EMBEDDED, TYPED),
            Self::Person => CoreTypeInfo {
                title_template: Some("{first_name} {last_name}"),
                ..entry("person", Flat, Ignored, NOT_EMBEDDED, TYPED)
            },
            // Collections nest through `member_of`, never `has_child`.
            Self::Collection => CoreTypeInfo {
                always_titled: true,
                ..root_only(entry(
                    "collection",
                    Flat,
                    Name,
                    NOT_EMBEDDED.not_mentionable(),
                    TYPED,
                ))
            },
            Self::Skill => entry("skill", Flat, Name, EMBEDDED, TYPED),
            Self::DatabaseSettings => {
                leaf(entry("database-settings", Flat, Name, NOT_EMBEDDED, TYPED))
            }
            Self::Query => leaf(entry(
                "query",
                Structured,
                DescriptionLine,
                NOT_EMBEDDED,
                TYPED,
            )),
            // A schema's children are its description subtree.
            Self::Schema => root_only(entry(
                "schema",
                Structured,
                Name,
                EMBEDDED.not_mentionable(),
                WireShape::Typed { update: false },
            )),
            Self::Play => entry("play", Structured, Name, NOT_EMBEDDED, TYPED),
            // The chat family (ADR-088). The base is never instantiated; its
            // rules are the floor both subtypes inherit.
            Self::AiChat => CoreTypeInfo {
                is_abstract: true,
                ..entry(
                    "ai-chat",
                    Flat,
                    Title,
                    NOT_EMBEDDED.not_mentionable(),
                    WireShape::Typed { update: false },
                )
            },
            Self::AiChatNative => CoreTypeInfo {
                parent: Some(Self::AiChat),
                ..entry(
                    "ai-chat-native",
                    Flat,
                    Title,
                    NOT_EMBEDDED,
                    WireShape::Typed { update: false },
                )
            },
            Self::AiChatPty => CoreTypeInfo {
                parent: Some(Self::AiChat),
                ..entry(
                    "ai-chat-pty",
                    Flat,
                    Title,
                    NOT_EMBEDDED,
                    WireShape::Typed { update: false },
                )
            },
            // A message of a native chat (ADR-088 §3): a child of its chat,
            // in conversation order, and never a node on its own.
            Self::AiChatMessage => CoreTypeInfo {
                structure: StructuralRules {
                    children: ChildrenRule::None,
                    parent: ParentRule::MustHaveParentOf(&[Self::AiChatNative]),
                },
                ..entry(
                    "ai-chat-message",
                    Flat,
                    Body,
                    NOT_EMBEDDED
                        .not_mentionable()
                        .excluded_from_default_queries(),
                    WireShape::Typed { update: false },
                )
            },
            // The tool family (§12). The base is never instantiated: the
            // subtype says where a tool comes from.
            Self::Tool => CoreTypeInfo {
                is_abstract: true,
                ..leaf(entry(
                    "tool",
                    Structured,
                    Name,
                    EMBEDDED,
                    WireShape::Generic,
                ))
            },
            Self::ToolNative => CoreTypeInfo {
                parent: Some(Self::Tool),
                ..entry(
                    "tool-native",
                    Structured,
                    Name,
                    EMBEDDED,
                    WireShape::Generic,
                )
            },
        }
    }

    /// The stored `node_type` of this type.
    pub const fn as_str(self) -> &'static str {
        self.info().id
    }

    /// The core type whose stored `node_type` is exactly `node_type`.
    ///
    /// `None` for a user-defined or extension type, including a subtype of a
    /// core type: use [`Self::nearest`] with the type's chain to apply a base
    /// type's rule.
    pub fn from_id(node_type: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == node_type)
    }

    /// The nearest core type in an `extends` chain given nearest scope first:
    /// the type itself when it is core, else its closest core ancestor.
    pub fn nearest<S: AsRef<str>>(chain: &[S]) -> Option<Self> {
        chain.iter().find_map(|t| Self::from_id(t.as_ref()))
    }

    /// The core type this one `extends`.
    pub const fn parent(self) -> Option<Self> {
        self.info().parent
    }

    /// This type followed by its core ancestors, nearest first.
    pub fn chain(self) -> Vec<Self> {
        let mut chain = vec![self];
        let mut current = self;
        while let Some(parent) = current.parent() {
            chain.push(parent);
            current = parent;
        }
        chain
    }

    /// Whether this type is `base` or descends from it.
    pub fn is_a(self, base: Self) -> bool {
        self.chain().contains(&base)
    }

    /// Whether `node_type` is exactly this type's id.
    ///
    /// Exactness is right only where a subtype must not match: the `schema`
    /// meta-type, which nothing can extend, and a wire shape, which a subtype
    /// never borrows from its base. A type's *rules* reach its subtypes, so
    /// applying one takes [`Self::nearest`] over the node's chain instead.
    pub fn is_exactly(self, node_type: impl AsRef<str>) -> bool {
        self.as_str() == node_type.as_ref()
    }

    pub const fn kind(self) -> CoreTypeKind {
        let info = self.info();
        if info.is_abstract {
            CoreTypeKind::AbstractBase
        } else if info.parent.is_some() {
            CoreTypeKind::CoreSubtype
        } else {
            CoreTypeKind::Concrete
        }
    }

    pub const fn is_abstract(self) -> bool {
        self.info().is_abstract
    }

    pub const fn category(self) -> TypeCategory {
        self.info().category
    }

    pub const fn content_role(self) -> ContentRole {
        self.info().content_role
    }

    /// The structural rules this type itself declares, before inheritance.
    pub const fn declared_structure(self) -> StructuralRules {
        self.info().structure
    }

    /// The structural rules in force for this type: its ancestors' rules with
    /// its own on top. A subtype only tightens, so the nearest declaration of
    /// each rule is the one in force.
    pub fn structure(self) -> StructuralRules {
        let mut rules = StructuralRules::ANY;
        for ancestor in self.chain().into_iter().rev() {
            let own = ancestor.info().structure;
            if own.children != ChildrenRule::Any {
                rules.children = own.children;
            }
            if own.parent != ParentRule::Any {
                rules.parent = own.parent;
            }
        }
        rules
    }

    /// The participation rules in force for this type. A rule can only narrow
    /// down the chain: once an ancestor is unembedded, unmentionable or
    /// excluded, so is every subtype.
    pub fn participation(self) -> ParticipationRules {
        let mut rules = self.info().participation;
        for ancestor in self.chain().into_iter().skip(1) {
            let inherited = ancestor.info().participation;
            rules.embedded &= inherited.embedded;
            rules.mentionable &= inherited.mentionable;
            rules.excluded_from_default_queries |= inherited.excluded_from_default_queries;
        }
        rules
    }

    /// Whether a node of this type is embedded together with its subtree.
    ///
    /// A chat's subtree is not (ADR-061 §4): each child of a chat is its own
    /// embedding root, so a node kept under a chat stays searchable
    /// (ADR-089 §4).
    pub fn embeds_subtree(self) -> bool {
        !self.is_a(Self::AiChat)
    }

    /// Whether another node may reference a node of this type. No node may
    /// reference a chat (ADR-061 §8) or one of its messages (ADR-088 §3): a
    /// mention of one creates no edge, and no relationship may target one.
    /// Placing one under a parent with `has_child` is not a reference.
    pub fn accepts_inbound_references(self) -> bool {
        !self.is_a(Self::AiChat) && self != Self::AiChatMessage
    }

    /// The core types no node may reference.
    pub fn unreferenceable() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|t| !t.accepts_inbound_references())
            .collect()
    }

    /// Whether the type is titled by its content at any depth.
    pub fn always_titled(self) -> bool {
        self.chain().into_iter().any(|t| t.info().always_titled)
    }

    /// The nearest `title_template` in the chain.
    pub fn title_template(self) -> Option<&'static str> {
        self.chain()
            .into_iter()
            .find_map(|t| t.info().title_template)
    }

    pub const fn wire(self) -> WireShape {
        self.info().wire
    }

    /// The derived attributes in force for this type: its own and its
    /// ancestors'.
    pub fn derived_attributes(self) -> Vec<DerivedAttribute> {
        self.chain()
            .into_iter()
            .flat_map(|t| t.info().derived.iter().copied())
            .collect()
    }

    /// The derived attribute called `name` on this type, if it has one.
    pub fn derived_attribute(self, name: &str) -> Option<DerivedAttribute> {
        self.derived_attributes()
            .into_iter()
            .find(|attribute| attribute.name() == name)
    }

    /// The derived attributes of the type whose `extends` chain is `chain`,
    /// nearest scope first. A user-defined subtype of a core type has the
    /// attributes of its nearest core ancestor.
    pub fn derived_attributes_in<S: AsRef<str>>(chain: &[S]) -> Vec<DerivedAttribute> {
        Self::nearest(chain)
            .map(Self::derived_attributes)
            .unwrap_or_default()
    }

    /// The derived attribute called `name` on the type whose `extends` chain
    /// is `chain`, nearest scope first.
    pub fn derived_attribute_in<S: AsRef<str>>(
        chain: &[S],
        name: &str,
    ) -> Option<DerivedAttribute> {
        Self::nearest(chain)?.derived_attribute(name)
    }

    /// The core types the `@` mention picker leaves out.
    pub fn not_mentionable() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|t| !t.participation().mentionable)
            .collect()
    }

    /// The core types left out of default queries, counts and lists even
    /// when active.
    pub fn excluded_from_default_queries() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|t| t.participation().excluded_from_default_queries)
            .collect()
    }

    /// The core types titled by their content at any depth.
    pub fn always_titled_types() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|t| t.always_titled())
            .collect()
    }
}

impl std::fmt::Display for CoreNodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for CoreNodeType {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_lists_every_variant_once() {
        // An exhaustive match: adding a variant without listing it in `ALL`
        // fails to compile here.
        for t in CoreNodeType::ALL {
            match t {
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
                | CoreNodeType::Task
                | CoreNodeType::Project
                | CoreNodeType::Spec
                | CoreNodeType::Plan
                | CoreNodeType::Decision
                | CoreNodeType::Person
                | CoreNodeType::Collection
                | CoreNodeType::Skill
                | CoreNodeType::DatabaseSettings
                | CoreNodeType::Query
                | CoreNodeType::Schema
                | CoreNodeType::Play
                | CoreNodeType::AiChat
                | CoreNodeType::AiChatNative
                | CoreNodeType::AiChatPty
                | CoreNodeType::AiChatMessage
                | CoreNodeType::Tool
                | CoreNodeType::ToolNative => {}
            }
        }
        let unique: HashSet<_> = CoreNodeType::ALL.into_iter().collect();
        assert_eq!(unique.len(), CoreNodeType::ALL.len());
    }

    #[test]
    fn ids_are_unique_kebab_case_and_round_trip() {
        let mut seen = HashSet::new();
        for t in CoreNodeType::ALL {
            let id = t.as_str();
            assert!(seen.insert(id), "duplicate id {id}");
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{id} is not kebab-case"
            );
            assert_eq!(CoreNodeType::from_id(id), Some(t));
            // The serde form is the stored id.
            assert_eq!(serde_json::to_value(t).unwrap(), serde_json::json!(id));
        }
        assert_eq!(CoreNodeType::from_id("issue"), None);
    }

    #[test]
    fn nearest_resolves_a_subtype_to_its_core_ancestor() {
        assert_eq!(
            CoreNodeType::nearest(&["issue", "task"]),
            Some(CoreNodeType::Task)
        );
        assert_eq!(CoreNodeType::nearest(&["task"]), Some(CoreNodeType::Task));
        assert_eq!(CoreNodeType::nearest(&["invoice"]), None);
    }

    #[test]
    fn a_parent_is_never_its_own_descendant() {
        for t in CoreNodeType::ALL {
            let chain = t.chain();
            let unique: HashSet<_> = chain.iter().collect();
            assert_eq!(unique.len(), chain.len(), "{t} has a cyclic chain");
            if t.parent().is_some() {
                assert_eq!(t.kind(), CoreTypeKind::CoreSubtype);
            }
        }
    }

    #[test]
    fn checkbox_declares_checked_and_no_other_type_does() {
        assert_eq!(
            DerivedAttribute::declared_as("checked"),
            vec![(CoreNodeType::Checkbox, DerivedAttribute::Checked)]
        );
        assert_eq!(
            CoreNodeType::Checkbox.derived_attribute("checked"),
            Some(DerivedAttribute::Checked)
        );
        assert_eq!(CoreNodeType::Task.derived_attribute("checked"), None);
        assert_eq!(CoreNodeType::Checkbox.derived_attribute("status"), None);
        assert!(DerivedAttribute::declared_as("status").is_empty());
        // A user subtype has its nearest core ancestor's attributes.
        assert_eq!(
            CoreNodeType::derived_attribute_in(&["criterion", "checkbox"], "checked"),
            Some(DerivedAttribute::Checked)
        );
        assert_eq!(
            CoreNodeType::derived_attribute_in(&["invoice"], "checked"),
            None
        );
        assert_eq!(
            DerivedAttribute::Checked.value_type(),
            DerivedValueType::Boolean
        );
    }

    #[test]
    fn only_a_primitive_declares_derived_attributes_and_every_declared_one_is_listed() {
        let mut declared = HashSet::new();
        for t in CoreNodeType::ALL {
            let own = t.info().derived;
            if !own.is_empty() {
                assert_eq!(t.category(), TypeCategory::Primitive, "{t}");
            }
            let names: HashSet<_> = own.iter().map(|a| a.name()).collect();
            assert_eq!(names.len(), own.len(), "{t} declares a name twice");
            declared.extend(own.iter().copied());
        }
        let all: HashSet<_> = DerivedAttribute::ALL.into_iter().collect();
        assert_eq!(declared, all);
    }

    #[test]
    fn checked_follows_the_content_prefix() {
        for (content, checked) in [
            ("- [x] done", true),
            ("- [X] done", true),
            ("- [x] ", true),
            ("- [ ] open", false),
            ("- [x]", false),
            (" - [x] indented", false),
            ("-[x] no space", false),
            ("- [x]\tdone", false),
            ("plain", false),
            ("", false),
        ] {
            assert_eq!(checkbox_is_checked(content), checked, "{content:?}");
            assert_eq!(
                DerivedAttribute::Checked.derive(content),
                serde_json::json!(checked)
            );
        }
    }

    #[test]
    fn a_primitive_travels_as_the_envelope_and_only_a_primitive_does() {
        for t in CoreNodeType::ALL {
            assert_eq!(
                t.category() == TypeCategory::Primitive,
                t.wire() == WireShape::Envelope,
                "{t}: a primitive has no struct, and a type with fields is not an envelope"
            );
        }
    }

    #[test]
    fn root_only_and_mention_rules_match_the_declared_types() {
        let with_parent_rule = |rule: ParentRule| -> Vec<CoreNodeType> {
            CoreNodeType::ALL
                .into_iter()
                .filter(|t| t.structure().parent == rule)
                .collect()
        };
        let with_children_rule = |rule: ChildrenRule| -> Vec<CoreNodeType> {
            CoreNodeType::ALL
                .into_iter()
                .filter(|t| t.structure().children == rule)
                .collect()
        };
        assert_eq!(
            with_parent_rule(ParentRule::MustBeRoot),
            vec![
                CoreNodeType::Date,
                CoreNodeType::Collection,
                CoreNodeType::Schema
            ]
        );
        assert_eq!(
            with_children_rule(ChildrenRule::None),
            vec![
                CoreNodeType::CodeBlock,
                CoreNodeType::OrderedList,
                CoreNodeType::HorizontalLine,
                CoreNodeType::Table,
                CoreNodeType::DatabaseSettings,
                CoreNodeType::Query,
                CoreNodeType::AiChatMessage,
                CoreNodeType::Tool,
                CoreNodeType::ToolNative
            ]
        );
        assert_eq!(
            CoreNodeType::AiChatMessage.structure().parent,
            ParentRule::MustHaveParentOf(&[CoreNodeType::AiChatNative])
        );
        // A chat holds its messages and may hold other nodes too.
        assert_eq!(CoreNodeType::AiChat.structure(), StructuralRules::ANY);
        assert_eq!(
            CoreNodeType::not_mentionable(),
            vec![
                CoreNodeType::Collection,
                CoreNodeType::Schema,
                CoreNodeType::AiChat,
                CoreNodeType::AiChatNative,
                CoreNodeType::AiChatPty,
                CoreNodeType::AiChatMessage
            ]
        );
        assert_eq!(
            CoreNodeType::excluded_from_default_queries(),
            vec![CoreNodeType::AiChatMessage]
        );
        assert_eq!(
            CoreNodeType::unreferenceable(),
            vec![
                CoreNodeType::AiChat,
                CoreNodeType::AiChatNative,
                CoreNodeType::AiChatPty,
                CoreNodeType::AiChatMessage
            ]
        );
        assert_eq!(
            CoreNodeType::always_titled_types(),
            vec![CoreNodeType::Task, CoreNodeType::Collection]
        );
    }

    #[test]
    fn the_chat_family_is_an_abstract_base_with_two_core_subtypes() {
        assert_eq!(CoreNodeType::AiChat.kind(), CoreTypeKind::AbstractBase);
        for subtype in [CoreNodeType::AiChatNative, CoreNodeType::AiChatPty] {
            assert_eq!(subtype.kind(), CoreTypeKind::CoreSubtype);
            assert_eq!(subtype.parent(), Some(CoreNodeType::AiChat));
            assert!(subtype.is_a(CoreNodeType::AiChat));
            assert!(!subtype.is_abstract());
            // The base's rules reach both: not embedded, not mentionable,
            // and any child is allowed.
            assert!(!subtype.participation().embedded);
            assert!(!subtype.participation().mentionable);
            assert_eq!(subtype.structure().children, ChildrenRule::Any);
            assert_eq!(subtype.content_role(), ContentRole::Title);
        }
        assert!(!CoreNodeType::AiChatNative.is_a(CoreNodeType::AiChatPty));
        assert_eq!(
            CoreNodeType::nearest(&["ai-chat-pty", "ai-chat"]),
            Some(CoreNodeType::AiChatPty)
        );
    }

    #[test]
    fn the_tool_family_is_an_abstract_base_with_a_native_subtype() {
        assert_eq!(CoreNodeType::Tool.kind(), CoreTypeKind::AbstractBase);
        let native = CoreNodeType::ToolNative;
        assert_eq!(native.kind(), CoreTypeKind::CoreSubtype);
        assert_eq!(native.parent(), Some(CoreNodeType::Tool));
        assert!(native.is_a(CoreNodeType::Tool));
        assert!(!native.is_abstract());
        // The base's rules reach it: a leaf, embedded, named by its content,
        // and structured through the inherited parameter schema.
        assert_eq!(native.declared_structure(), StructuralRules::ANY);
        assert_eq!(native.structure().children, ChildrenRule::None);
        assert!(native.participation().embedded);
        assert_eq!(native.content_role(), ContentRole::Name);
        assert_eq!(native.category(), TypeCategory::Structured);
        assert_eq!(
            CoreNodeType::nearest(&["tool-native", "tool"]),
            Some(CoreNodeType::ToolNative)
        );
        // A subtype no build ships resolves to the base, not to the native one.
        assert_eq!(
            CoreNodeType::nearest(&["tool-remote", "tool"]),
            Some(CoreNodeType::Tool)
        );
    }

    #[test]
    fn a_chat_message_is_a_flat_leaf_that_takes_part_in_nothing() {
        let message = CoreNodeType::AiChatMessage;
        assert_eq!(message.kind(), CoreTypeKind::Concrete);
        assert_eq!(message.category(), TypeCategory::Flat);
        assert_eq!(message.content_role(), ContentRole::Body);
        assert!(!message.is_a(CoreNodeType::AiChat));
        let participation = message.participation();
        assert!(!participation.embedded);
        assert!(!participation.mentionable);
        assert!(participation.excluded_from_default_queries);
        assert!(!message.accepts_inbound_references());
        assert!(CoreNodeType::Task.accepts_inbound_references());
        // The messages left `ai-chat-native`: every field it keeps is a scalar.
        assert_eq!(CoreNodeType::AiChatNative.category(), TypeCategory::Flat);
    }
}
