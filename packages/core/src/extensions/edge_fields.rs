//! Fields another build adds to a core relationship (ADR-082 §2.2).
//!
//! A declaration names a relationship, the registering extension's id, and
//! the typed fields it adds, with an optional validation function. The fields
//! are stored in the extension's bucket of the edge's properties,
//! `properties.<extension id>.*`, so they never collide with a field core
//! adds to the relationship later. A database's node service validates a
//! registered bucket on every edge write it is handed. Core never reads or
//! validates a bucket nobody registered, and never writes one itself.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::models::schema::{EdgeField, SchemaFieldType};
use crate::services::NodeService;

/// The core relationships another build may add fields to.
///
/// Core never rewrites the properties of these edges, so a bucket written on
/// one stays until a write replaces the edge's properties. A `has_child` edge
/// is recreated when its node moves to another parent and a `mentions` edge
/// follows its node's content, so a bucket on either would be lost.
pub const EDGE_FIELD_RELATIONSHIPS: [&str; 2] = ["member_of", "has_role"];

/// The keys core itself stores in the properties of its relationships
/// (ADR-083 §2): an extension's bucket may not take one of these names.
const CORE_EDGE_KEYS: [&str; 1] = ["order"];

/// The types an edge field may have: the scalar ones.
const EDGE_FIELD_TYPES: [SchemaFieldType; 6] = [
    SchemaFieldType::Text,
    SchemaFieldType::Number,
    SchemaFieldType::Boolean,
    SchemaFieldType::Date,
    SchemaFieldType::Datetime,
    SchemaFieldType::Enum,
];

/// Checks an extension's bucket once its fields are type-checked, given the
/// bucket's fields by name. An `Err` refuses the write with its message.
pub type EdgeFieldValidator = Arc<dyn Fn(&Map<String, Value>) -> Result<(), String> + Send + Sync>;

/// Fields another build adds to a core relationship (ADR-082 §2.2), stored in
/// the build's bucket of the edge's properties: `properties.<extension id>`.
///
/// Each field is an [`EdgeField`] with a name and a scalar type (`text`,
/// `number`, `boolean`, `date`, `datetime` or `enum`, an enum with its
/// values), optionally `required`. A bucket present on an edge must be an
/// object holding only declared fields, each of its declared type or null,
/// and every required field set; then the validator, if any, runs.
///
/// ```
/// use nodespace_core::extensions::{DataExtensions, EdgeFieldDeclaration};
/// use nodespace_core::models::schema::{EdgeField, SchemaFieldType};
///
/// let note = EdgeField {
///     name: "note".to_string(),
///     field_type: SchemaFieldType::Text,
///     core_values: None,
///     indexed: None,
///     required: None,
///     default: None,
///     target_type: None,
///     description: None,
/// };
/// let declaration = EdgeFieldDeclaration::new("member_of", "fixture", vec![note])
///     .with_validator(|bucket| match bucket.get("note") {
///         Some(note) if note.as_str() == Some("") => Err("a note is not empty".to_string()),
///         _ => Ok(()),
///     });
/// assert!(DataExtensions::none().edge_fields(declaration).check().is_ok());
/// ```
#[derive(Clone)]
pub struct EdgeFieldDeclaration {
    relationship: String,
    extension_id: String,
    fields: Vec<EdgeField>,
    validator: Option<EdgeFieldValidator>,
}

impl EdgeFieldDeclaration {
    /// The `fields` extension `extension_id` adds to `relationship` edges.
    pub fn new(
        relationship: impl Into<String>,
        extension_id: impl Into<String>,
        fields: Vec<EdgeField>,
    ) -> Self {
        Self {
            relationship: relationship.into(),
            extension_id: extension_id.into(),
            fields,
            validator: None,
        }
    }

    /// Adds a check of the whole bucket, run after its fields are
    /// type-checked: for a rule between fields, or one a field's type cannot
    /// express. A second call replaces the first.
    pub fn with_validator<F>(mut self, validator: F) -> Self
    where
        F: Fn(&Map<String, Value>) -> Result<(), String> + Send + Sync + 'static,
    {
        self.validator = Some(Arc::new(validator));
        self
    }

    /// The relationship the fields are added to.
    pub fn relationship(&self) -> &str {
        &self.relationship
    }

    /// The registering extension's id, which names the bucket.
    pub fn extension_id(&self) -> &str {
        &self.extension_id
    }

    /// The fields the extension adds.
    pub fn fields(&self) -> &[EdgeField] {
        &self.fields
    }

    /// What is wrong with this declaration, or `None` when it can be
    /// registered.
    pub(crate) fn problem(&self) -> Option<String> {
        if !EDGE_FIELD_RELATIONSHIPS.contains(&self.relationship.as_str()) {
            return Some(format!(
                "fields can be added only to {} edges",
                EDGE_FIELD_RELATIONSHIPS.join(" and ")
            ));
        }
        if !super::is_extension_id(&self.extension_id) {
            return Some(
                "the extension id must be a lowercase letter followed by lowercase letters, \
                 digits, '-' or '_'"
                    .to_string(),
            );
        }
        if CORE_EDGE_KEYS.contains(&self.extension_id.as_str()) {
            return Some(format!(
                "'{}' is a key core stores on its edges and cannot name a bucket",
                self.extension_id
            ));
        }
        if self.fields.is_empty() {
            return Some("no fields are declared".to_string());
        }
        for (i, field) in self.fields.iter().enumerate() {
            if let Some(problem) = Self::field_problem(field) {
                return Some(format!("field '{}': {problem}", field.name));
            }
            if self.fields[..i].iter().any(|f| f.name == field.name) {
                return Some(format!("field '{}' is declared twice", field.name));
            }
        }
        None
    }

    fn field_problem(field: &EdgeField) -> Option<String> {
        if field.name.is_empty() {
            return Some("a field needs a name".to_string());
        }
        if !EDGE_FIELD_TYPES.contains(&field.field_type) {
            return Some(format!(
                "an edge field cannot be of type '{}'; use one of: {}",
                field.field_type.as_str(),
                EDGE_FIELD_TYPES.map(SchemaFieldType::as_str).join(", ")
            ));
        }
        if field.field_type == SchemaFieldType::Enum {
            if field.core_values.as_ref().is_none_or(Vec::is_empty) {
                return Some("an enum field needs its values".to_string());
            }
        } else if field.core_values.is_some() {
            return Some("only an enum field takes values".to_string());
        }
        if field.default.is_some() {
            return Some(
                "core never writes an extension's bucket, so a field takes no default".to_string(),
            );
        }
        if field.indexed == Some(true) {
            return Some("edge fields are not indexed".to_string());
        }
        if field.target_type.is_some() {
            return Some("an edge field takes no target type".to_string());
        }
        None
    }

    /// Checks the extension's bucket in `properties`, an edge's properties.
    /// An edge without the bucket passes: its fields are not set.
    fn validate(&self, properties: &Map<String, Value>) -> Result<(), String> {
        let id = &self.extension_id;
        let relationship = &self.relationship;
        let Some(bucket) = properties.get(id) else {
            return Ok(());
        };
        let Some(bucket) = bucket.as_object() else {
            return Err(format!(
                "'{id}' in the properties of a '{relationship}' edge holds the fields extension \
                 '{id}' adds and must be a JSON object, got {}",
                NodeService::describe_received(bucket)
            ));
        };
        if let Some(key) = bucket
            .keys()
            .find(|key| !self.fields.iter().any(|f| &f.name == *key))
        {
            return Err(format!(
                "'{key}' is not a field extension '{id}' adds to '{relationship}' edges; its \
                 fields are: {}",
                self.fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for field in &self.fields {
            match bucket.get(&field.name) {
                None | Some(Value::Null) => {
                    if field.required == Some(true) {
                        return Err(format!(
                            "'{id}.{}' is required on a '{relationship}' edge that carries \
                             '{id}' fields",
                            field.name
                        ));
                    }
                }
                Some(value) => {
                    if let Some(expected) = Self::type_mismatch(field, value) {
                        return Err(format!(
                            "'{id}.{}' on a '{relationship}' edge must be {expected}, got {}",
                            field.name,
                            NodeService::describe_received(value)
                        ));
                    }
                }
            }
        }
        if let Some(validator) = &self.validator {
            validator(bucket)
                .map_err(|reason| format!("'{id}' fields on a '{relationship}' edge: {reason}"))?;
        }
        Ok(())
    }

    /// What `value` should have been for `field`, or `None` when it fits.
    fn type_mismatch(field: &EdgeField, value: &Value) -> Option<String> {
        match field.field_type {
            SchemaFieldType::Text => (!value.is_string()).then(|| "a string".to_string()),
            SchemaFieldType::Enum => {
                let values = field.core_values.as_deref().unwrap_or_default();
                let admitted = value
                    .as_str()
                    .is_some_and(|v| values.iter().any(|ev| ev.value == v));
                (!admitted).then(|| {
                    format!(
                        "one of: {}",
                        values
                            .iter()
                            .map(|ev| ev.value.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
            }
            other => NodeService::scalar_mismatch(other, value)
                .map(|expected| format!("a {}{expected}", other.as_str())),
        }
    }
}

impl std::fmt::Debug for EdgeFieldDeclaration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EdgeFieldDeclaration")
            .field("relationship", &self.relationship)
            .field("extension_id", &self.extension_id)
            .field("fields", &self.fields)
            .field("validator", &self.validator.is_some())
            .finish()
    }
}

/// The edge-field declarations a database's node service validates edge
/// writes with. Empty unless another build registered some.
#[derive(Clone, Debug, Default)]
pub(crate) struct EdgeFieldRegistry {
    declarations: Vec<EdgeFieldDeclaration>,
}

impl EdgeFieldRegistry {
    pub(crate) fn new(declarations: Vec<EdgeFieldDeclaration>) -> Self {
        Self { declarations }
    }

    /// Checks every bucket registered for `relationship` in `properties`, the
    /// properties an edge write is about to store. Properties that are not an
    /// object hold no bucket.
    pub(crate) fn validate(&self, relationship: &str, properties: &Value) -> Result<(), String> {
        let Some(properties) = properties.as_object() else {
            return Ok(());
        };
        self.declarations
            .iter()
            .filter(|d| d.relationship == relationship)
            .try_for_each(|d| d.validate(properties))
    }
}
