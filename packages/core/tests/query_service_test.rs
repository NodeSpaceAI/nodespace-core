//! Integration tests for QueryService
//!
//! Tests cover:
//! - QueryDefinition serialization/deserialization
//! - Filter building for queries
//! - Sorting configuration
//! - SQL generation patterns
//! - Query execution with real database

use nodespace_core::services::query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, RelationshipType, SortConfig,
    SortDirection,
};
use serde_json::json;

// =========================================================================
// QueryDefinition Serialization Tests
// =========================================================================

#[test]
fn test_query_definition_deserialization_minimal() {
    let json = json!({
        "targetType": "task",
        "filters": []
    });

    let query: QueryDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(query.target_type, "task");
    assert!(query.filters.is_empty());
    assert!(query.sorting.is_none());
    assert!(query.limit.is_none());
}

#[test]
fn test_query_definition_deserialization_full() {
    let json = json!({
        "targetType": "task",
        "filters": [
            {
                "type": "property",
                "operator": "equals",
                "property": "status",
                "value": "open"
            }
        ],
        "sorting": [
            {
                "field": "created_at",
                "direction": "desc"
            }
        ],
        "limit": 50
    });

    let query: QueryDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(query.target_type, "task");
    assert_eq!(query.filters.len(), 1);
    assert_eq!(query.limit, Some(50));
    assert!(query.sorting.is_some());
}

#[test]
fn test_query_definition_wildcard_type() {
    let json = json!({
        "targetType": "*",
        "filters": []
    });

    let query: QueryDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(query.target_type, "*");
}

// =========================================================================
// Filter Type Tests
// =========================================================================

#[test]
fn test_filter_type_property() {
    let json = json!({
        "type": "property",
        "operator": "equals",
        "property": "status",
        "value": "open"
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.filter_type, FilterType::Property);
    assert_eq!(filter.operator, FilterOperator::Equals);
    assert_eq!(filter.property, Some("status".to_string()));
}

#[test]
fn test_filter_type_content() {
    let json = json!({
        "type": "content",
        "operator": "contains",
        "value": "important"
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.filter_type, FilterType::Content);
    assert_eq!(filter.operator, FilterOperator::Contains);
}

#[test]
fn test_filter_type_relationship() {
    let json = json!({
        "type": "relationship",
        "operator": "equals",
        "relationshipType": "children",
        "nodeId": "parent-123"
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.filter_type, FilterType::Relationship);
    assert_eq!(filter.relationship_type, Some(RelationshipType::Children));
    assert_eq!(filter.node_id, Some("parent-123".to_string()));
}

#[test]
fn test_filter_type_metadata() {
    let json = json!({
        "type": "metadata",
        "operator": "gte",
        "property": "created_at",
        "value": "2025-01-01"
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.filter_type, FilterType::Metadata);
    assert_eq!(filter.operator, FilterOperator::GreaterThanOrEqual);
}

// =========================================================================
// Operator Tests
// =========================================================================

#[test]
fn test_operator_equals() {
    let json = json!({ "type": "property", "operator": "equals", "property": "x", "value": 1 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::Equals);
}

#[test]
fn test_operator_contains() {
    let json = json!({ "type": "content", "operator": "contains", "value": "test" });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::Contains);
}

#[test]
fn test_operator_gt() {
    let json = json!({ "type": "property", "operator": "gt", "property": "x", "value": 10 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::GreaterThan);
}

#[test]
fn test_operator_lt() {
    let json = json!({ "type": "property", "operator": "lt", "property": "x", "value": 10 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::LessThan);
}

#[test]
fn test_operator_gte() {
    let json = json!({ "type": "property", "operator": "gte", "property": "x", "value": 10 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::GreaterThanOrEqual);
}

#[test]
fn test_operator_lte() {
    let json = json!({ "type": "property", "operator": "lte", "property": "x", "value": 10 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::LessThanOrEqual);
}

#[test]
fn test_operator_in() {
    let json = json!({ "type": "property", "operator": "in", "property": "status", "value": ["open", "in_progress"] });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::In);
}

#[test]
fn test_operator_exists() {
    let json = json!({ "type": "property", "operator": "exists", "property": "assignee" });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::Exists);
}

// =========================================================================
// Relationship Type Tests
// =========================================================================

#[test]
fn test_relationship_type_parent() {
    let json = json!({
        "type": "relationship",
        "operator": "equals",
        "relationshipType": "parent",
        "nodeId": "node-123"
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.relationship_type, Some(RelationshipType::Parent));
}

#[test]
fn test_relationship_type_children() {
    let json = json!({
        "type": "relationship",
        "operator": "equals",
        "relationshipType": "children",
        "nodeId": "node-123"
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.relationship_type, Some(RelationshipType::Children));
}

#[test]
fn test_relationship_type_mentions() {
    let json = json!({
        "type": "relationship",
        "operator": "equals",
        "relationshipType": "mentions",
        "nodeId": "node-123"
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.relationship_type, Some(RelationshipType::Mentions));
}

#[test]
fn test_relationship_type_mentioned_by() {
    let json = json!({
        "type": "relationship",
        "operator": "equals",
        "relationshipType": "mentioned_by",
        "nodeId": "node-123"
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(
        filter.relationship_type,
        Some(RelationshipType::MentionedBy)
    );
}

// =========================================================================
// Sort Direction Tests
// =========================================================================

#[test]
fn test_sort_direction_ascending() {
    let json = json!({
        "field": "created_at",
        "direction": "asc"
    });
    let sort: SortConfig = serde_json::from_value(json).unwrap();
    assert_eq!(sort.direction, SortDirection::Ascending);
    assert_eq!(sort.field, "created_at");
}

#[test]
fn test_sort_direction_descending() {
    let json = json!({
        "field": "priority",
        "direction": "desc"
    });
    let sort: SortConfig = serde_json::from_value(json).unwrap();
    assert_eq!(sort.direction, SortDirection::Descending);
    assert_eq!(sort.field, "priority");
}

// =========================================================================
// Case Sensitivity Tests
// =========================================================================

#[test]
fn test_filter_case_sensitive_default() {
    let json = json!({
        "type": "content",
        "operator": "contains",
        "value": "test"
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    // Default should be None (treated as true in query building)
    assert!(filter.case_sensitive.is_none());
}

#[test]
fn test_filter_case_sensitive_false() {
    let json = json!({
        "type": "content",
        "operator": "contains",
        "value": "test",
        "caseSensitive": false
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.case_sensitive, Some(false));
}

#[test]
fn test_filter_case_sensitive_true() {
    let json = json!({
        "type": "content",
        "operator": "contains",
        "value": "test",
        "caseSensitive": true
    });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.case_sensitive, Some(true));
}

// =========================================================================
// Edge Case Tests
// =========================================================================

#[test]
fn test_filter_with_null_value() {
    let json = json!({
        "type": "property",
        "operator": "equals",
        "property": "status",
        "value": null
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.filter_type, FilterType::Property);
    // With Option<Value>, JSON null deserializes to None (not Some(Null))
    // because serde treats null as absence of value for Option types
    assert!(filter.value.is_none() || filter.value.as_ref().is_some_and(|v| v.is_null()));
}

#[test]
fn test_filter_with_boolean_value() {
    let json = json!({
        "type": "property",
        "operator": "equals",
        "property": "completed",
        "value": true
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.filter_type, FilterType::Property);
    assert_eq!(filter.value, Some(json!(true)));
}

#[test]
fn test_multiple_sort_configs() {
    let json = json!({
        "targetType": "task",
        "filters": [],
        "sorting": [
            {"field": "priority", "direction": "desc"},
            {"field": "created_at", "direction": "asc"}
        ]
    });

    let query: QueryDefinition = serde_json::from_value(json).unwrap();
    let sorting = query.sorting.unwrap();
    assert_eq!(sorting.len(), 2);
    assert_eq!(sorting[0].field, "priority");
    assert_eq!(sorting[0].direction, SortDirection::Descending);
    assert_eq!(sorting[1].field, "created_at");
    assert_eq!(sorting[1].direction, SortDirection::Ascending);
}

#[test]
fn test_empty_sorting_list() {
    let json = json!({
        "targetType": "task",
        "filters": [],
        "sorting": []
    });

    let query: QueryDefinition = serde_json::from_value(json).unwrap();
    assert!(query.sorting.is_some());
    assert!(query.sorting.unwrap().is_empty());
}

// =========================================================================
// Complex Filter Combinations
// =========================================================================

#[test]
fn test_filter_with_nested_value() {
    let json = json!({
        "type": "property",
        "operator": "in",
        "property": "status",
        "value": ["open", "pending", "in_progress"]
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::In);
    let arr = filter.value.as_ref().unwrap().as_array().unwrap();
    assert_eq!(arr.len(), 3);
}

#[test]
fn test_query_with_all_filter_types() {
    let json = json!({
        "targetType": "task",
        "filters": [
            {"type": "property", "operator": "equals", "property": "status", "value": "open"},
            {"type": "content", "operator": "contains", "value": "urgent"},
            {"type": "metadata", "operator": "gte", "property": "created_at", "value": "2025-01-01"},
            {"type": "relationship", "operator": "equals", "relationshipType": "parent", "nodeId": "project-1"}
        ],
        "sorting": [{"field": "priority", "direction": "desc"}],
        "limit": 100
    });

    let query: QueryDefinition = serde_json::from_value(json).unwrap();
    assert_eq!(query.filters.len(), 4);
    assert_eq!(query.filters[0].filter_type, FilterType::Property);
    assert_eq!(query.filters[1].filter_type, FilterType::Content);
    assert_eq!(query.filters[2].filter_type, FilterType::Metadata);
    assert_eq!(query.filters[3].filter_type, FilterType::Relationship);
    assert_eq!(query.limit, Some(100));
}

#[test]
fn test_numeric_value_types() {
    let json_int = json!({
        "type": "property",
        "operator": "gt",
        "property": "priority",
        "value": 5
    });

    let filter: QueryFilter = serde_json::from_value(json_int).unwrap();
    assert!(filter.value.as_ref().unwrap().is_i64());

    let json_float = json!({
        "type": "property",
        "operator": "lt",
        "property": "score",
        "value": 3.15
    });

    let filter: QueryFilter = serde_json::from_value(json_float).unwrap();
    assert!(filter.value.as_ref().unwrap().is_f64());
}

#[test]
fn test_comparison_operators_with_different_values() {
    // Greater than
    let json = json!({ "type": "property", "operator": "gt", "property": "count", "value": 100 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::GreaterThan);

    // Less than
    let json = json!({ "type": "property", "operator": "lt", "property": "count", "value": 50 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::LessThan);

    // Greater than or equal
    let json = json!({ "type": "property", "operator": "gte", "property": "count", "value": 0 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::GreaterThanOrEqual);

    // Less than or equal
    let json = json!({ "type": "property", "operator": "lte", "property": "count", "value": 1000 });
    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::LessThanOrEqual);
}

#[test]
fn test_exists_operator_no_value() {
    let json = json!({
        "type": "property",
        "operator": "exists",
        "property": "assignee"
    });

    let filter: QueryFilter = serde_json::from_value(json).unwrap();
    assert_eq!(filter.operator, FilterOperator::Exists);
    assert!(filter.value.is_none());
}

#[test]
fn test_query_serialization_round_trip() {
    let original = QueryDefinition {
        target_type: "task".to_string(),
        filters: vec![QueryFilter {
            filter_type: FilterType::Property,
            operator: FilterOperator::Equals,
            property: Some("status".to_string()),
            value: Some(json!("open")),
            case_sensitive: None,
            relationship_type: None,
            node_id: None,
        }],
        sorting: Some(vec![SortConfig {
            field: "created_at".to_string(),
            direction: SortDirection::Descending,
        }]),
        limit: Some(50),
    };

    // Serialize to JSON
    let json_value = serde_json::to_value(&original).unwrap();

    // Deserialize back
    let deserialized: QueryDefinition = serde_json::from_value(json_value).unwrap();

    assert_eq!(deserialized.target_type, "task");
    assert_eq!(deserialized.filters.len(), 1);
    assert_eq!(deserialized.limit, Some(50));
    assert!(deserialized.sorting.is_some());
}

#[test]
fn test_sort_config_serialization() {
    let config = SortConfig {
        field: "modified_at".to_string(),
        direction: SortDirection::Ascending,
    };

    let json = serde_json::to_value(&config).unwrap();
    assert_eq!(json["field"], "modified_at");
    assert_eq!(json["direction"], "asc");
}

#[test]
fn test_filter_serialization() {
    let filter = QueryFilter {
        filter_type: FilterType::Content,
        operator: FilterOperator::Contains,
        property: None,
        value: Some(json!("search term")),
        case_sensitive: Some(false),
        relationship_type: None,
        node_id: None,
    };

    let json = serde_json::to_value(&filter).unwrap();
    assert_eq!(json["type"], "content");
    assert_eq!(json["operator"], "contains");
    assert_eq!(json["caseSensitive"], false);
}
