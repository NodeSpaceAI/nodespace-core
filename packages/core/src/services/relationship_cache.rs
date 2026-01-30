//! Inbound Relationship Cache for NLP Discovery
//!
//! Provides fast lookup of relationships pointing TO a node type without scanning all schemas.
//! This cache is used by NLP/AI to quickly discover how different node types relate to each other.
//!
//! # Architecture
//!
//! The cache maintains an index of `target_type → Vec<InboundRelationship>` which allows O(1)
//! lookup of all relationships pointing to a given node type.
//!
//! # Cache Invalidation
//!
//! The cache uses a hybrid invalidation strategy:
//! - **Time-based**: Cache is considered stale after 60 seconds
//! - **Event-driven**: Schema change flag triggers immediate refresh on next access
//!
//! # Performance
//!
//! - **Cache hit**: <1µs (HashMap lookup)
//! - **Cache refresh**: ~50-200ms (loads all schemas once)
//! - **Memory overhead**: ~10KB per 100 schemas
//!
use crate::db::SurrealStore;
use crate::models::schema::RelationshipCardinality;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Metadata about a relationship pointing TO a node type
///
/// This struct provides the information NLP needs to understand reverse relationships
/// without having to scan all schemas or mutate target schemas.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InboundRelationship {
    /// The type of node that defines this relationship (e.g., "invoice")
    pub source_type: String,

    /// The name of the relationship in the source schema (e.g., "billed_to")
    pub relationship_name: String,

    /// Optional reverse name for NLP discovery (e.g., "invoices")
    ///
    /// This is metadata provided by the source schema to help NLP understand
    /// how to describe the relationship from the target's perspective.
    pub reverse_name: Option<String>,

    /// Cardinality from the source's perspective (e.g., "one" if invoice->one customer)
    pub cardinality: RelationshipCardinality,

    /// Reverse cardinality (from target's perspective)
    ///
    /// e.g., "many" if one customer can have many invoices
    pub reverse_cardinality: Option<RelationshipCardinality>,

    /// The edge table name for querying relationships
    pub edge_table: String,

    /// Human-readable description (for NLP)
    pub description: Option<String>,
}

/// Cache for fast inbound relationship discovery
///
/// Maintains an index of relationships by target type, enabling O(1) lookup
/// of "what points to this type" without scanning all schemas.
pub struct InboundRelationshipCache {
    /// Map: target_type → Vec<InboundRelationship>
    cache: Arc<RwLock<HashMap<String, Vec<InboundRelationship>>>>,

    /// Last cache refresh time
    last_refresh: Arc<RwLock<Option<Instant>>>,

    /// Flag indicating schema has changed (set by event handlers)
    schema_change_flag: Arc<AtomicBool>,

    /// Database store for querying schemas
    store: Arc<SurrealStore>,

    /// Cache TTL (defaults to 60 seconds)
    cache_ttl: Duration,
}

impl InboundRelationshipCache {
    /// Create a new InboundRelationshipCache
    ///
    /// The cache starts empty and will be populated on first access.
    pub fn new(store: Arc<SurrealStore>) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            last_refresh: Arc::new(RwLock::new(None)),
            schema_change_flag: Arc::new(AtomicBool::new(false)),
            store,
            cache_ttl: Duration::from_secs(60),
        }
    }

    /// Create a cache with custom TTL (primarily for testing)
    pub fn with_ttl(store: Arc<SurrealStore>, ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            last_refresh: Arc::new(RwLock::new(None)),
            schema_change_flag: Arc::new(AtomicBool::new(false)),
            store,
            cache_ttl: ttl,
        }
    }

    /// Get all relationships pointing TO a specific node type
    ///
    /// Returns an empty vec if no relationships point to this type.
    /// The cache is automatically refreshed if stale.
    ///
    /// # Arguments
    ///
    /// * `target_type` - The node type to find inbound relationships for (e.g., "customer")
    ///
    /// # Returns
    ///
    /// Vector of InboundRelationship structs describing relationships that point to this type.
    ///
    pub async fn get_inbound_relationships(
        &self,
        target_type: &str,
    ) -> anyhow::Result<Vec<InboundRelationship>> {
        // Check if cache needs refresh
        if self.needs_refresh().await {
            self.refresh_cache().await?;
        }

        // Return from cache (fast!)
        let cache = self.cache.read().await;
        Ok(cache.get(target_type).cloned().unwrap_or_default())
    }

    /// Get all inbound relationships for all types
    ///
    /// Returns the complete cache index. Useful for NLP to understand
    /// the entire relationship graph.
    pub async fn get_all_inbound_relationships(
        &self,
    ) -> anyhow::Result<HashMap<String, Vec<InboundRelationship>>> {
        if self.needs_refresh().await {
            self.refresh_cache().await?;
        }

        let cache = self.cache.read().await;
        Ok(cache.clone())
    }

    /// Signal that a schema has changed, triggering refresh on next access
    ///
    /// This should be called when:
    /// - A schema node is created
    /// - A schema node is updated
    /// - A schema node is deleted
    pub fn invalidate(&self) {
        self.schema_change_flag.store(true, Ordering::Release);
    }

    /// Force immediate cache refresh
    ///
    /// Useful for testing or when you know schemas have changed and need
    /// immediate consistency.
    pub async fn force_refresh(&self) -> anyhow::Result<()> {
        self.refresh_cache().await
    }

    /// Check if cache needs refresh
    async fn needs_refresh(&self) -> bool {
        // Check event-driven flag first
        if self.schema_change_flag.load(Ordering::Acquire) {
            return true;
        }

        // Check time-based staleness
        let last = self.last_refresh.read().await;
        match *last {
            None => true, // Never refreshed
            Some(instant) => instant.elapsed() > self.cache_ttl,
        }
    }

    /// Refresh the cache by querying all schemas
    async fn refresh_cache(&self) -> anyhow::Result<()> {
        // Query all schemas
        let schemas = self.store.get_all_schemas().await?;

        // Build index of inbound relationships
        let mut index: HashMap<String, Vec<InboundRelationship>> = HashMap::new();

        for schema in schemas {
            let source_type = &schema.id;

            for relationship in &schema.relationships {
                let inbound = InboundRelationship {
                    source_type: source_type.clone(),
                    relationship_name: relationship.name.clone(),
                    reverse_name: relationship.reverse_name.clone(),
                    cardinality: relationship.cardinality.clone(),
                    reverse_cardinality: relationship.reverse_cardinality.clone(),
                    edge_table: relationship.compute_edge_table_name(source_type),
                    description: relationship.description.clone(),
                };

                index
                    .entry(relationship.target_type.clone())
                    .or_default()
                    .push(inbound);
            }
        }

        // Atomic swap
        {
            let mut cache = self.cache.write().await;
            *cache = index;
        }

        // Update refresh time and clear change flag
        {
            let mut last = self.last_refresh.write().await;
            *last = Some(Instant::now());
        }
        self.schema_change_flag.store(false, Ordering::Release);

        Ok(())
    }

    /// Get cache statistics (for debugging/monitoring)
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        let last = self.last_refresh.read().await;

        CacheStats {
            target_types: cache.len(),
            total_relationships: cache.values().map(|v| v.len()).sum(),
            last_refresh: *last,
            is_stale: self.needs_refresh().await,
        }
    }
}

/// Statistics about the relationship cache
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of target types indexed
    pub target_types: usize,
    /// Total number of inbound relationships cached
    pub total_relationships: usize,
    /// Last refresh time (None if never refreshed)
    pub last_refresh: Option<Instant>,
    /// Whether the cache is currently considered stale
    pub is_stale: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::RelationshipCardinality;
    use serde_json::json;

    #[test]
    fn test_inbound_relationship_serialization() {
        let inbound = InboundRelationship {
            source_type: "invoice".to_string(),
            relationship_name: "billed_to".to_string(),
            reverse_name: Some("invoices".to_string()),
            cardinality: RelationshipCardinality::One,
            reverse_cardinality: Some(RelationshipCardinality::Many),
            edge_table: "invoice_billed_to_customer".to_string(),
            description: Some("The customer this invoice is billed to".to_string()),
        };

        let json = serde_json::to_value(&inbound).unwrap();
        assert_eq!(json["sourceType"], "invoice");
        assert_eq!(json["relationshipName"], "billed_to");
        assert_eq!(json["reverseName"], "invoices");
        assert_eq!(json["cardinality"], "one");
        assert_eq!(json["reverseCardinality"], "many");
        assert_eq!(json["edgeTable"], "invoice_billed_to_customer");
    }

    #[test]
    fn test_inbound_relationship_deserialization() {
        let json = json!({
            "sourceType": "task",
            "relationshipName": "assigned_to",
            "reverseName": "tasks",
            "cardinality": "many",
            "reverseCardinality": "many",
            "edgeTable": "task_assigned_to_person"
        });

        let inbound: InboundRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(inbound.source_type, "task");
        assert_eq!(inbound.relationship_name, "assigned_to");
        assert_eq!(inbound.reverse_name, Some("tasks".to_string()));
        assert_eq!(inbound.cardinality, RelationshipCardinality::Many);
        assert_eq!(
            inbound.reverse_cardinality,
            Some(RelationshipCardinality::Many)
        );
    }

    #[test]
    fn test_inbound_relationship_minimal() {
        let json = json!({
            "sourceType": "doc",
            "relationshipName": "references",
            "cardinality": "many",
            "edgeTable": "doc_references_doc"
        });

        let inbound: InboundRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(inbound.source_type, "doc");
        assert!(inbound.reverse_name.is_none());
        assert!(inbound.reverse_cardinality.is_none());
        assert!(inbound.description.is_none());
    }
}
