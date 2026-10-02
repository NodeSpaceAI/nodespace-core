//! `RelationshipPath`: one typed way to say "from this node, follow these
//! relationships" (ADR-086 §11).
//!
//! Query relationship filters, a play's `for_each` and the dot-paths in a
//! play's conditions all describe a walk with this type. The names it carries
//! are what an author writes; [`ResolvedPath`] is what those names mean once
//! the schemas have been consulted, and is what compiles to SQL.

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One hop of a [`RelationshipPath`].
///
/// `name` is a relationship as the author names it: a built-in (`has_child`,
/// `member_of`, `mentions`, `has_role`), a schema-declared relationship
/// (`blocks`, `tasks`), or the reverse name of either (`child_of`,
/// `blocked_by`, `project`). The direction of travel is implied by which name
/// was used.
///
/// An open-ended hop follows the relationship repeatedly: `child_of` reaches
/// the parent, an open-ended `child_of` reaches every ancestor.
///
/// On the wire a fixed hop is its bare name and an open-ended hop is
/// `{ "name": "child_of", "open_ended": true }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(
    feature = "ts",
    ts(type = "string | { name: string; open_ended: boolean }")
)]
pub struct RelationshipHop {
    pub name: String,
    pub open_ended: bool,
}

impl RelationshipHop {
    /// A hop that follows `name` once.
    pub fn fixed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            open_ended: false,
        }
    }

    /// A hop that follows `name` repeatedly, to every node it leads to.
    pub fn open_ended(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            open_ended: true,
        }
    }
}

impl Serialize for RelationshipHop {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.open_ended {
            let mut hop = serializer.serialize_struct("RelationshipHop", 2)?;
            hop.serialize_field("name", &self.name)?;
            hop.serialize_field("open_ended", &true)?;
            hop.end()
        } else {
            serializer.serialize_str(&self.name)
        }
    }
}

impl<'de> Deserialize<'de> for RelationshipHop {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HopVisitor;

        impl<'de> Visitor<'de> for HopVisitor {
            type Value = RelationshipHop;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a relationship name, or { \"name\": ..., \"open_ended\": ... }")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
                named(name.to_string(), false)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut name: Option<String> = None;
                let mut open_ended = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => name = Some(map.next_value()?),
                        "open_ended" => open_ended = map.next_value()?,
                        other => {
                            return Err(de::Error::unknown_field(other, &["name", "open_ended"]))
                        }
                    }
                }
                named(
                    name.ok_or_else(|| de::Error::missing_field("name"))?,
                    open_ended,
                )
            }
        }

        fn named<E: de::Error>(name: String, open_ended: bool) -> Result<RelationshipHop, E> {
            if name.is_empty() {
                return Err(E::custom("a path hop needs a relationship name"));
            }
            Ok(RelationshipHop { name, open_ended })
        }

        deserializer.deserialize_any(HopVisitor)
    }
}

/// A walk through the graph: the hops to follow, in order, from a starting
/// node. On the wire it is the list of hops: `["child_of", "has_child"]`.
///
/// A path names relationships only. Whether a name is valid, and which stored
/// edges it means, depends on the schemas: resolving a path against them
/// yields a [`ResolvedPath`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct RelationshipPath(pub Vec<RelationshipHop>);

impl RelationshipPath {
    /// A path of fixed hops, one per name.
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(names.into_iter().map(RelationshipHop::fixed).collect())
    }

    pub fn hops(&self) -> &[RelationshipHop] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl std::fmt::Display for RelationshipPath {
    /// The dotted form an author writes in a condition, with `*` after an
    /// open-ended hop: `child_of*.project`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, hop) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(&hop.name)?;
            if hop.open_ended {
                f.write_str("*")?;
            }
        }
        Ok(())
    }
}

/// Which way a resolved hop crosses its stored edge.
///
/// An edge is stored once, from its source to its target, under the forward
/// name. A forward name walks it source to target; a reverse name walks the
/// same row target to source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HopDirection {
    /// Source to target: the current node is the edge's source.
    Outbound,
    /// Target to source: the current node is the edge's target.
    Inbound,
}

/// One hop of a [`ResolvedPath`]: the stored edges a name means.
///
/// Never serialized. A path is resolved again from its names whenever it is
/// loaded, so a resolved hop never outlives the schemas it was resolved
/// against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedHop {
    /// The name the author wrote.
    pub name: String,
    /// The `relationship_type` the edges are stored under: the forward name,
    /// whichever name the author used.
    pub relationship_type: String,
    pub direction: HopDirection,
    /// The type that declared the relationship, when a reverse name was used.
    ///
    /// Several schemas may declare the same forward name toward one type
    /// (`project.tasks` and `person.tasks` both target `task`), and a reverse
    /// name belongs to exactly one of them. Only edges whose source is this
    /// type, or a type extending it, belong to the hop. `None` for a built-in
    /// relationship, which has no declaring schema, and for a forward name.
    pub source_type: Option<String>,
    /// The declared type of the nodes this hop reaches, if the schemas name
    /// one. `None` after a built-in relationship (any type may sit at either
    /// end) or an untyped declaration.
    pub far_type: Option<String>,
    /// Whether the schemas declare this side of the relationship as `many`.
    /// A declared-many hop is a collection however many nodes it currently
    /// reaches.
    pub declared_many: bool,
    /// Whether the hop came from a declaration with no target type. Such a
    /// declaration may point at any type, so it counts for a node only when
    /// an edge actually reaches it.
    pub untyped: bool,
    pub open_ended: bool,
}

/// A [`RelationshipPath`] resolved against the schemas: every name is a
/// concrete `relationship_type` and direction. This is the form that compiles
/// to SQL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ResolvedPath {
    pub hops: Vec<ResolvedHop>,
}

impl ResolvedPath {
    pub fn is_empty(&self) -> bool {
        self.hops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.hops.len()
    }

    /// The declared type of the nodes the whole path reaches, if any.
    pub fn far_type(&self) -> Option<&str> {
        self.hops.last().and_then(|hop| hop.far_type.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fixed_hop_is_its_name_on_the_wire() {
        let path = RelationshipPath::from_names(["child_of", "has_child"]);
        assert_eq!(
            serde_json::to_value(&path).unwrap(),
            json!(["child_of", "has_child"])
        );
        let back: RelationshipPath =
            serde_json::from_value(json!(["child_of", "has_child"])).unwrap();
        assert_eq!(back, path);
    }

    #[test]
    fn an_open_ended_hop_is_an_object_on_the_wire() {
        let path = RelationshipPath(vec![
            RelationshipHop::open_ended("child_of"),
            RelationshipHop::fixed("project"),
        ]);
        let wire = serde_json::to_value(&path).unwrap();
        assert_eq!(
            wire,
            json!([{ "name": "child_of", "open_ended": true }, "project"])
        );
        assert_eq!(
            serde_json::from_value::<RelationshipPath>(wire).unwrap(),
            path
        );
        assert_eq!(path.to_string(), "child_of*.project");
    }

    #[test]
    fn the_object_form_defaults_to_a_fixed_hop() {
        let path: RelationshipPath = serde_json::from_value(json!([{ "name": "tasks" }])).unwrap();
        assert_eq!(path, RelationshipPath::from_names(["tasks"]));
    }

    #[test]
    fn a_malformed_hop_is_rejected() {
        for bad in [
            json!([""]),
            json!([{ "name": "" }]),
            json!([{ "name": "tasks", "depth": 3 }]),
            json!([{ "open_ended": true }]),
            json!([7]),
            json!("child_of"),
        ] {
            assert!(
                serde_json::from_value::<RelationshipPath>(bad.clone()).is_err(),
                "{bad} must be rejected"
            );
        }
    }
}
