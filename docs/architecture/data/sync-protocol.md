# NodeSpace Sync Protocol

## Overview

The NodeSpace Sync Protocol enables **eventually consistent** data synchronization between local LanceDB instances and cloud coordination services. This document defines the protocol, data structures, conflict resolution strategies, and implementation patterns for multi-user NodeSpace deployments.

## Architecture Overview

### Sync Flow Diagram
```
┌─────────────────┐    Push Changes    ┌─────────────────┐    Pull Changes    ┌─────────────────┐
│   User A        │───────────────────►│   Cloud Sync    │◄───────────────────│   User B        │
│   LanceDB       │                    │   PostgreSQL    │                    │   LanceDB       │
│                 │◄───────────────────│                 │───────────────────►│                 │
│ (Source Truth)  │    Conflict Res.   │ (Coordination)  │   Change Events    │ (Source Truth)  │
└─────────────────┘                    └─────────────────┘                    └─────────────────┘
```

### Core Principles
1. **Eventually Consistent**: All nodes converge to same state over time
2. **Local Authority**: Each user's LanceDB is authoritative for their operations
3. **Conflict Resolution**: Automatic for simple cases, manual for complex ones
4. **Offline Support**: Full functionality without network connectivity
5. **Incremental Sync**: Only transmit changes, not full state

## Data Structures

### Operation Log Entry
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOperation {
    pub id: String,                    // Unique operation ID (UUID)
    pub node_id: String,               // Target node ID
    pub user_id: String,               // User who made the change
    pub operation_type: OperationType, // Create, Update, Delete
    pub timestamp: DateTime<Utc>,      // When operation occurred
    pub version: u64,                  // Logical vector clock
    pub parent_version: Option<u64>,   // Previous version for this node
    pub data: OperationData,           // Operation-specific data
    pub checksum: String,              // Data integrity verification
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationType {
    Create,
    Update,
    Delete,
    Move,        // Parent/sibling changes
    Metadata,    // Metadata-only updates
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationData {
    Create {
        node: Node,
        embedding: Option<Vec<f32>>,
    },
    Update {
        content_delta: Option<String>,     // Content changes (diff format)
        metadata_changes: Option<JsonValue>, // Changed metadata fields
        embedding_delta: Option<Vec<f32>>,  // Updated embedding
    },
    Delete {
        soft_delete: bool,                 // Soft vs hard delete
    },
    Move {
        new_parent_id: Option<String>,
        insert_after: Option<String>,  // Sibling to insert after (Issue #614)
    },
    Metadata {
        changes: JsonValue,                // Only metadata changes
    },
}
```

### Sync Packet
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPacket {
    pub user_id: String,
    pub device_id: String,
    pub operations: Vec<SyncOperation>,
    pub sync_timestamp: DateTime<Utc>,
    pub last_known_version: u64,        // Last version client has seen
    pub vector_checksums: HashMap<String, String>, // Vector integrity
    pub compression: CompressionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Gzip,
    Zstd,    // For large datasets
}
```

### Conflict Resolution Context
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictContext {
    pub node_id: String,
    pub conflict_type: ConflictType,
    pub local_operation: SyncOperation,
    pub remote_operations: Vec<SyncOperation>,
    pub resolution_strategy: ResolutionStrategy,
    pub requires_user_input: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictType {
    ConcurrentEdit,      // Same field edited simultaneously
    StructuralConflict,  // Type change vs content edit
    DeleteConflict,      // Edit vs delete
    MoveConflict,        // Node moved by multiple users
    MetadataConflict,    // Metadata field conflicts
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResolutionStrategy {
    LastWriteWins,       // Use timestamp to resolve
    MergeWhenPossible,   // Automatic merge if non-conflicting
    ThreeWayMerge,       // Common ancestor-based merge
    UserChooses,         // Prompt user for resolution
    OperationalTransform, // Use OT for text conflicts
}
```

## Protocol Implementation

### 1. Operation Generation
```typescript
class OperationLogger {
    private localVersion = 0;
    private pendingOperations: SyncOperation[] = [];
    
    async logNodeUpdate(nodeId: string, changes: Partial<Node>): Promise<SyncOperation> {
        this.localVersion++;
        
        const operation: SyncOperation = {
            id: generateUUID(),
            node_id: nodeId,
            user_id: this.currentUser.id,
            operation_type: 'Update',
            timestamp: new Date(),
            version: this.localVersion,
            parent_version: await this.getLastNodeVersion(nodeId),
            data: {
                Update: {
                    content_delta: changes.content ? 
                        this.computeContentDiff(nodeId, changes.content) : null,
                    metadata_changes: changes.metadata ? 
                        this.computeMetadataDiff(nodeId, changes.metadata) : null,
                    embedding_delta: changes.embedding_vector || null
                }
            },
            checksum: this.computeChecksum(changes)
        };
        
        // Apply locally first
        await this.localStorage.applyOperation(operation);
        
        // Queue for sync
        this.pendingOperations.push(operation);
        
        // Trigger sync if online
        if (navigator.onLine) {
            this.scheduleSync();
        }
        
        return operation;
    }
}
```

### 2. Sync Orchestration
```typescript
class SyncOrchestrator {
    private syncInterval = 30_000; // 30 seconds
    private syncInProgress = false;
    private backoffMultiplier = 1;
    
    async performSync(): Promise<SyncResult> {
        if (this.syncInProgress || !navigator.onLine) {
            return { status: 'skipped', reason: 'already_syncing_or_offline' };
        }
        
        this.syncInProgress = true;
        
        try {
            // 1. Prepare sync packet
            const syncPacket = await this.prepareSyncPacket();
            
            if (syncPacket.operations.length === 0) {
                return { status: 'success', changes: 0 };
            }
            
            // 2. Send to cloud
            const response = await this.sendSyncPacket(syncPacket);
            
            // 3. Process response
            await this.processSyncResponse(response);
            
            // 4. Handle conflicts if any
            if (response.conflicts.length > 0) {
                await this.resolveConflicts(response.conflicts);
            }
            
            // Reset backoff on success
            this.backoffMultiplier = 1;
            
            return { 
                status: 'success', 
                changes: syncPacket.operations.length,
                conflicts: response.conflicts.length
            };
            
        } catch (error) {
            console.error('Sync failed:', error);
            
            // Exponential backoff
            this.backoffMultiplier = Math.min(this.backoffMultiplier * 2, 16);
            
            return { status: 'failed', error: error.message };
        } finally {
            this.syncInProgress = false;
            
            // Schedule next sync with backoff
            setTimeout(() => this.performSync(), 
                this.syncInterval * this.backoffMultiplier);
        }
    }
    
    private async prepareSyncPacket(): Promise<SyncPacket> {
        const pendingOps = await this.localStorage.getPendingOperations();
        
        return {
            user_id: this.currentUser.id,
            device_id: this.deviceId,
            operations: pendingOps,
            sync_timestamp: new Date(),
            last_known_version: await this.localStorage.getLastKnownServerVersion(),
            vector_checksums: await this.computeVectorChecksums(pendingOps),
            compression: pendingOps.length > 10 ? 'Gzip' : 'None'
        };
    }
}
```

### 3. Cloud Sync Service
```rust
// Rust backend service for sync coordination
pub struct CloudSyncService {
    postgres: Pool<Postgres>,
    redis: RedisClient,
}

impl CloudSyncService {
    pub async fn process_sync_packet(
        &self, 
        packet: SyncPacket
    ) -> Result<SyncResponse, SyncError> {
        let mut response = SyncResponse::new();
        
        // Start database transaction
        let mut tx = self.postgres.begin().await?;
        
        for operation in packet.operations {
            match self.apply_operation(&mut tx, &operation).await {
                Ok(_) => {
                    response.applied_operations.push(operation.id);
                }
                Err(ConflictError(conflict)) => {
                    response.conflicts.push(conflict);
                }
                Err(other) => return Err(other),
            }
        }
        
        // Get operations this client hasn't seen
        let new_operations = self.get_operations_since(
            &mut tx,
            &packet.user_id,
            packet.last_known_version
        ).await?;
        
        response.new_operations = new_operations;
        response.latest_version = self.get_latest_version(&mut tx).await?;
        
        // Commit transaction
        tx.commit().await?;
        
        // Notify other clients via Redis pub/sub
        self.notify_other_clients(&packet.user_id, &response).await?;
        
        Ok(response)
    }
    
    async fn apply_operation(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        operation: &SyncOperation,
    ) -> Result<(), SyncError> {
        // Check for conflicts
        let existing_ops = sqlx::query!(
            r#"
            SELECT * FROM sync_log 
            WHERE node_id = $1 AND version > $2 AND user_id != $3
            ORDER BY version ASC
            "#,
            operation.node_id,
            operation.parent_version.unwrap_or(0) as i64,
            operation.user_id
        ).fetch_all(tx).await?;
        
        if !existing_ops.is_empty() {
            // Potential conflict - analyze
            let conflict_type = self.detect_conflict_type(operation, &existing_ops);
            
            match conflict_type {
                ConflictType::ConcurrentEdit => {
                    // Try automatic merge
                    if let Some(merged) = self.attempt_auto_merge(operation, &existing_ops) {
                        self.store_operation(tx, &merged).await?;
                        return Ok(());
                    }
                }
                _ => {
                    // Requires manual resolution
                    return Err(SyncError::Conflict(ConflictContext {
                        node_id: operation.node_id.clone(),
                        conflict_type,
                        local_operation: operation.clone(),
                        remote_operations: existing_ops.into_iter()
                            .map(|row| self.row_to_operation(row))
                            .collect(),
                        resolution_strategy: ResolutionStrategy::UserChooses,
                        requires_user_input: true,
                    }));
                }
            }
        }
        
        // No conflict - apply operation
        self.store_operation(tx, operation).await?;
        Ok(())
    }
}
```

## Conflict Resolution Strategies

### 1. Last Write Wins (Simple)
```rust
impl ConflictResolver {
    fn resolve_last_write_wins(
        &self,
        local: &SyncOperation,
        remote: &[SyncOperation],
    ) -> SyncOperation {
        let mut latest = local.clone();
        
        for op in remote {
            if op.timestamp > latest.timestamp {
                latest = op.clone();
            }
        }
        
        latest
    }
}
```

### 2. Three-Way Merge (Content)
```rust
impl ConflictResolver {
    async fn resolve_three_way_merge(
        &self,
        local: &SyncOperation,
        remote: &SyncOperation,
        node_id: &str,
    ) -> Result<SyncOperation, ConflictError> {
        // Get common ancestor
        let ancestor = self.get_common_ancestor(node_id, local, remote).await?;
        
        match (&local.data, &remote.data, &ancestor.data) {
            (
                OperationData::Update { content_delta: Some(local_content), .. },
                OperationData::Update { content_delta: Some(remote_content), .. },
                OperationData::Update { content_delta: Some(ancestor_content), .. }
            ) => {
                // Perform three-way text merge
                let merged_content = self.merge_text_content(
                    ancestor_content,
                    local_content,
                    remote_content,
                )?;
                
                Ok(SyncOperation {
                    id: generate_uuid(),
                    timestamp: std::cmp::max(local.timestamp, remote.timestamp),
                    data: OperationData::Update {
                        content_delta: Some(merged_content),
                        metadata_changes: self.merge_metadata(
                            &local.data, &remote.data
                        ),
                        embedding_delta: None, // Recompute after merge
                    },
                    ..local.clone()
                })
            }
            _ => Err(ConflictError::CannotMerge("Incompatible operation types".to_string()))
        }
    }
}
```

### 3. Operational Transform (Real-time)
```typescript
class OperationalTransformer {
    // Transform operation A against operation B
    // Used for real-time collaborative editing
    transformOperation(opA: TextOperation, opB: TextOperation): TextOperation {
        if (opA.type === 'insert' && opB.type === 'insert') {
            if (opA.position <= opB.position) {
                return {
                    ...opB,
                    position: opB.position + opA.content.length
                };
            }
        }
        
        if (opA.type === 'delete' && opB.type === 'insert') {
            if (opA.position < opB.position) {
                return {
                    ...opB,
                    position: Math.max(opB.position - opA.length, opA.position)
                };
            }
        }
        
        // More transformation rules...
        return opB;
    }
}
```

## Vector Embedding Synchronization

### Embedding Sync Boundaries — Local Commit Before Sync

**Core principle: Sync operates on committed state, not in-progress state.**

A node is not considered "committed" until all its derived data (including embeddings)
has been computed. This mirrors database transaction semantics — partial writes are
never exposed to other readers. The sync outbox only contains fully resolved nodes.

**Rules:**

1. **A node does not enter the sync outbox until its embedding is complete.** When
   content changes, the node is marked stale locally and held back from sync until
   re-embedding finishes. The sync layer never sees intermediate state.

2. **Stale markers are local-only.** They drive the local embedding pipeline and are
   never transmitted to other clients. No client should ever receive a node in a
   stale/pending state.

3. **Each synced node is an atomic unit:** content + embedding vector + content hash
   (`sha256(content)`) + model ID. Receiving clients can trust that the embedding
   matches the content.

```
Sync outbox eligibility:
  ✅ Root node with completed embedding (content_hash covers root + all children)
     → entire subtree (root + all descendants) syncs as one atomic unit
  ❌ Root node with stale/pending embedding → entire subtree held back
  ❌ Stale markers (never synced)
```

**Subtree as atomic sync unit**: Embeddings are computed only at the root node
level, incorporating the content of all descendant nodes. Therefore the sync
unit is the **entire subtree** — root node + all children. If a child node's
content changes, the root's embedding becomes stale, and the whole subtree is
held from the sync outbox until re-embedding completes. When it does, the root
and all its descendants sync together as one atomic operation.

**Edge cases:**

- **Database switch before re-embedding completes**: The pending nodes remain in the
  local database but never enter the sync outbox. If the database is never opened
  again, those nodes simply don't sync — which is correct, since no client is
  actively using that database.

- **Embedding model failure**: If embeddings cannot be computed (GPU error, model
  won't load), affected nodes are held from sync. This is surfaced as a user-visible
  error. The sync layer does not attempt to work around broken local state.

- **Node update with pending re-embedding**: When an existing node's content changes,
  the sync outbox retains the last committed version (previous content + its matching
  embedding) until the new embedding completes. Once re-embedding finishes, the update
  becomes the new committed state and replaces the previous version in the outbox.
  This is analogous to offline edits — the user keeps working locally, but the sync
  layer only transmits fully resolved state.

- **Receiving a synced node**: Because only committed nodes are synced, the receiver
  accepts the node + embedding as-is. No receiver-side audit or re-embedding needed.

### Efficient Vector Sync
```rust
pub struct VectorSyncOptimizer {
    pub similarity_threshold: f32, // 0.95 - only sync if significant change
}

impl VectorSyncOptimizer {
    pub async fn should_sync_embedding(
        &self,
        old_embedding: &[f32],
        new_embedding: &[f32],
    ) -> bool {
        let similarity = self.cosine_similarity(old_embedding, new_embedding);
        similarity < self.similarity_threshold
    }
    
    pub fn compress_embedding(&self, embedding: &[f32]) -> Vec<u8> {
        // Use quantization for network transfer
        embedding.iter()
            .map(|&x| (x * 127.0) as i8 as u8)  // 8-bit quantization
            .collect()
    }
    
    pub async fn sync_vectors_batch(
        &self,
        embeddings: HashMap<String, Vec<f32>>
    ) -> Result<(), SyncError> {
        // Batch upload to S3 or similar
        let compressed_batch = self.compress_embedding_batch(embeddings);
        
        // Upload to cloud storage
        self.storage_client
            .upload_batch("embeddings/", compressed_batch)
            .await?;
        
        Ok(())
    }
}
```

## Performance Optimizations

### 1. Delta Compression
```rust
pub struct DeltaCompressor {
    compression_ratio_threshold: f32, // Only compress if >30% savings
}

impl DeltaCompressor {
    pub fn compute_content_delta(
        &self,
        original: &str,
        modified: &str,
    ) -> ContentDelta {
        let patches = diff::lines(original, modified);
        
        let delta = ContentDelta {
            format: DeltaFormat::LineDiff,
            patches: patches.into_iter().map(|patch| match patch {
                diff::Result::Left(line) => DeltaPatch::Delete(line.to_string()),
                diff::Result::Both(line, _) => DeltaPatch::Keep(line.to_string()),
                diff::Result::Right(line) => DeltaPatch::Insert(line.to_string()),
            }).collect(),
        };
        
        // Only use delta if it's more efficient
        if self.is_delta_efficient(&delta, original, modified) {
            delta
        } else {
            ContentDelta {
                format: DeltaFormat::FullReplace,
                patches: vec![DeltaPatch::Replace(modified.to_string())],
            }
        }
    }
}
```

### 2. Batch Synchronization
```typescript
class BatchSyncOptimizer {
    private readonly BATCH_SIZE = 50;
    private readonly BATCH_TIMEOUT = 5000; // 5 seconds
    private pendingBatch: SyncOperation[] = [];
    private batchTimer?: NodeJS.Timeout;
    
    queueOperation(operation: SyncOperation): void {
        this.pendingBatch.push(operation);
        
        if (this.pendingBatch.length >= this.BATCH_SIZE) {
            this.flushBatch();
        } else if (!this.batchTimer) {
            this.batchTimer = setTimeout(() => this.flushBatch(), this.BATCH_TIMEOUT);
        }
    }
    
    private async flushBatch(): Promise<void> {
        if (this.pendingBatch.length === 0) return;
        
        const batch = this.pendingBatch.splice(0);
        
        if (this.batchTimer) {
            clearTimeout(this.batchTimer);
            this.batchTimer = undefined;
        }
        
        try {
            await this.syncService.processBatch(batch);
        } catch (error) {
            // Re-queue failed operations
            this.pendingBatch.unshift(...batch);
            throw error;
        }
    }
}
```

## Error Handling and Recovery

### Sync Failure Recovery
```typescript
class SyncRecoveryManager {
    private readonly MAX_RETRIES = 5;
    private readonly RECOVERY_STRATEGIES = [
        'retry_with_backoff',
        'partial_sync',
        'force_reconciliation',
        'manual_intervention'
    ];
    
    async handleSyncFailure(error: SyncError, attempt: number): Promise<RecoveryAction> {
        if (attempt > this.MAX_RETRIES) {
            return this.escalateToManualIntervention(error);
        }
        
        switch (error.type) {
            case 'network_timeout':
                return this.retryWithBackoff(attempt);
                
            case 'conflict_resolution_failed':
                return this.attemptPartialSync(error.conflictedNodes);
                
            case 'data_corruption':
                return this.forceReconciliation(error.nodeId);
                
            case 'version_mismatch':
                return this.rebuildSyncState();
                
            default:
                return this.escalateToManualIntervention(error);
        }
    }
    
    private async forceReconciliation(nodeId: string): Promise<void> {
        // Last resort - fetch canonical version from server
        const serverVersion = await this.fetchCanonicalVersion(nodeId);
        const localVersion = await this.localStorage.getNode(nodeId);
        
        // Present conflict to user
        const resolution = await this.presentConflictDialog(
            localVersion,
            serverVersion
        );
        
        await this.applyResolution(nodeId, resolution);
    }
}
```

## Monitoring and Observability

### Sync Metrics
```rust
#[derive(Debug, Clone)]
pub struct SyncMetrics {
    pub operations_synced: u64,
    pub conflicts_resolved: u64,
    pub sync_latency_ms: u64,
    pub bandwidth_used: u64,
    pub error_rate: f32,
}

pub struct SyncMonitor {
    metrics: Arc<Mutex<SyncMetrics>>,
    prometheus_registry: Registry,
}

impl SyncMonitor {
    pub fn record_sync_operation(&self, operation: &SyncOperation, duration: Duration) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.operations_synced += 1;
        metrics.sync_latency_ms += duration.as_millis() as u64;
        
        // Export to Prometheus/observability system
        self.update_prometheus_metrics(&metrics);
    }
    
    pub fn record_conflict_resolution(&self, context: &ConflictContext) {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.conflicts_resolved += 1;
        
        // Log conflict details for analysis
        tracing::info!(
            node_id = context.node_id,
            conflict_type = ?context.conflict_type,
            resolution_strategy = ?context.resolution_strategy,
            "Conflict resolved"
        );
    }
}
```

## Security Considerations

### Operation Validation
```rust
pub struct SyncSecurity {
    rate_limiter: RateLimiter,
    operation_validator: OperationValidator,
}

impl SyncSecurity {
    pub async fn validate_sync_packet(
        &self,
        packet: &SyncPacket,
        user: &AuthenticatedUser,
    ) -> Result<(), SecurityError> {
        // Rate limiting
        if !self.rate_limiter.check_user_rate(user.id, packet.operations.len()) {
            return Err(SecurityError::RateLimitExceeded);
        }
        
        // Operation validation
        for operation in &packet.operations {
            self.validate_operation(operation, user)?;
        }
        
        // Integrity check
        self.verify_packet_integrity(packet)?;
        
        Ok(())
    }
    
    fn validate_operation(
        &self,
        operation: &SyncOperation,
        user: &AuthenticatedUser,
    ) -> Result<(), SecurityError> {
        // User authorization
        if operation.user_id != user.id {
            return Err(SecurityError::UnauthorizedOperation);
        }
        
        // Operation structure validation
        if !self.operation_validator.is_valid(operation) {
            return Err(SecurityError::MalformedOperation);
        }
        
        // Content sanitization
        if self.contains_malicious_content(&operation.data) {
            return Err(SecurityError::MaliciousContent);
        }
        
        Ok(())
    }
}
```

This sync protocol provides a robust foundation for multi-user NodeSpace deployments while maintaining the local-first principles that make NodeSpace fast and reliable for individual users.