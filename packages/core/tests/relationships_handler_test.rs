//! Integration tests for MCP Relationship Handlers
//!
//! Tests cover:
//! - Parameter deserialization for all handlers
//! - Output serialization verification
//! - Wire format (camelCase) compliance
//! - Error handling patterns

use nodespace_core::mcp::handlers::relationships::{
    CheckNodeCompletenessParams, CreateRelationshipOutput, CreateRelationshipParams,
    DeleteRelationshipParams, GetInboundRelationshipsParams, GetRelatedNodesOutput,
    GetRelatedNodesParams, InboundRelationshipInfo, InboundRelationshipsOutput, RelationshipEdge,
    RelationshipGraphOutput,
};
use nodespace_core::services::node_service::CompletenessResult;
use serde_json::json;

// =========================================================================
// CreateRelationshipParams Tests
// =========================================================================

#[test]
fn test_create_relationship_params_deserialization() {
    let json = json!({
        "source_id": "invoice-001",
        "relationship_name": "billed_to",
        "target_id": "customer-acme"
    });

    let params: CreateRelationshipParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.source_id, "invoice-001");
    assert_eq!(params.relationship_name, "billed_to");
    assert_eq!(params.target_id, "customer-acme");
    assert!(params.edge_data.is_none());
}

#[test]
fn test_create_relationship_params_with_edge_data() {
    let json = json!({
        "source_id": "invoice-001",
        "relationship_name": "billed_to",
        "target_id": "customer-acme",
        "edge_data": {
            "billing_date": "2025-01-15",
            "amount": 1500
        }
    });

    let params: CreateRelationshipParams = serde_json::from_value(json).unwrap();
    assert!(params.edge_data.is_some());

    let edge_data = params.edge_data.unwrap();
    assert_eq!(edge_data["billing_date"], "2025-01-15");
    assert_eq!(edge_data["amount"], 1500);
}

#[test]
fn test_create_relationship_params_empty_edge_data() {
    let json = json!({
        "source_id": "node-a",
        "relationship_name": "links_to",
        "target_id": "node-b",
        "edge_data": {}
    });

    let params: CreateRelationshipParams = serde_json::from_value(json).unwrap();
    assert!(params.edge_data.is_some());
    assert!(params.edge_data.unwrap().as_object().unwrap().is_empty());
}

#[test]
fn test_create_relationship_params_missing_required_fields() {
    // Missing source_id
    let json = json!({
        "relationship_name": "billed_to",
        "target_id": "customer-acme"
    });
    let result: Result<CreateRelationshipParams, _> = serde_json::from_value(json);
    assert!(result.is_err());

    // Missing relationship_name
    let json = json!({
        "source_id": "invoice-001",
        "target_id": "customer-acme"
    });
    let result: Result<CreateRelationshipParams, _> = serde_json::from_value(json);
    assert!(result.is_err());

    // Missing target_id
    let json = json!({
        "source_id": "invoice-001",
        "relationship_name": "billed_to"
    });
    let result: Result<CreateRelationshipParams, _> = serde_json::from_value(json);
    assert!(result.is_err());
}

// =========================================================================
// DeleteRelationshipParams Tests
// =========================================================================

#[test]
fn test_delete_relationship_params_deserialization() {
    let json = json!({
        "source_id": "invoice-001",
        "relationship_name": "billed_to",
        "target_id": "customer-acme"
    });

    let params: DeleteRelationshipParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.source_id, "invoice-001");
    assert_eq!(params.relationship_name, "billed_to");
    assert_eq!(params.target_id, "customer-acme");
}

#[test]
fn test_delete_relationship_params_missing_fields() {
    let json = json!({
        "source_id": "invoice-001"
    });

    let result: Result<DeleteRelationshipParams, _> = serde_json::from_value(json);
    assert!(result.is_err());
}

// =========================================================================
// GetRelatedNodesParams Tests
// =========================================================================

#[test]
fn test_get_related_nodes_params_default_direction() {
    let json = json!({
        "node_id": "invoice-001",
        "relationship_name": "billed_to"
    });

    let params: GetRelatedNodesParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.node_id, "invoice-001");
    assert_eq!(params.relationship_name, "billed_to");
    assert_eq!(params.direction, "out"); // Default
}

#[test]
fn test_get_related_nodes_params_explicit_direction() {
    let json = json!({
        "node_id": "customer-001",
        "relationship_name": "invoices",
        "direction": "in"
    });

    let params: GetRelatedNodesParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.direction, "in");
}

#[test]
fn test_get_related_nodes_params_out_direction() {
    let json = json!({
        "node_id": "invoice-001",
        "relationship_name": "billed_to",
        "direction": "out"
    });

    let params: GetRelatedNodesParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.direction, "out");
}

// =========================================================================
// GetInboundRelationshipsParams Tests
// =========================================================================

#[test]
fn test_get_inbound_relationships_params_deserialization() {
    let json = json!({
        "target_type": "customer"
    });

    let params: GetInboundRelationshipsParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.target_type, "customer");
}

#[test]
fn test_get_inbound_relationships_params_missing_target_type() {
    let json = json!({});

    let result: Result<GetInboundRelationshipsParams, _> = serde_json::from_value(json);
    assert!(result.is_err());
}

// =========================================================================
// Output Serialization Tests (Wire Format)
// =========================================================================

#[test]
fn test_create_relationship_output_camel_case() {
    let output = CreateRelationshipOutput {
        success: true,
        source_id: "invoice-001".to_string(),
        relationship_name: "billed_to".to_string(),
        target_id: "customer-acme".to_string(),
    };

    let json = serde_json::to_value(&output).unwrap();

    // Verify camelCase serialization
    assert_eq!(json["success"], true);
    assert_eq!(json["sourceId"], "invoice-001");
    assert_eq!(json["relationshipName"], "billed_to");
    assert_eq!(json["targetId"], "customer-acme");

    // Verify snake_case NOT used
    assert!(json.get("source_id").is_none());
    assert!(json.get("relationship_name").is_none());
    assert!(json.get("target_id").is_none());
}

#[test]
fn test_get_related_nodes_output_camel_case() {
    let output = GetRelatedNodesOutput {
        node_id: "invoice-001".to_string(),
        relationship_name: "billed_to".to_string(),
        direction: "out".to_string(),
        related_nodes: vec![json!({"id": "customer-001"})],
        count: 1,
    };

    let json = serde_json::to_value(&output).unwrap();

    // Verify camelCase serialization
    assert_eq!(json["nodeId"], "invoice-001");
    assert_eq!(json["relationshipName"], "billed_to");
    assert_eq!(json["direction"], "out");
    assert_eq!(json["relatedNodes"].as_array().unwrap().len(), 1);
    assert_eq!(json["count"], 1);

    // Verify snake_case NOT used
    assert!(json.get("node_id").is_none());
    assert!(json.get("related_nodes").is_none());
}

#[test]
fn test_relationship_edge_camel_case() {
    let edge = RelationshipEdge {
        source_type: "invoice".to_string(),
        relationship_name: "billed_to".to_string(),
        target_type: Some("customer".to_string()),
    };

    let json = serde_json::to_value(&edge).unwrap();

    // Verify camelCase
    assert_eq!(json["sourceType"], "invoice");
    assert_eq!(json["relationshipName"], "billed_to");
    assert_eq!(json["targetType"], "customer");
}

#[test]
fn test_relationship_graph_output_camel_case() {
    let output = RelationshipGraphOutput {
        edges: vec![RelationshipEdge {
            source_type: "invoice".to_string(),
            relationship_name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
        }],
        total_edges: 1,
    };

    let json = serde_json::to_value(&output).unwrap();

    assert_eq!(json["totalEdges"], 1);
    assert!(json.get("edges").is_some());
}

#[test]
fn test_inbound_relationship_info_camel_case() {
    let info = InboundRelationshipInfo {
        source_type: "invoice".to_string(),
        relationship_name: "billed_to".to_string(),
        reverse_name: Some("invoices".to_string()),
        cardinality: "one".to_string(),
        edge_table: "invoice_billed_to_customer".to_string(),
    };

    let json = serde_json::to_value(&info).unwrap();

    assert_eq!(json["sourceType"], "invoice");
    assert_eq!(json["relationshipName"], "billed_to");
    assert_eq!(json["reverseName"], "invoices");
    assert_eq!(json["cardinality"], "one");
    assert_eq!(json["edgeTable"], "invoice_billed_to_customer");
}

#[test]
fn test_inbound_relationships_output_camel_case() {
    let output = InboundRelationshipsOutput {
        target_type: "customer".to_string(),
        inbound_relationships: vec![InboundRelationshipInfo {
            source_type: "invoice".to_string(),
            relationship_name: "billed_to".to_string(),
            reverse_name: Some("invoices".to_string()),
            cardinality: "one".to_string(),
            edge_table: "invoice_billed_to_customer".to_string(),
        }],
        count: 1,
    };

    let json = serde_json::to_value(&output).unwrap();

    assert_eq!(json["targetType"], "customer");
    assert_eq!(json["inboundRelationships"].as_array().unwrap().len(), 1);
    assert_eq!(json["count"], 1);
}

// =========================================================================
// Serialization Verification Tests
// =========================================================================

#[test]
fn test_create_relationship_params_parse_and_verify() {
    // Parse from JSON (as it would come from wire)
    let json = json!({
        "source_id": "src-001",
        "relationship_name": "rel",
        "target_id": "tgt-001",
        "edge_data": {"key": "value"}
    });

    let parsed: CreateRelationshipParams = serde_json::from_value(json).unwrap();

    assert_eq!(parsed.source_id, "src-001");
    assert_eq!(parsed.relationship_name, "rel");
    assert_eq!(parsed.target_id, "tgt-001");
    assert!(parsed.edge_data.is_some());
}

#[test]
fn test_get_related_nodes_output_serialization() {
    let output = GetRelatedNodesOutput {
        node_id: "node-001".to_string(),
        relationship_name: "links".to_string(),
        direction: "out".to_string(),
        related_nodes: vec![json!({"id": "node-002"}), json!({"id": "node-003"})],
        count: 2,
    };

    let json = serde_json::to_value(&output).unwrap();

    // Verify serialization produces expected structure
    assert_eq!(json["nodeId"], "node-001");
    assert_eq!(json["relationshipName"], "links");
    assert_eq!(json["direction"], "out");
    assert_eq!(json["count"], 2);
    assert_eq!(json["relatedNodes"].as_array().unwrap().len(), 2);
}

// =========================================================================
// Edge Cases
// =========================================================================

#[test]
fn test_params_with_unicode() {
    let json = json!({
        "source_id": "节点-001",
        "relationship_name": "связь",
        "target_id": "نود-002"
    });

    let params: CreateRelationshipParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.source_id, "节点-001");
    assert_eq!(params.relationship_name, "связь");
    assert_eq!(params.target_id, "نود-002");
}

#[test]
fn test_params_with_special_characters() {
    let json = json!({
        "source_id": "node:001/path",
        "relationship_name": "rel-with-dashes_and_underscores",
        "target_id": "target.with.dots"
    });

    let params: CreateRelationshipParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.source_id, "node:001/path");
    assert_eq!(params.relationship_name, "rel-with-dashes_and_underscores");
    assert_eq!(params.target_id, "target.with.dots");
}

#[test]
fn test_output_with_empty_related_nodes() {
    let output = GetRelatedNodesOutput {
        node_id: "node-001".to_string(),
        relationship_name: "links".to_string(),
        direction: "out".to_string(),
        related_nodes: vec![],
        count: 0,
    };

    let json = serde_json::to_value(&output).unwrap();
    assert!(json["relatedNodes"].as_array().unwrap().is_empty());
    assert_eq!(json["count"], 0);
}

#[test]
fn test_inbound_relationship_info_without_reverse_name() {
    let info = InboundRelationshipInfo {
        source_type: "doc".to_string(),
        relationship_name: "references".to_string(),
        reverse_name: None,
        cardinality: "many".to_string(),
        edge_table: "doc_references_doc".to_string(),
    };

    let json = serde_json::to_value(&info).unwrap();
    assert!(json["reverseName"].is_null());
}

// =========================================================================
// CheckNodeCompletenessParams Tests
// =========================================================================

#[test]
fn test_check_node_completeness_params_deserialization() {
    let json = json!({ "node_id": "invoice-001" });

    let params: CheckNodeCompletenessParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.node_id, "invoice-001");
}

// =========================================================================
// CompletenessResult Serialization Tests
// =========================================================================

#[test]
fn test_completeness_result_complete_camel_case() {
    let result = CompletenessResult {
        node_id: "invoice-001".to_string(),
        is_complete: true,
        missing_relationships: vec![],
    };

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["nodeId"], "invoice-001");
    assert_eq!(json["isComplete"], true);
    assert!(json["missingRelationships"].as_array().unwrap().is_empty());
}

#[test]
fn test_completeness_result_incomplete() {
    let result = CompletenessResult {
        node_id: "invoice-001".to_string(),
        is_complete: false,
        missing_relationships: vec!["billed_to".to_string()],
    };

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["isComplete"], false);
    let missing = json["missingRelationships"].as_array().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], "billed_to");
}

#[test]
fn test_completeness_result_multiple_missing() {
    let result = CompletenessResult {
        node_id: "node-xyz".to_string(),
        is_complete: false,
        missing_relationships: vec!["billed_to".to_string(), "assigned_to".to_string()],
    };

    let json = serde_json::to_value(&result).unwrap();
    let missing = json["missingRelationships"].as_array().unwrap();
    assert_eq!(missing.len(), 2);
}

#[test]
fn test_completeness_result_json_round_trip() {
    let result = CompletenessResult {
        node_id: "task-001".to_string(),
        is_complete: false,
        missing_relationships: vec!["assigned_to".to_string()],
    };

    let json = serde_json::to_string(&result).unwrap();
    let parsed: CompletenessResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.node_id, result.node_id);
    assert_eq!(parsed.is_complete, result.is_complete);
    assert_eq!(parsed.missing_relationships, result.missing_relationships);
}
