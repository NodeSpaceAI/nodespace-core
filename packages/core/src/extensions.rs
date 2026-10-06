//! What another build adds to core's data (ADR-082 §2, ADR-083 §2).
//!
//! An extension is named by an id: the entry a database lists in its settings
//! node's `required_extensions`, and the bucket its fields take in a core
//! relationship's edge properties.
//!
//! [`DataExtensions`] is what another build adds to a database's node
//! service: a [`NodeBehavior`] for each `extends` subtype of a core type it
//! defines (ADR-082 §2.1), and fields on core relationships (ADR-082 §2.2,
//! [`EdgeFieldDeclaration`]). [`crate::NodeService::new_with_extensions`]
//! builds a database's node service with it; [`crate::NodeService::new`] adds
//! nothing.

use std::sync::Arc;

use thiserror::Error;

use crate::behaviors::{BehaviorRegistrationError, NodeBehavior, NodeBehaviorRegistry};

mod edge_fields;
#[cfg(test)]
mod fixture_tests;

pub(crate) use edge_fields::EdgeFieldRegistry;
pub use edge_fields::{EdgeFieldDeclaration, EDGE_FIELD_RELATIONSHIPS};

/// Whether `id` has the form of an extension id: a lowercase ASCII letter,
/// then lowercase ASCII letters, digits, `-` or `_`.
///
/// ```
/// use nodespace_core::extensions::is_extension_id;
///
/// assert!(is_extension_id("fixture"));
/// assert!(!is_extension_id("Fixture"));
/// ```
pub fn is_extension_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// What another build adds to core's data model (ADR-082 §2): the behaviours
/// of the `extends` subtypes of core types it defines, and the fields it adds
/// to core relationships.
///
/// Built once and shared by every database: each database's node service
/// registers the same behaviours in its own [`NodeBehaviorRegistry`] and
/// validates the same edge fields. [`DataExtensions::none`] adds nothing.
///
/// ```
/// use std::sync::Arc;
/// use nodespace_core::behaviors::CustomNodeBehavior;
/// use nodespace_core::extensions::DataExtensions;
///
/// let data = DataExtensions::none().behavior(Arc::new(CustomNodeBehavior::new("team")));
/// assert!(data.check().is_ok());
/// assert!(data.behavior_registry().unwrap().get("team").is_some());
///
/// // `collection` is a core type: its behaviour cannot be replaced.
/// let core = DataExtensions::none().behavior(Arc::new(CustomNodeBehavior::new("collection")));
/// assert!(core.check().is_err());
/// ```
#[derive(Clone, Default)]
pub struct DataExtensions {
    behaviors: Vec<Arc<dyn NodeBehavior>>,
    edge_fields: Vec<EdgeFieldDeclaration>,
}

impl DataExtensions {
    /// Nothing added: core's own data model.
    pub fn none() -> Self {
        Self::default()
    }

    /// Adds the behaviour of an `extends` subtype of a core type (ADR-082
    /// §2.1), keyed by its `type_name()`.
    ///
    /// The behaviour adds to the rules of the types its type extends: a node
    /// is validated by every behaviour in its type's chain, base first, so a
    /// subtype can reject more and never less. The nearest behaviour in the
    /// chain also decides embedding, markdown and content rules.
    ///
    /// The type is the subtype's schema id. The build creates that schema, with
    /// `extends`, in each database it uses; without it a node of the type is
    /// refused as an unknown type.
    ///
    /// Rejected by [`Self::check`]: a behaviour for a core type, a second
    /// behaviour for one type, and a type that is not a schema id
    /// `create_schema` can store.
    pub fn behavior(mut self, behavior: Arc<dyn NodeBehavior>) -> Self {
        self.behaviors.push(behavior);
        self
    }

    /// Adds fields to a core relationship (ADR-082 §2.2), stored in the
    /// declaring extension's bucket of each edge's properties,
    /// `properties.<extension id>`. A database's node service validates that
    /// bucket on every edge write it is handed: through `create_relationship`,
    /// the transactional relationship writes and `update_relationship_properties`.
    /// Another bucket on the edge, registered by nobody, is left as it is.
    ///
    /// Rejected by [`Self::check`]: a relationship other than those in
    /// [`EDGE_FIELD_RELATIONSHIPS`], an extension id that is not one or names
    /// a key core stores on its edges (`order`), a second declaration for one
    /// relationship and id, and a declaration without fields or with a field
    /// that is unnamed, repeated, not a scalar type, an enum without values or
    /// with a value listed twice, a non-enum with values, or one that carries a
    /// default, an index or a target type.
    pub fn edge_fields(mut self, declaration: EdgeFieldDeclaration) -> Self {
        self.edge_fields.push(declaration);
        self
    }

    /// The behaviour registry a database's node service validates with:
    /// core's behaviours and every behaviour added here.
    ///
    /// # Errors
    ///
    /// What [`Self::check`] reports.
    pub fn behavior_registry(&self) -> Result<NodeBehaviorRegistry, DataExtensionsError> {
        let mut registry = NodeBehaviorRegistry::new();
        for behavior in &self.behaviors {
            let type_name = behavior.type_name();
            // A core type is refused as one first: its id is kebab-case too,
            // but `create_schema` refuses a name that derives it.
            registry.register(behavior.clone())?;
            if type_name.is_empty()
                || crate::services::node_service::normalize_schema_id(type_name) != type_name
            {
                return Err(DataExtensionsError::NotASchemaId(type_name.to_string()));
            }
        }
        Ok(registry)
    }

    /// The edge-field declarations a database's node service validates with.
    pub(crate) fn edge_field_registry(&self) -> Result<EdgeFieldRegistry, DataExtensionsError> {
        for (i, declaration) in self.edge_fields.iter().enumerate() {
            let refused = |reason: String| DataExtensionsError::EdgeFields {
                relationship: declaration.relationship().to_string(),
                extension_id: declaration.extension_id().to_string(),
                reason,
            };
            if let Some(problem) = declaration.problem() {
                return Err(refused(problem));
            }
            if self.edge_fields[..i].iter().any(|earlier| {
                earlier.relationship() == declaration.relationship()
                    && earlier.extension_id() == declaration.extension_id()
            }) {
                return Err(refused("they are declared twice".to_string()));
            }
        }
        Ok(EdgeFieldRegistry::new(self.edge_fields.clone()))
    }

    /// Checks everything this value adds, as a daemon does at startup.
    ///
    /// # Errors
    ///
    /// The first problem found; see [`DataExtensionsError`].
    pub fn check(&self) -> Result<(), DataExtensionsError> {
        self.behavior_registry()?;
        self.edge_field_registry().map(|_| ())
    }
}

impl std::fmt::Debug for DataExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataExtensions")
            .field(
                "behaviors",
                &self
                    .behaviors
                    .iter()
                    .map(|b| b.type_name())
                    .collect::<Vec<_>>(),
            )
            .field("edge_fields", &self.edge_fields)
            .finish()
    }
}

/// Why a [`DataExtensions`] was refused.
///
/// `non_exhaustive`: a new hook adds the ways it can be refused, which is a
/// minor change to the extension API.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum DataExtensionsError {
    /// A behaviour names a core type, or a type that already has one.
    #[error(transparent)]
    Behavior(#[from] BehaviorRegistrationError),
    /// A behaviour's type is not a schema id `create_schema` can store, so no
    /// node would ever have it.
    #[error(
        "'{0}' is not a schema id: a type id is kebab-case, lowercase words joined by '-' \
         (e.g. 'customer-profile')"
    )]
    NotASchemaId(String),
    /// An edge-field declaration was refused (see
    /// [`DataExtensions::edge_fields`]).
    #[error("the fields '{extension_id}' adds to '{relationship}' edges: {reason}")]
    EdgeFields {
        relationship: String,
        extension_id: String,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviors::CustomNodeBehavior;
    use crate::models::schema::{EdgeField, EnumValue, SchemaFieldType};
    use crate::models::CoreNodeType;

    #[test]
    fn an_extension_id_is_a_lowercase_letter_then_letters_digits_dashes_or_underscores() {
        for valid in ["fixture", "ext", "a", "x2", "my-ext", "my_ext"] {
            assert!(is_extension_id(valid), "{valid:?} is an extension id");
        }
        for invalid in ["", "Ext", "2x", "-x", "_x", "my ext", "my.ext", "é"] {
            assert!(
                !is_extension_id(invalid),
                "{invalid:?} is not an extension id"
            );
        }
    }

    fn behavior(type_name: &str) -> Arc<dyn NodeBehavior> {
        Arc::new(CustomNodeBehavior::new(type_name))
    }

    #[test]
    fn no_extension_gives_exactly_the_core_registry() {
        let mut registered = DataExtensions::none()
            .behavior_registry()
            .unwrap()
            .get_all_types();
        registered.sort();
        let mut core: Vec<String> = CoreNodeType::ALL
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        core.sort();
        assert_eq!(registered, core);
    }

    #[test]
    fn a_behaviour_for_a_core_type_or_a_second_one_for_a_type_is_rejected() {
        assert_eq!(
            DataExtensions::none()
                .behavior(behavior("collection"))
                .check(),
            Err(DataExtensionsError::Behavior(
                BehaviorRegistrationError::CoreType("collection".to_string())
            ))
        );
        assert_eq!(
            DataExtensions::none()
                .behavior(behavior("team"))
                .behavior(behavior("team"))
                .check(),
            Err(DataExtensionsError::Behavior(
                BehaviorRegistrationError::AlreadyRegistered("team".to_string())
            ))
        );
    }

    fn field(name: &str, field_type: SchemaFieldType) -> EdgeField {
        EdgeField {
            name: name.to_string(),
            field_type,
            core_values: (field_type == SchemaFieldType::Enum)
                .then(|| vec![EnumValue::new("a", "A")]),
            indexed: None,
            required: None,
            default: None,
            target_type: None,
            description: None,
        }
    }

    fn refusal(declaration: EdgeFieldDeclaration) -> Option<String> {
        match DataExtensions::none().edge_fields(declaration).check() {
            Ok(()) => None,
            Err(DataExtensionsError::EdgeFields { reason, .. }) => Some(reason),
            Err(other) => panic!("not an edge-field refusal: {other}"),
        }
    }

    #[test]
    fn an_edge_field_declaration_on_member_of_or_has_role_is_accepted() {
        for &relationship in EDGE_FIELD_RELATIONSHIPS {
            let fields = SchemaFieldType::ALL
                .into_iter()
                .filter(|t| {
                    !matches!(
                        t,
                        SchemaFieldType::Array | SchemaFieldType::Object | SchemaFieldType::Link
                    )
                })
                .map(|t| field(t.as_str(), t))
                .collect();
            assert_eq!(
                refusal(EdgeFieldDeclaration::new(relationship, "fixture", fields)),
                None,
                "{relationship}"
            );
        }
    }

    #[test]
    fn a_malformed_edge_field_declaration_is_rejected() {
        let ok = || vec![field("note", SchemaFieldType::Text)];
        let with = |change: fn(&mut EdgeField)| {
            let mut f = field("note", SchemaFieldType::Text);
            change(&mut f);
            EdgeFieldDeclaration::new("member_of", "fixture", vec![f])
        };
        let cases: Vec<(EdgeFieldDeclaration, &str)> = vec![
            (
                EdgeFieldDeclaration::new("has_child", "fixture", ok()),
                "only to member_of and has_role",
            ),
            (
                EdgeFieldDeclaration::new("mentions", "fixture", ok()),
                "only to member_of and has_role",
            ),
            (
                EdgeFieldDeclaration::new("member_of", "Fixture", ok()),
                "must be a lowercase letter",
            ),
            (
                EdgeFieldDeclaration::new("member_of", "order", ok()),
                "a key core stores on its edges",
            ),
            (
                EdgeFieldDeclaration::new("member_of", "fixture", vec![]),
                "no fields are declared",
            ),
            (
                EdgeFieldDeclaration::new(
                    "member_of",
                    "fixture",
                    vec![
                        field("note", SchemaFieldType::Text),
                        field("note", SchemaFieldType::Number),
                    ],
                ),
                "declared twice",
            ),
            (with(|f| f.name.clear()), "a field needs a name"),
            (
                with(|f| f.field_type = SchemaFieldType::Object),
                "cannot be of type 'object'",
            ),
            (
                with(|f| f.field_type = SchemaFieldType::Enum),
                "an enum field needs its values",
            ),
            (
                with(|f| {
                    f.field_type = SchemaFieldType::Enum;
                    f.core_values =
                        Some(vec![EnumValue::new("a", "A"), EnumValue::new("a", "Again")]);
                }),
                "the value 'a' is listed twice",
            ),
            (
                with(|f| f.core_values = Some(vec![EnumValue::new("a", "A")])),
                "only an enum field takes values",
            ),
            (
                with(|f| f.default = Some(serde_json::json!("x"))),
                "takes no default",
            ),
            (with(|f| f.indexed = Some(true)), "not indexed"),
            (
                with(|f| f.target_type = Some("person".to_string())),
                "takes no target type",
            ),
        ];
        for (declaration, expected) in cases {
            let described = format!("{declaration:?}");
            let reason = refusal(declaration).unwrap_or_else(|| panic!("accepted: {described}"));
            assert!(reason.contains(expected), "{described}: {reason}");
        }

        let twice = DataExtensions::none()
            .edge_fields(EdgeFieldDeclaration::new("member_of", "fixture", ok()))
            .edge_fields(EdgeFieldDeclaration::new("member_of", "fixture", ok()));
        assert!(matches!(
            twice.check(),
            Err(DataExtensionsError::EdgeFields { ref reason, .. }) if reason.contains("declared twice")
        ));
        let elsewhere = DataExtensions::none()
            .edge_fields(EdgeFieldDeclaration::new("member_of", "fixture", ok()))
            .edge_fields(EdgeFieldDeclaration::new("has_role", "fixture", ok()))
            .edge_fields(EdgeFieldDeclaration::new("member_of", "other", ok()));
        assert_eq!(elsewhere.check(), Ok(()));
    }

    #[test]
    fn a_behaviour_whose_type_is_not_a_schema_id_is_rejected() {
        assert_eq!(
            DataExtensions::none()
                .behavior(behavior("code-block"))
                .check(),
            Err(DataExtensionsError::Behavior(
                BehaviorRegistrationError::CoreType("code-block".to_string())
            )),
            "a core type is reported as one"
        );
        for type_name in [
            "team_space",
            "Team",
            "team space",
            "-team",
            "team--space",
            "",
        ] {
            assert_eq!(
                DataExtensions::none().behavior(behavior(type_name)).check(),
                Err(DataExtensionsError::NotASchemaId(type_name.to_string())),
            );
        }
        let message = DataExtensionsError::NotASchemaId("team_space".to_string()).to_string();
        assert!(message.contains("kebab-case"), "{message}");
        assert_eq!(
            DataExtensions::none()
                .behavior(behavior("fixture-collection"))
                .check(),
            Ok(())
        );
    }
}
