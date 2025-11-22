use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use nodespace_core::SurrealStore;

/// Node data structure from database
/// Note: Field names match snake_case database schema (node_type, modified_at)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    pub id: String,
    pub content: String,
    pub node_type: String,
    pub version: i32,
    pub modified_at: String,
}

/// Edge data structure from database (for internal tracking)
/// Contains id for change detection, but id is not sent to frontend
#[derive(Debug, Clone, Deserialize)]
struct EdgeDbRecord {
    pub id: String,
    #[serde(rename = "in")]
    pub parent_id: String,
    #[serde(rename = "out")]
    pub child_id: String,
    pub order: f64,
}

/// Hierarchy relationship for frontend (domain-focused naming)
/// Excludes edge ID as frontend doesn't need it
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HierarchyRelationship {
    pub parent_id: String,
    pub child_id: String,
    pub order: f64,
}

impl From<&EdgeDbRecord> for HierarchyRelationship {
    fn from(edge: &EdgeDbRecord) -> Self {
        Self {
            parent_id: edge.parent_id.clone(),
            child_id: edge.child_id.clone(),
            order: edge.order,
        }
    }
}

/// Service for managing real-time database synchronization
///
/// Uses polling-based change detection as an MVP implementation.
/// Will migrate to LIVE SELECT when the SurrealDB Rust driver
/// provides proper streaming support.
pub struct LiveQueryService {
    store: Arc<SurrealStore>,
    app: AppHandle,
    node_versions: Arc<RwLock<HashMap<String, i32>>>,
    edge_hashes: Arc<RwLock<HashMap<String, u64>>>,
    poll_interval_ms: u64,
    reconnect_attempts: Arc<RwLock<u32>>,
    max_reconnect_attempts: u32,
}

impl LiveQueryService {
    pub fn new(store: Arc<SurrealStore>, app: AppHandle) -> Self {
        Self {
            store,
            app,
            node_versions: Arc::new(RwLock::new(HashMap::new())),
            edge_hashes: Arc::new(RwLock::new(HashMap::new())),
            poll_interval_ms: 1000, // 1 second polling interval for MVP
            reconnect_attempts: Arc::new(RwLock::new(0)),
            max_reconnect_attempts: 10,
        }
    }

    /// Start the real-time synchronization service with polling-based change detection
    ///
    /// This implementation polls the database for changes at regular intervals.
    /// Detects changes by tracking:
    /// - Node versions (incremented on each update)
    /// - Edge hashes (computed from edge data)
    ///
    /// Emits Tauri events when changes are detected:
    /// - node:created, node:updated, node:deleted
    /// - edge:created, edge:updated, edge:deleted
    pub async fn run(self) -> Result<()> {
        info!("🔧 Starting polling-based real-time synchronization service");
        info!("📊 Poll interval: {}ms", self.poll_interval_ms);

        // Emit initial status
        self.emit_status("connected", None);

        // Initialize baseline state
        if let Err(e) = self.initialize_baseline().await {
            error!("Failed to initialize baseline state: {}", e);
            self.emit_status("disconnected", Some("initialization-failed"));
            return Err(e);
        }

        info!("✅ Baseline state initialized successfully");

        // Start polling loop with exponential backoff on errors
        loop {
            match self.poll_for_changes().await {
                Ok(_) => {
                    // Reset reconnect attempts on success
                    *self.reconnect_attempts.write().await = 0;

                    // Wait for next poll interval
                    tokio::time::sleep(tokio::time::Duration::from_millis(self.poll_interval_ms))
                        .await;
                }
                Err(e) => {
                    let attempts = {
                        let mut attempts = self.reconnect_attempts.write().await;
                        *attempts += 1;
                        *attempts
                    };

                    if attempts >= self.max_reconnect_attempts {
                        error!(
                            "Max reconnection attempts ({}) reached, giving up",
                            self.max_reconnect_attempts
                        );
                        self.emit_status("disconnected", Some("max-retries-exceeded"));
                        return Err(anyhow::anyhow!("Max reconnection attempts exceeded"));
                    }

                    // Exponential backoff: 2^attempts seconds (capped at 60s)
                    let backoff_secs = std::cmp::min(2u64.pow(attempts), 60);
                    warn!(
                        "Polling error (attempt {}/{}): {}. Retrying in {}s",
                        attempts, self.max_reconnect_attempts, e, backoff_secs
                    );

                    self.emit_status("reconnecting", Some(&format!("retry-{}", attempts)));
                    tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    }

    /// Initialize baseline state by loading all current nodes and edges
    async fn initialize_baseline(&self) -> Result<()> {
        info!("Initializing baseline state...");

        // Load all nodes
        let nodes: Vec<NodeData> = self
            .store
            .db()
            .query("SELECT id, content, node_type, version, modified_at FROM node")
            .await?
            .take(0)?;

        let mut node_versions = self.node_versions.write().await;
        for node in nodes {
            node_versions.insert(node.id.clone(), node.version);
        }
        info!("Loaded {} nodes into baseline", node_versions.len());

        // Load all edges
        let edges: Vec<EdgeDbRecord> = self
            .store
            .db()
            .query("SELECT id, in, out, order FROM has_child")
            .await?
            .take(0)?;

        let mut edge_hashes = self.edge_hashes.write().await;
        for edge in edges {
            let hash = Self::compute_edge_hash(&edge);
            edge_hashes.insert(edge.id.clone(), hash);
        }
        info!("Loaded {} edges into baseline", edge_hashes.len());

        Ok(())
    }

    /// Poll database for changes and emit events
    async fn poll_for_changes(&self) -> Result<()> {
        debug!("Polling for database changes...");

        // Check for node changes
        self.check_node_changes().await?;

        // Check for edge changes
        self.check_edge_changes().await?;

        Ok(())
    }

    /// Check for node changes (created, updated, deleted)
    async fn check_node_changes(&self) -> Result<()> {
        let nodes: Vec<NodeData> = self
            .store
            .db()
            .query("SELECT id, content, node_type, version, modified_at FROM node")
            .await?
            .take(0)?;

        let mut node_versions = self.node_versions.write().await;
        let mut current_nodes = HashMap::new();

        for node in nodes {
            let node_id = node.id.clone();
            let version = node.version;

            current_nodes.insert(node_id.clone(), version);

            match node_versions.get(&node_id) {
                None => {
                    // New node created
                    debug!("Node created: {}", node_id);
                    self.emit_node_event("created", &node);
                    node_versions.insert(node_id, version);
                }
                Some(&old_version) if old_version < version => {
                    // Node updated
                    debug!(
                        "Node updated: {} (v{} -> v{})",
                        node_id, old_version, version
                    );
                    self.emit_node_event("updated", &node);
                    node_versions.insert(node_id, version);
                }
                _ => {
                    // No change
                }
            }
        }

        // Check for deleted nodes
        let deleted_nodes: Vec<String> = node_versions
            .keys()
            .filter(|id| !current_nodes.contains_key(*id))
            .cloned()
            .collect();

        for node_id in deleted_nodes {
            debug!("Node deleted: {}", node_id);
            self.emit_node_deleted(&node_id);
            node_versions.remove(&node_id);
        }

        Ok(())
    }

    /// Check for edge changes (created, updated, deleted)
    async fn check_edge_changes(&self) -> Result<()> {
        let edges: Vec<EdgeDbRecord> = self
            .store
            .db()
            .query("SELECT id, in, out, order FROM has_child")
            .await?
            .take(0)?;

        let mut edge_hashes = self.edge_hashes.write().await;
        let mut current_edges = HashMap::new();

        for edge in edges {
            let edge_id = edge.id.clone();
            let hash = Self::compute_edge_hash(&edge);

            current_edges.insert(edge_id.clone(), hash);

            match edge_hashes.get(&edge_id) {
                None => {
                    // New edge created
                    debug!("Edge created: {}", edge_id);
                    self.emit_edge_event("created", &edge);
                    edge_hashes.insert(edge_id, hash);
                }
                Some(&old_hash) if old_hash != hash => {
                    // Edge updated
                    debug!("Edge updated: {}", edge_id);
                    self.emit_edge_event("updated", &edge);
                    edge_hashes.insert(edge_id, hash);
                }
                _ => {
                    // No change
                }
            }
        }

        // Check for deleted edges
        let deleted_edges: Vec<String> = edge_hashes
            .keys()
            .filter(|id| !current_edges.contains_key(*id))
            .cloned()
            .collect();

        for edge_id in deleted_edges {
            debug!("Edge deleted: {}", edge_id);
            self.emit_edge_deleted(&edge_id);
            edge_hashes.remove(&edge_id);
        }

        Ok(())
    }

    /// Emit node event to Tauri frontend
    fn emit_node_event(&self, change_type: &str, node: &NodeData) {
        let event_name = format!("node:{}", change_type);

        if let Err(e) = self.app.emit(&event_name, node) {
            error!("Failed to emit {}: {}", event_name, e);
        }
    }

    /// Emit node deleted event
    fn emit_node_deleted(&self, node_id: &str) {
        #[derive(Serialize)]
        struct DeletedPayload {
            id: String,
        }

        let payload = DeletedPayload {
            id: node_id.to_string(),
        };

        if let Err(e) = self.app.emit("node:deleted", &payload) {
            error!("Failed to emit node:deleted: {}", e);
        }
    }

    /// Emit edge event to Tauri frontend
    /// Converts internal EdgeDbRecord to HierarchyRelationship at the serialization boundary
    fn emit_edge_event(&self, change_type: &str, edge: &EdgeDbRecord) {
        let event_name = format!("edge:{}", change_type);
        let relationship: HierarchyRelationship = edge.into();

        if let Err(e) = self.app.emit(&event_name, &relationship) {
            error!("Failed to emit {}: {}", event_name, e);
        }
    }

    /// Emit edge deleted event
    fn emit_edge_deleted(&self, edge_id: &str) {
        #[derive(Serialize)]
        struct DeletedPayload {
            id: String,
        }

        let payload = DeletedPayload {
            id: edge_id.to_string(),
        };

        if let Err(e) = self.app.emit("edge:deleted", &payload) {
            error!("Failed to emit edge:deleted: {}", e);
        }
    }

    /// Emit synchronization status event
    fn emit_status(&self, status: &str, reason: Option<&str>) {
        #[derive(Serialize)]
        struct StatusPayload {
            status: String,
            reason: Option<String>,
        }

        let payload = StatusPayload {
            status: status.to_string(),
            reason: reason.map(|s| s.to_string()),
        };

        if let Err(e) = self.app.emit("sync:status", &payload) {
            error!("Failed to emit sync:status: {}", e);
        }
    }

    /// Compute hash of edge data for change detection
    fn compute_edge_hash(edge: &EdgeDbRecord) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        edge.parent_id.hash(&mut hasher);
        edge.child_id.hash(&mut hasher);
        edge.order.to_bits().hash(&mut hasher);
        hasher.finish()
    }
}
