/**
 * Update Protocol Types for Multi-Source Synchronization
 *
 * Defines types for tracking node updates from multiple sources (viewers, database, MCP server)
 * and coordinating real-time synchronization with conflict detection.
 *
 * Phase 2 Implementation: Multi-Source Update Handling
 */

import type { Node } from '$lib/types';

/**
 * Source of a node update
 * Used to track where changes originated for conflict resolution and debugging
 */
export type UpdateSource =
  | { type: 'viewer'; viewerId: string; userId?: string }
  | { type: 'database'; reason: string }
  | { type: 'mcp-server'; serverId?: string; agentId?: string }
  | { type: 'external'; source: string; description?: string };

/**
 * Complete node update with metadata for tracking and conflict resolution
 */
export interface NodeUpdate {
  nodeId: string;
  changes: Partial<Node>;
  source: UpdateSource;
  timestamp: number;
  version?: number; // For optimistic concurrency control
  previousVersion?: number; // Previous version for conflict detection
}

/**
 * Detected conflict between two concurrent updates
 */
export interface Conflict {
  nodeId: string;
  localUpdate: NodeUpdate;
  remoteUpdate: NodeUpdate;
  conflictType: 'concurrent-edit' | 'version-mismatch' | 'deleted-node';
  detectedAt: number;
}

/**
 * Result of conflict resolution
 */
export interface ConflictResolution {
  nodeId: string;
  resolvedNode: Node;
  strategy: 'last-write-wins' | 'field-merge' | 'manual' | 'operational-transform';
  discardedUpdate?: NodeUpdate; // Update that was overwritten
  mergedFields?: string[]; // Fields that were merged (for field-level resolution)
}

/**
 * Conflict resolver interface - pluggable strategy pattern
 * Allows upgrading from Last-Write-Wins to Field-Level or OT without rewriting core logic
 */
export interface ConflictResolver {
  /**
   * Resolve a conflict between two updates
   * @param conflict - The detected conflict
   * @param existingNode - The current state of the node before resolution
   * @returns The resolved node state
   */
  resolve(conflict: Conflict, existingNode: Node): ConflictResolution;

  /**
   * Get the name of this resolution strategy
   */
  getStrategyName(): string;
}

/**
 * Subscription callback for node changes
 */
export type NodeChangeCallback = (node: Node, source: UpdateSource) => void;

/**
 * Unsubscribe function returned by subscribe()
 */
export type Unsubscribe = () => void;

/**
 * Performance metrics for SharedNodeStore
 */
export interface StoreMetrics {
  updateCount: number;
  avgUpdateTime: number;
  maxUpdateTime: number;
  subscriptionCount: number;
  conflictCount: number;
  rollbackCount: number;
}

/**
 * Options for updateNode operations
 */
export interface UpdateOptions {
  /** Skip conflict detection (use for trusted sources like database) */
  skipConflictDetection?: boolean;
  /** Skip persistence (for temporary UI-only updates) */
  skipPersistence?: boolean;
  /** Force update even if version mismatch (dangerous) */
  force?: boolean;
  /** Notify subscribers even if no actual changes */
  forceNotify?: boolean;
  /** Additional dependencies for persistence sequencing (node IDs or lambda functions) */
  persistenceDependencies?: Array<string | (() => Promise<void>)>;
}
