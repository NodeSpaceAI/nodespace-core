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
    Person,
    Collection,
    Skill,
    DatabaseSettings,
    Query,
    Schema,
    Play,
    AiChat,
    Tool,
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
    const ANY: Self = Self {
        children: ChildrenRule::Any,
        parent: ParentRule::Any,
    };
    const ROOT_ONLY: Self = Self {
        children: ChildrenRule::Any,
        parent: ParentRule::MustBeRoot,
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
}

impl CoreNodeType {
    /// Every core type, in registry order.
    pub const ALL: [CoreNodeType; 21] = [
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
        CoreNodeType::Person,
        CoreNodeType::Collection,
        CoreNodeType::Skill,
        CoreNodeType::DatabaseSettings,
        CoreNodeType::Query,
        CoreNodeType::Schema,
        CoreNodeType::Play,
        CoreNodeType::AiChat,
        CoreNodeType::Tool,
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
            }
        }
        const EMBEDDED: ParticipationRules = ParticipationRules::EMBEDDED;
        const NOT_EMBEDDED: ParticipationRules = ParticipationRules::NOT_EMBEDDED;
        const TYPED: WireShape = WireShape::Typed { update: true };

        match self {
            Self::Text => entry("text", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::Header => entry("header", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::CodeBlock => entry("code-block", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::QuoteBlock => entry(
                "quote-block",
                Primitive,
                Body,
                EMBEDDED,
                WireShape::Envelope,
            ),
            Self::OrderedList => entry(
                "ordered-list",
                Primitive,
                Body,
                EMBEDDED,
                WireShape::Envelope,
            ),
            Self::Checkbox => entry("checkbox", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::HorizontalLine => entry(
                "horizontal-line",
                Primitive,
                Body,
                NOT_EMBEDDED,
                WireShape::Envelope,
            ),
            Self::Table => entry("table", Primitive, Body, EMBEDDED, WireShape::Envelope),
            Self::Date => entry("date", Primitive, Name, NOT_EMBEDDED, WireShape::Envelope),
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
            Self::Person => CoreTypeInfo {
                title_template: Some("{first_name} {last_name}"),
                ..entry("person", Flat, Ignored, NOT_EMBEDDED, TYPED)
            },
            Self::Collection => CoreTypeInfo {
                structure: StructuralRules::ROOT_ONLY,
                always_titled: true,
                ..entry(
                    "collection",
                    Flat,
                    Name,
                    NOT_EMBEDDED.not_mentionable(),
                    WireShape::Generic,
                )
            },
            Self::Skill => entry("skill", Flat, Name, EMBEDDED, WireShape::Generic),
            Self::DatabaseSettings => entry(
                "database-settings",
                Flat,
                Name,
                NOT_EMBEDDED,
                WireShape::Generic,
            ),
            Self::Query => entry("query", Structured, DescriptionLine, NOT_EMBEDDED, TYPED),
            Self::Schema => CoreTypeInfo {
                structure: StructuralRules::ROOT_ONLY,
                ..entry(
                    "schema",
                    Structured,
                    Name,
                    EMBEDDED.not_mentionable(),
                    WireShape::Typed { update: false },
                )
            },
            Self::Play => entry("play", Structured, Name, NOT_EMBEDDED, WireShape::Generic),
            Self::AiChat => entry(
                "ai-chat",
                Structured,
                Title,
                NOT_EMBEDDED.not_mentionable(),
                WireShape::Typed { update: false },
            ),
            Self::Tool => entry("tool", Structured, Name, EMBEDDED, WireShape::Generic),
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

    /// The structural rules in force for this type: its own, or the nearest
    /// ancestor's that declares a tighter one.
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

    /// The core types that must be roots, with every core subtype of one.
    pub fn root_only() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|t| t.structure().parent == ParentRule::MustBeRoot)
            .collect()
    }

    /// The core types the `@` mention picker leaves out.
    pub fn not_mentionable() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|t| !t.participation().mentionable)
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
                | CoreNodeType::Person
                | CoreNodeType::Collection
                | CoreNodeType::Skill
                | CoreNodeType::DatabaseSettings
                | CoreNodeType::Query
                | CoreNodeType::Schema
                | CoreNodeType::Play
                | CoreNodeType::AiChat
                | CoreNodeType::Tool => {}
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
        assert_eq!(
            CoreNodeType::root_only(),
            vec![CoreNodeType::Collection, CoreNodeType::Schema]
        );
        assert_eq!(
            CoreNodeType::not_mentionable(),
            vec![
                CoreNodeType::Collection,
                CoreNodeType::Schema,
                CoreNodeType::AiChat
            ]
        );
        assert_eq!(
            CoreNodeType::always_titled_types(),
            vec![CoreNodeType::Task, CoreNodeType::Collection]
        );
    }
}
