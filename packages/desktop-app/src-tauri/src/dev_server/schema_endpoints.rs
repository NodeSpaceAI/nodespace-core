//! Schema Management Endpoints for HTTP Dev Server
//!
//! This module implements HTTP endpoints for schema management operations,
//! enabling browser-based development and testing of schema features.
//! These endpoints call the SchemaService from nodespace-core.
//!
//! # Endpoints
//!
//! - `GET /api/schemas/:schema_id` - Get schema definition
//! - `POST /api/schemas/:schema_id/fields` - Add field to schema
//! - `DELETE /api/schemas/:schema_id/fields/:field_name` - Remove field from schema
//! - `POST /api/schemas/:schema_id/fields/:field_name/enum-values` - Extend enum field
//! - `DELETE /api/schemas/:schema_id/fields/:field_name/enum-values/:value` - Remove enum value

use axum::{
    extract::{Path, State},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use nodespace_core::models::schema::{ProtectionLevel, SchemaField};
use nodespace_core::services::SchemaService;
use serde::{Deserialize, Serialize};

use crate::dev_server::{AppState, HttpError};

// ===== Type Definitions =====

/// Response for get_schema endpoint
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetSchemaResponse {
    pub is_core: bool,
    pub version: i32,
    pub description: String,
    pub fields: Vec<SchemaFieldResponse>,
}

/// Schema field in response
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFieldResponse {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub protection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_values: Option<Vec<String>>,
    pub indexed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
}

/// Convert SchemaField from nodespace-core to HTTP response format
impl From<SchemaField> for SchemaFieldResponse {
    fn from(f: SchemaField) -> Self {
        Self {
            name: f.name,
            field_type: f.field_type,
            protection: format!("{:?}", f.protection),
            core_values: f.core_values,
            user_values: f.user_values,
            indexed: f.indexed,
            required: f.required,
            extensible: f.extensible,
            default: f.default,
            description: f.description,
            item_type: f.item_type,
        }
    }
}

/// Request body for adding a field
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddFieldRequest {
    pub field_name: String,
    pub field_type: String,
    #[serde(default)]
    pub indexed: bool,
    pub required: Option<bool>,
    pub default: Option<serde_json::Value>,
    pub description: Option<String>,
    pub item_type: Option<String>,
    pub enum_values: Option<Vec<String>>,
    pub extensible: Option<bool>,
}

/// Response for mutation operations (add/remove field, extend/remove enum)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResponse {
    pub schema_id: String,
    pub new_version: i32,
    pub success: bool,
}

/// Request body for extending enum
#[derive(Debug, Deserialize)]
pub struct ExtendEnumRequest {
    pub value: String,
}

// ===== Endpoint Handlers =====

/// GET /api/schemas/:schema_id
///
/// Get a schema definition by schema ID
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/schemas/task
/// ```
async fn get_schema(
    Path(schema_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<GetSchemaResponse>, HttpError> {
    // Get NodeService from state
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(format!("Failed to acquire read lock: {}", e), "LOCK_ERROR")
        })?;
        std::sync::Arc::clone(&*lock)
    };

    // Create SchemaService
    let schema_service = SchemaService::new(node_service);

    // Get schema
    let schema = schema_service
        .get_schema(&schema_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    // Convert to response format
    let response = GetSchemaResponse {
        is_core: schema.is_core,
        version: schema.version as i32,
        description: schema.description,
        fields: schema.fields.into_iter().map(|f| f.into()).collect(),
    };

    Ok(Json(response))
}

/// POST /api/schemas/:schema_id/fields
///
/// Add a new field to a schema
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/schemas/task/fields \
///   -H "Content-Type: application/json" \
///   -d '{"fieldName": "priority", "fieldType": "enum", "enumValues": ["low", "medium", "high"]}'
/// ```
async fn add_field(
    Path(schema_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<AddFieldRequest>,
) -> Result<Json<MutationResponse>, HttpError> {
    // Acquire write lock for mutation operation
    let _write_guard = state.write_lock.lock().await;

    // Get NodeService from state
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(format!("Failed to acquire read lock: {}", e), "LOCK_ERROR")
        })?;
        std::sync::Arc::clone(&*lock)
    };

    // Create SchemaService
    let schema_service = SchemaService::new(node_service);

    // Validate field type constraints at API boundary
    // This provides better HTTP error responses (400 Bad Request) vs backend errors (500)
    if request.field_type == "enum" && request.enum_values.is_none() {
        return Err(HttpError::new(
            "Enum fields must specify enumValues".to_string(),
            "VALIDATION_ERROR",
        ));
    }

    if request.field_type == "array" && request.item_type.is_none() {
        return Err(HttpError::new(
            "Array fields must specify itemType".to_string(),
            "VALIDATION_ERROR",
        ));
    }

    // Build SchemaField from request
    let schema_field = SchemaField {
        name: request.field_name,
        field_type: request.field_type,
        protection: ProtectionLevel::User, // Only user fields can be added
        core_values: None,                 // User fields don't have core values
        user_values: request.enum_values,
        indexed: request.indexed,
        required: request.required,
        extensible: request.extensible,
        default: request.default,
        description: request.description,
        item_type: request.item_type,
    };

    // Add field to schema
    schema_service
        .add_field(&schema_id, schema_field)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    // Get updated schema to return new version
    let updated_schema = schema_service
        .get_schema(&schema_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    Ok(Json(MutationResponse {
        schema_id,
        new_version: updated_schema.version as i32,
        success: true,
    }))
}

/// DELETE /api/schemas/:schema_id/fields/:field_name
///
/// Remove a field from a schema (user fields only)
///
/// # Example
///
/// ```bash
/// curl -X DELETE http://localhost:3001/api/schemas/task/fields/priority
/// ```
async fn remove_field(
    Path((schema_id, field_name)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<Json<MutationResponse>, HttpError> {
    // Acquire write lock for mutation operation
    let _write_guard = state.write_lock.lock().await;

    // Get NodeService from state
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(format!("Failed to acquire read lock: {}", e), "LOCK_ERROR")
        })?;
        std::sync::Arc::clone(&*lock)
    };

    // Create SchemaService
    let schema_service = SchemaService::new(node_service);

    // Remove field
    schema_service
        .remove_field(&schema_id, &field_name)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    // Get updated schema to return new version
    let updated_schema = schema_service
        .get_schema(&schema_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    Ok(Json(MutationResponse {
        schema_id,
        new_version: updated_schema.version as i32,
        success: true,
    }))
}

/// POST /api/schemas/:schema_id/fields/:field_name/enum-values
///
/// Extend an enum field with a new value
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/schemas/task/fields/status/enum-values \
///   -H "Content-Type: application/json" \
///   -d '{"value": "archived"}'
/// ```
async fn extend_enum(
    Path((schema_id, field_name)): Path<(String, String)>,
    State(state): State<AppState>,
    Json(request): Json<ExtendEnumRequest>,
) -> Result<Json<MutationResponse>, HttpError> {
    // Acquire write lock for mutation operation
    let _write_guard = state.write_lock.lock().await;

    // Get NodeService from state
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(format!("Failed to acquire read lock: {}", e), "LOCK_ERROR")
        })?;
        std::sync::Arc::clone(&*lock)
    };

    // Create SchemaService
    let schema_service = SchemaService::new(node_service);

    // Extend enum
    schema_service
        .extend_enum_field(&schema_id, &field_name, request.value)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    // Get updated schema to return new version
    let updated_schema = schema_service
        .get_schema(&schema_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    Ok(Json(MutationResponse {
        schema_id,
        new_version: updated_schema.version as i32,
        success: true,
    }))
}

/// DELETE /api/schemas/:schema_id/fields/:field_name/enum-values/:value
///
/// Remove a value from an enum field (user values only)
///
/// # Example
///
/// ```bash
/// curl -X DELETE http://localhost:3001/api/schemas/task/fields/status/enum-values/archived
/// ```
async fn remove_enum_value(
    Path((schema_id, field_name, value)): Path<(String, String, String)>,
    State(state): State<AppState>,
) -> Result<Json<MutationResponse>, HttpError> {
    // Acquire write lock for mutation operation
    let _write_guard = state.write_lock.lock().await;

    // Get NodeService from state
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(format!("Failed to acquire read lock: {}", e), "LOCK_ERROR")
        })?;
        std::sync::Arc::clone(&*lock)
    };

    // Create SchemaService
    let schema_service = SchemaService::new(node_service);

    // Remove enum value
    schema_service
        .remove_enum_value(&schema_id, &field_name, &value)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    // Get updated schema to return new version
    let updated_schema = schema_service
        .get_schema(&schema_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "SCHEMA_ERROR"))?;

    Ok(Json(MutationResponse {
        schema_id,
        new_version: updated_schema.version as i32,
        success: true,
    }))
}

// ===== Router Configuration =====

/// Create router for schema endpoints
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/schemas/:schema_id", get(get_schema))
        .route("/api/schemas/:schema_id/fields", post(add_field))
        .route(
            "/api/schemas/:schema_id/fields/:field_name",
            delete(remove_field),
        )
        .route(
            "/api/schemas/:schema_id/fields/:field_name/enum-values",
            post(extend_enum),
        )
        .route(
            "/api/schemas/:schema_id/fields/:field_name/enum-values/:value",
            delete(remove_enum_value),
        )
        .with_state(state)
}
