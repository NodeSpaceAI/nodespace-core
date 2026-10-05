//! What another build adds to core's data (ADR-082 §2, ADR-083 §2).
//!
//! An extension is named by an id: the entry a database lists in its settings
//! node's `required_extensions`, and the bucket its fields take in a core
//! relationship's edge properties.
//!
//! [`DataExtensions`] is what another build adds to a database's node
//! service: a [`NodeBehavior`] for each `extends` subtype of a core type it
//! defines (ADR-082 §2.1). [`crate::NodeService::new_with_extensions`] builds
//! a database's node service with it; [`crate::NodeService::new`] adds
//! nothing.

use std::sync::Arc;

use thiserror::Error;

use crate::behaviors::{BehaviorRegistrationError, NodeBehavior, NodeBehaviorRegistry};

#[cfg(test)]
mod fixture_tests;

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
/// of the `extends` subtypes of core types it defines.
///
/// Built once and shared by every database: each database's node service
/// registers the same behaviours in its own [`NodeBehaviorRegistry`].
/// [`DataExtensions::none`] adds nothing.
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
            if crate::services::node_service::normalize_schema_id(type_name) != type_name {
                return Err(DataExtensionsError::NotASchemaId(type_name.to_string()));
            }
            registry.register(behavior.clone())?;
        }
        Ok(registry)
    }

    /// Checks everything this value adds, as a daemon does at startup.
    ///
    /// # Errors
    ///
    /// The first problem found; see [`DataExtensionsError`].
    pub fn check(&self) -> Result<(), DataExtensionsError> {
        self.behavior_registry().map(|_| ())
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
        "'{0}' is not a schema id: create_schema stores a schema under its lowercase name, \
         with words joined by '_'"
    )]
    NotASchemaId(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviors::CustomNodeBehavior;
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

    #[test]
    fn a_behaviour_whose_type_is_not_a_schema_id_is_rejected() {
        for type_name in ["team-space", "Team", "team space"] {
            assert_eq!(
                DataExtensions::none().behavior(behavior(type_name)).check(),
                Err(DataExtensionsError::NotASchemaId(type_name.to_string())),
            );
        }
        assert_eq!(
            DataExtensions::none()
                .behavior(behavior("team_space"))
                .check(),
            Ok(())
        );
    }
}
