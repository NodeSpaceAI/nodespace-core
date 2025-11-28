//! Natural Language Schema Generation
//!
//! Converts natural language descriptions into schema definitions with intelligent
//! field type inference and namespace enforcement.
//!
//! As of Issue #676, all handlers use NodeService directly instead of NodeOperations.

use crate::mcp::types::MCPError;
use crate::models::schema::{ProtectionLevel, SchemaDefinition, SchemaField};
use crate::operations::CreateNodeParams;
use crate::services::NodeService;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Reserved core property names that conflict with system properties
const RESERVED_CORE_PROPERTIES: &[&str] = &[
    "id",
    "node_type",
    "content",
    "parent_id",
    "root_id",
    "created_at",
    "modified_at",
    "status",
    "priority",
    "due_date",
    "due",
];

/// Input parameters for create_entity_schema_from_description
#[derive(Debug, Deserialize)]
pub struct CreateEntitySchemaFromDescriptionParams {
    /// Entity name (e.g., "Invoice", "Customer")
    pub entity_name: String,
    /// Natural language description of entity fields
    pub description: String,
    /// Optional additional constraints for explicit type hints
    #[serde(default)]
    pub additional_constraints: Option<AdditionalConstraints>,
}

/// Additional constraints for schema creation
#[derive(Debug, Deserialize)]
pub struct AdditionalConstraints {
    /// List of field names that are required
    #[serde(default)]
    pub required_fields: Option<Vec<String>>,
    /// Default values for specific fields
    #[serde(default)]
    pub default_values: Option<std::collections::HashMap<String, Value>>,
    /// Enum values for specific fields
    #[serde(default)]
    pub enum_values: Option<std::collections::HashMap<String, Vec<String>>>,
}

/// Output from schema creation
#[derive(Debug, Serialize)]
pub struct CreateEntitySchemaOutput {
    /// ID for the generated schema (snake_case of entity name)
    pub schema_id: String,
    /// Full schema definition
    pub schema_definition: SchemaDefinition,
    /// List of created fields
    pub created_fields: Vec<SchemaField>,
    /// Optional warnings about ambiguous descriptions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

/// Inferred field information from natural language
#[derive(Debug, Clone)]
struct InferredField {
    name: String,
    field_type: String,
    required: bool,
    enum_values: Option<Vec<String>>,
    warnings: Vec<String>,
}

/// Create entity schema from natural language description
///
/// # MCP Tool Description
/// Convert a natural language description of entity fields into a complete schema definition
/// with intelligent type inference. Automatically enforces namespace prefixes for user-defined
/// fields to prevent conflicts with future core properties.
///
/// # Parameters
/// - `entity_name`: Name of the entity (e.g., "Invoice", "Customer")
/// - `description`: Natural language description of fields
/// - `additional_constraints`: Optional explicit type hints and enum values
///
/// # Returns
/// - `schema_id`: Generated schema ID (snake_case)
/// - `schema_definition`: Complete schema with all fields
/// - `created_fields`: List of inferred fields
/// - `warnings`: Any ambiguities or assumptions made
///
/// # Errors
/// - `INVALID_PARAMS`: If description is empty or entity_name invalid
/// - `INTERNAL_ERROR`: If schema creation fails
pub async fn handle_create_entity_schema_from_description(
    node_service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: CreateEntitySchemaFromDescriptionParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    if params.entity_name.trim().is_empty() {
        return Err(MCPError::invalid_params(
            "entity_name cannot be empty".to_string(),
        ));
    }

    if params.description.trim().is_empty() {
        return Err(MCPError::invalid_params(
            "description cannot be empty".to_string(),
        ));
    }

    // Parse natural language and infer fields
    let inferred_fields = parse_field_descriptions(&params.description);

    // Apply additional constraints
    let fields = apply_constraints(inferred_fields, params.additional_constraints);

    // Normalize field names to snake_case and apply namespace prefixes
    let namespaced_fields = normalize_and_namespace_fields(fields.clone());

    // Collect warnings
    let warnings = fields
        .iter()
        .flat_map(|f| f.warnings.clone())
        .collect::<Vec<_>>();

    // Generate schema ID
    let schema_id = normalize_schema_id(&params.entity_name);

    // Create schema definition
    let schema_definition = SchemaDefinition {
        is_core: false,
        version: 1,
        description: format!("Auto-generated schema for {}", params.entity_name),
        fields: namespaced_fields.clone(),
    };

    // Create schema node params
    let schema_node_params = CreateNodeParams {
        id: Some(schema_id.clone()),
        node_type: "schema".to_string(),
        content: params.entity_name.clone(),
        parent_id: None,
        insert_after_node_id: None,
        properties: serde_json::to_value(&schema_definition)
            .map_err(|e| MCPError::internal_error(format!("Failed to serialize schema: {}", e)))?,
    };

    // Store the schema node
    node_service
        .create_node_with_parent(schema_node_params)
        .await
        .map_err(|e| {
            MCPError::internal_error(format!(
                "Failed to create schema node for '{}': {}",
                schema_id, e
            ))
        })?;

    let output = CreateEntitySchemaOutput {
        schema_id: schema_id.clone(),
        schema_definition,
        created_fields: namespaced_fields,
        warnings: if warnings.is_empty() {
            None
        } else {
            Some(warnings)
        },
    };

    serde_json::to_value(&output)
        .map_err(|e| MCPError::internal_error(format!("Failed to serialize output: {}", e)))
}

/// Parse natural language description and extract fields
fn parse_field_descriptions(description: &str) -> Vec<InferredField> {
    let mut fields = Vec::new();

    // Split by common delimiters: commas, "and", semicolons
    let field_descriptions = split_field_descriptions(description);

    for field_desc in field_descriptions {
        if let Some(inferred) = parse_single_field_description(&field_desc, description) {
            fields.push(inferred);
        }
    }

    fields
}

/// Split description into individual field descriptions
fn split_field_descriptions(description: &str) -> Vec<String> {
    // Split by comma, semicolon, or " and "
    let parts: Vec<&str> = description
        .split([',', ';'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    // Further split by " and " within each part
    let mut result = Vec::new();
    for part in parts {
        let subparts: Vec<&str> = part.split(" and ").map(|s| s.trim()).collect();
        for subpart in subparts {
            if !subpart.is_empty() {
                result.push(subpart.to_string());
            }
        }
    }

    result
}

/// Parse a single field description and infer its properties
fn parse_single_field_description(
    field_desc: &str,
    _full_description: &str,
) -> Option<InferredField> {
    let field_desc = field_desc.trim();
    if field_desc.is_empty() {
        return None;
    }

    // Extract field name (first word or phrase before parentheses/keywords)
    let field_name = extract_field_name(field_desc)?;

    // Infer field type
    let field_type = infer_field_type(field_desc, &field_name);

    // Extract enum values if present
    let enum_values = extract_enum_values(field_desc);

    // Check if field is required
    let required = is_field_required(field_desc);

    // Collect warnings
    let mut warnings = Vec::new();
    if field_type == "string" && contains_any(field_desc, &["amount", "price", "cost"]) {
        warnings.push(format!(
            "Field '{}' mentions monetary amount but inferred as string. Consider using number type.",
            field_name
        ));
    }

    Some(InferredField {
        name: field_name,
        field_type,
        required,
        enum_values,
        warnings,
    })
}

/// Extract field name from description
fn extract_field_name(field_desc: &str) -> Option<String> {
    // Pattern 1: "field_name (required)" or "field_name (something/else)"
    if let Some(name) = extract_before_paren(field_desc) {
        return Some(name);
    }

    // Pattern 2: "field_name in USD" or similar
    if let Some(name) = extract_before_keyword(field_desc, &["in ", "with ", "that "]) {
        return Some(name);
    }

    // Pattern 3: Just take the first few words until a keyword
    let words: Vec<&str> = field_desc.split_whitespace().take(3).collect();
    if !words.is_empty() {
        let combined = words.join("_").to_lowercase();
        if !combined.is_empty() && combined.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some(combined);
        }
    }

    None
}

/// Extract text before first parenthesis
fn extract_before_paren(text: &str) -> Option<String> {
    if let Some(idx) = text.find('(') {
        let name = text[..idx].trim();
        if !name.is_empty() {
            return Some(normalize_field_name(name));
        }
    }
    None
}

/// Extract text before specific keyword
fn extract_before_keyword(text: &str, keywords: &[&str]) -> Option<String> {
    for keyword in keywords {
        if let Some(idx) = text.find(keyword) {
            let name = text[..idx].trim();
            if !name.is_empty() {
                return Some(normalize_field_name(name));
            }
        }
    }
    None
}

/// Normalize field name to snake_case
fn normalize_field_name(name: &str) -> String {
    name.to_lowercase()
        .replace([' ', '-'], "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// Infer field type from description
///
/// # Type Inference Priority (in order of precedence)
/// 1. **Date/Time** - Keywords: "date", "deadline", "due", "created", etc.
///    - Highest priority because date/time are specific and unambiguous
/// 2. **Boolean** - Keywords: "yes/no", "enabled/disabled", "active/inactive"
///    - High priority because boolean is explicit
/// 3. **Numeric** - Keywords: "amount", "price", "quantity", "count", "usd"
///    - Also checked in field name (e.g., "invoice_amount" → number)
/// 4. **Enum** - Pattern: "(option1/option2)" with parentheses and forward slashes
///    - High specificity, explicit syntax
/// 5. **Array** - Keywords: "list", "items", "tags", "array", "multiple"
/// 6. **String** - Default fallback for any ambiguous descriptions
///
/// Note: If a description mentions "enabled date", the date check will match first,
/// so it returns "date" (most specific keyword wins).
fn infer_field_type(field_desc: &str, field_name: &str) -> String {
    let lower = field_desc.to_lowercase();
    let name_lower = field_name.to_lowercase();

    // Priority 1: Check for date/time (most specific, highest priority)
    if contains_any(
        &lower,
        &[
            "date",
            "when",
            "time",
            "deadline",
            "due",
            "expires",
            "scheduled",
            "created",
        ],
    ) {
        return "date".to_string();
    }

    // Priority 2: Check for boolean (explicit yes/no values)
    if contains_any(
        &lower,
        &[
            "yes", "no", "enabled", "disabled", "active", "inactive", "true", "false",
        ],
    ) {
        return "boolean".to_string();
    }

    // Priority 3: Check for numeric values
    if contains_any(
        &lower,
        &[
            "amount",
            "price",
            "cost",
            "count",
            "number",
            "quantity",
            "value",
            "total",
            "sum",
            "average",
            "percentage",
            "rate",
            "usd",
            "dollars",
            "euros",
            "cents",
        ],
    ) || contains_any(
        &name_lower,
        &["amount", "price", "cost", "count", "quantity"],
    ) {
        return "number".to_string();
    }

    // Priority 4: Check for enum (explicit option syntax)
    if field_desc.contains('(') && field_desc.contains('/') {
        return "enum".to_string();
    }

    // Priority 5: Check for array/collection types
    if contains_any(&lower, &["list", "items", "tags", "array", "multiple"]) {
        return "array".to_string();
    }

    // Priority 6: Default to string for any ambiguous descriptions
    "string".to_string()
}

/// Extract enum values from "(option1/option2)" pattern
fn extract_enum_values(field_desc: &str) -> Option<Vec<String>> {
    // Match pattern: (value1/value2/value3)
    if let Some(start) = field_desc.find('(') {
        if let Some(end) = field_desc.find(')') {
            if start < end {
                let content = &field_desc[start + 1..end];
                if content.contains('/') {
                    let values: Vec<String> = content
                        .split('/')
                        .map(|s| s.trim().to_uppercase())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !values.is_empty() {
                        return Some(values);
                    }
                }
            }
        }
    }
    None
}

/// Check if field is marked as required
fn is_field_required(field_desc: &str) -> bool {
    let lower = field_desc.to_lowercase();
    contains_any(
        &lower,
        &["required", "must", "mandatory", "essential", "critical"],
    )
}

/// Check if any keyword is contained in text
fn contains_any(text: &str, keywords: &[&str]) -> bool {
    let lower = text.to_lowercase();
    keywords.iter().any(|kw| lower.contains(kw))
}

/// Apply additional constraints to inferred fields
fn apply_constraints(
    mut fields: Vec<InferredField>,
    constraints: Option<AdditionalConstraints>,
) -> Vec<InferredField> {
    let constraints = match constraints {
        Some(c) => c,
        None => return fields,
    };

    // Apply required field constraints
    if let Some(required_list) = constraints.required_fields {
        for field in &mut fields {
            if required_list.iter().any(|req| {
                req.to_lowercase().contains(&field.name.to_lowercase())
                    || field.name.to_lowercase().contains(&req.to_lowercase())
            }) {
                field.required = true;
            }
        }
    }

    // Apply enum value constraints
    if let Some(enum_map) = constraints.enum_values {
        for field in &mut fields {
            for (enum_field, values) in &enum_map {
                if enum_field
                    .to_lowercase()
                    .contains(&field.name.to_lowercase())
                    || field
                        .name
                        .to_lowercase()
                        .contains(&enum_field.to_lowercase())
                {
                    field.field_type = "enum".to_string();
                    field.enum_values = Some(values.iter().map(|v| v.to_uppercase()).collect());
                }
            }
        }
    }

    fields
}

/// Normalize field names and apply namespace prefixes
fn normalize_and_namespace_fields(inferred_fields: Vec<InferredField>) -> Vec<SchemaField> {
    inferred_fields
        .into_iter()
        .map(|mut inferred| {
            let field_name = normalize_field_name(&inferred.name);

            // Warn if field name matches a reserved core property
            if RESERVED_CORE_PROPERTIES.contains(&field_name.as_str()) {
                inferred.warnings.push(format!(
                    "Field name '{}' matches a reserved core property. Using 'custom:{}' prefix to avoid conflicts.",
                    field_name, field_name
                ));
            }

            // Apply custom: namespace prefix to all user fields
            let namespaced_name = format!("custom:{}", field_name);

            SchemaField {
                name: namespaced_name,
                field_type: inferred.field_type.clone(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: inferred.enum_values.clone(),
                indexed: false, // Not indexed by default
                required: Some(inferred.required),
                extensible: Some(inferred.field_type == "enum"), // Enums are extensible by default
                default: None,
                description: None,
                item_type: if inferred.field_type == "array" {
                    Some("string".to_string())
                } else {
                    None
                },
                fields: None,
                item_fields: None,
            }
        })
        .collect()
}

/// Normalize schema ID to snake_case
fn normalize_schema_id(entity_name: &str) -> String {
    normalize_field_name(entity_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_field_name_with_parens() {
        let desc = "invoice number (required)";
        let name = extract_field_name(desc);
        assert_eq!(name, Some("invoice_number".to_string()));
    }

    #[test]
    fn test_extract_field_name_with_keyword() {
        let desc = "amount in USD";
        let name = extract_field_name(desc);
        assert_eq!(name, Some("amount".to_string()));
    }

    #[test]
    fn test_infer_number_type() {
        assert_eq!(infer_field_type("amount in USD", "amount"), "number");
        assert_eq!(infer_field_type("price", "price"), "number");
        assert_eq!(infer_field_type("total cost", "cost"), "number");
    }

    #[test]
    fn test_infer_date_type() {
        assert_eq!(infer_field_type("due date", "due_date"), "date");
        assert_eq!(infer_field_type("when is it due", "due"), "date");
        assert_eq!(infer_field_type("deadline", "deadline"), "date");
    }

    #[test]
    fn test_infer_enum_type() {
        assert_eq!(
            infer_field_type("status (draft/sent/paid)", "status"),
            "enum"
        );
    }

    #[test]
    fn test_infer_boolean_type() {
        // "yes/no" keywords match boolean type first (priority 2) before enum pattern (priority 4)
        assert_eq!(infer_field_type("enabled (yes/no)", "enabled"), "boolean");
        assert_eq!(infer_field_type("active or inactive", "active"), "boolean");
    }

    #[test]
    fn test_extract_enum_values() {
        let values = extract_enum_values("status (draft/sent/paid)");
        assert_eq!(
            values,
            Some(vec![
                "DRAFT".to_string(),
                "SENT".to_string(),
                "PAID".to_string()
            ])
        );
    }

    #[test]
    fn test_is_field_required() {
        assert!(is_field_required("invoice number (required)"));
        assert!(is_field_required("must have email"));
        assert!(!is_field_required("optional notes"));
    }

    #[test]
    fn test_normalize_field_name() {
        assert_eq!(normalize_field_name("Invoice Number"), "invoice_number");
        assert_eq!(normalize_field_name("first-name"), "first_name");
        assert_eq!(normalize_field_name("email address"), "email_address");
    }

    #[test]
    fn test_parse_field_descriptions_simple() {
        let desc = "invoice number, amount, status";
        let fields = parse_field_descriptions(desc);

        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "invoice_number");
        assert_eq!(fields[1].name, "amount");
        assert_eq!(fields[2].name, "status");
    }

    #[test]
    fn test_parse_field_descriptions_with_types() {
        let desc = "invoice number (required), amount in USD, status (draft/sent/paid), due date";
        let fields = parse_field_descriptions(desc);

        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0].field_type, "number"); // "number" keyword detected in "invoice number"
        assert_eq!(fields[1].field_type, "number"); // amount in USD
        assert_eq!(fields[2].field_type, "enum"); // "(options/separated/by/slashes)" pattern detected
        assert_eq!(fields[3].field_type, "date"); // due date
    }

    #[test]
    fn test_normalize_and_namespace() {
        let inferred = vec![
            InferredField {
                name: "invoice_number".to_string(),
                field_type: "string".to_string(),
                required: true,
                enum_values: None,
                warnings: vec![],
            },
            InferredField {
                name: "status".to_string(),
                field_type: "enum".to_string(),
                required: false,
                enum_values: Some(vec!["DRAFT".to_string(), "SENT".to_string()]),
                warnings: vec![],
            },
        ];

        let fields = normalize_and_namespace_fields(inferred);

        assert_eq!(fields[0].name, "custom:invoice_number");
        // Note: status should trigger warning about reserved core property
        assert_eq!(fields[1].name, "custom:status");
        assert!(fields[0].required.unwrap());
        assert!(fields[1].extensible.unwrap()); // enum is extensible
    }

    #[test]
    fn test_normalize_schema_id() {
        assert_eq!(normalize_schema_id("Invoice"), "invoice");
        assert_eq!(normalize_schema_id("Customer Profile"), "customer_profile");
    }

    #[test]
    fn test_apply_constraints_required() {
        let fields = vec![InferredField {
            name: "email".to_string(),
            field_type: "string".to_string(),
            required: false,
            enum_values: None,
            warnings: vec![],
        }];

        let constraints = Some(AdditionalConstraints {
            required_fields: Some(vec!["email".to_string()]),
            default_values: None,
            enum_values: None,
        });

        let result = apply_constraints(fields, constraints);
        assert!(result[0].required);
    }

    #[test]
    fn test_apply_constraints_enum_values() {
        let fields = vec![InferredField {
            name: "status".to_string(),
            field_type: "string".to_string(),
            required: false,
            enum_values: None,
            warnings: vec![],
        }];

        let mut enum_map = std::collections::HashMap::new();
        enum_map.insert(
            "status".to_string(),
            vec!["active".to_string(), "inactive".to_string()],
        );

        let constraints = Some(AdditionalConstraints {
            required_fields: None,
            default_values: None,
            enum_values: Some(enum_map),
        });

        let result = apply_constraints(fields, constraints);
        assert_eq!(result[0].field_type, "enum");
        assert_eq!(
            result[0].enum_values,
            Some(vec!["ACTIVE".to_string(), "INACTIVE".to_string()])
        );
    }

    #[test]
    fn test_split_field_descriptions_comma() {
        let desc = "field1, field2, field3";
        let parts = split_field_descriptions(desc);
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn test_split_field_descriptions_and() {
        let desc = "field1 and field2 and field3";
        let parts = split_field_descriptions(desc);
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn test_split_field_descriptions_mixed() {
        let desc = "field1, field2 and field3; field4";
        let parts = split_field_descriptions(desc);
        assert_eq!(parts.len(), 4);
    }

    #[test]
    fn test_normalize_empty_field_name() {
        // Edge case: field name becomes empty after normalization
        let desc = "!@#$%^&*()";
        let normalized = normalize_field_name(desc);
        // Should return empty string for invalid input
        assert_eq!(normalized, "");
    }

    #[test]
    fn test_integration_full_schema_creation() {
        let desc = "invoice number (required), amount in USD, status (draft/sent/paid), due date";
        let fields = parse_field_descriptions(desc);
        let namespaced = normalize_and_namespace_fields(fields);

        // Verify all fields have custom: prefix
        assert!(namespaced.iter().all(|f| f.name.starts_with("custom:")));

        // Verify field names are present
        assert_eq!(namespaced.len(), 4);
        assert!(namespaced[0].name.contains("invoice"));
        assert!(namespaced[1].name.contains("amount"));
        assert!(namespaced[2].name.contains("status"));
        assert!(namespaced[3].name.contains("due"));

        // Verify types are inferred correctly (following priority order)
        assert_eq!(namespaced[0].field_type, "number"); // "number" keyword in "invoice number"
        assert_eq!(namespaced[1].field_type, "number"); // amount in USD
        assert_eq!(namespaced[2].field_type, "enum"); // "(options/separated/by/slashes)" pattern
        assert_eq!(namespaced[3].field_type, "date"); // due date
    }

    #[test]
    fn test_integration_ambiguous_description() {
        let desc = "some field";
        let fields = parse_field_descriptions(desc);
        let namespaced = normalize_and_namespace_fields(fields);

        // Even with ambiguous description, should still create valid schema
        assert_eq!(namespaced.len(), 1);
        assert_eq!(namespaced[0].field_type, "string"); // Defaults to string
        assert_eq!(namespaced[0].name, "custom:some_field");
    }

    #[test]
    fn test_integration_edge_case_empty_enum_values() {
        let desc = "status ()";
        let fields = parse_field_descriptions(desc);

        // Should not create enum if no values found
        assert_eq!(fields[0].enum_values, None);
        assert_eq!(fields[0].field_type, "string");
    }

    #[test]
    fn test_integration_multiple_inferred_fields() {
        let desc = "customer name, total amount in USD, invoice date, is_paid (yes/no)";
        let fields = parse_field_descriptions(desc);

        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0].field_type, "string");
        assert_eq!(fields[1].field_type, "number");
        assert_eq!(fields[2].field_type, "date");
        assert_eq!(fields[3].field_type, "boolean");
    }

    #[test]
    fn test_integration_schema_id_generation() {
        let entity_name = "Customer Invoice";
        let schema_id = normalize_schema_id(entity_name);
        assert_eq!(schema_id, "customer_invoice");
    }

    #[test]
    fn test_integration_reserved_property_names() {
        // Test that fields matching common core properties still work
        // (they just get the custom: prefix)
        let desc = "status, priority, due_date";
        let fields = parse_field_descriptions(desc);
        let namespaced = normalize_and_namespace_fields(fields);

        // All should be prefixed with custom: to avoid conflicts
        assert!(namespaced.iter().all(|f| f.name.starts_with("custom:")));
        assert_eq!(namespaced[0].name, "custom:status");
        assert_eq!(namespaced[1].name, "custom:priority");
        assert_eq!(namespaced[2].name, "custom:due_date");
    }
}
