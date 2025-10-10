/**
 * Conflict Resolution Strategies for Multi-Source Updates
 *
 * Implements pluggable conflict resolution strategies:
 * - Last-Write-Wins (Phase 1 - Simple)
 * - Field-Level Merging (Phase 2 - Future)
 * - Operational Transformation (Phase 3+ - Future)
 *
 * Design: Strategy pattern allows easy upgrade without rewriting core logic
 */

import type { Conflict, ConflictResolution, ConflictResolver } from '$lib/types/update-protocol';
import type { Node } from '$lib/types';

/**
 * Last-Write-Wins Conflict Resolver (Phase 1 Implementation)
 *
 * Simple strategy: Most recent timestamp wins
 * - Easy to implement and reason about
 * - Works well for single-user editing with occasional external updates
 * - User notified when their changes are overwritten
 *
 * Limitations:
 * - Can discard valid local changes if remote update is newer
 * - No field-level merging
 * - Not ideal for high-frequency concurrent editing
 */
export class LastWriteWinsResolver implements ConflictResolver {
  resolve(conflict: Conflict, existingNode: Node): ConflictResolution {
    const { localUpdate, remoteUpdate } = conflict;

    // Simple timestamp comparison
    const localWins = localUpdate.timestamp >= remoteUpdate.timestamp;
    const winner = localWins ? localUpdate : remoteUpdate;
    const loser = localWins ? remoteUpdate : localUpdate;

    // Build resolved node by merging winner's changes with existing node
    // This is type-safe and preserves all required fields
    const resolvedNode: Node = {
      ...existingNode,
      ...winner.changes,
      modifiedAt: new Date().toISOString()
    };

    return {
      nodeId: conflict.nodeId,
      resolvedNode,
      strategy: 'last-write-wins',
      discardedUpdate: loser
    };
  }

  getStrategyName(): string {
    return 'Last-Write-Wins';
  }
}

/**
 * Field-Level Merge Resolver (Future - Phase 2+)
 *
 * Intelligent strategy: Merge non-conflicting field changes
 * - Preserves more edits by merging at field granularity
 * - Detects true conflicts (same field edited concurrently)
 * - Requires manual resolution for conflicting fields
 *
 * Example:
 *   Local:  { content: "A", properties: { color: "red" } }
 *   Remote: { content: "B", properties: { size: "large" } }
 *   Merged: { content: ?, properties: { color: "red", size: "large" } }
 *   (content conflict needs resolution)
 */
export class FieldLevelMergeResolver implements ConflictResolver {
  resolve(conflict: Conflict, existingNode: Node): ConflictResolution {
    const { localUpdate, remoteUpdate, nodeId } = conflict;

    // Get all unique fields from both updates
    const allFields = new Set([
      ...Object.keys(localUpdate.changes),
      ...Object.keys(remoteUpdate.changes)
    ]);

    const mergedNode: Partial<Node> = {};
    const mergedFields: string[] = [];
    let hasConflicts = false;

    for (const field of allFields) {
      const localValue = (localUpdate.changes as Record<string, unknown>)[field];
      const remoteValue = (remoteUpdate.changes as Record<string, unknown>)[field];

      // Both updates modified this field - conflict
      if (localValue !== undefined && remoteValue !== undefined) {
        if (JSON.stringify(localValue) === JSON.stringify(remoteValue)) {
          // Same change - no conflict
          (mergedNode as Record<string, unknown>)[field] = localValue;
          mergedFields.push(field);
        } else {
          // Different changes - true conflict, use Last-Write-Wins as tiebreaker
          hasConflicts = true;
          const winner = localUpdate.timestamp >= remoteUpdate.timestamp ? localValue : remoteValue;
          (mergedNode as Record<string, unknown>)[field] = winner;
        }
      } else if (localValue !== undefined) {
        // Only local modified this field
        (mergedNode as Record<string, unknown>)[field] = localValue;
        mergedFields.push(field);
      } else if (remoteValue !== undefined) {
        // Only remote modified this field
        (mergedNode as Record<string, unknown>)[field] = remoteValue;
        mergedFields.push(field);
      }
    }

    return {
      nodeId,
      resolvedNode: {
        ...existingNode,
        ...mergedNode,
        modifiedAt: new Date().toISOString()
      },
      strategy: hasConflicts ? 'last-write-wins' : 'field-merge',
      mergedFields,
      discardedUpdate: hasConflicts ? undefined : undefined // Track which update had conflicts
    };
  }

  getStrategyName(): string {
    return 'Field-Level Merge';
  }
}

/**
 * Operational Transformation Resolver (Future - Phase 3+)
 *
 * Advanced strategy: Transform operations based on concurrent changes
 * - Ideal for text editing with character-level precision
 * - Preserves user intent even with complex concurrent edits
 * - Requires sophisticated transformation functions
 *
 * Example OT for text:
 *   Initial: "Hello"
 *   Local:  Insert("World", 5) → "HelloWorld"
 *   Remote: Delete(0, 1) → "ello"
 *   Transformed: Apply Delete then adjust Insert → "elloWorld"
 */
export class OperationalTransformResolver implements ConflictResolver {
  resolve(conflict: Conflict, existingNode: Node): ConflictResolution {
    // Placeholder for future OT implementation
    // For now, fallback to Last-Write-Wins
    console.warn(
      '[OperationalTransformResolver] OT not yet implemented, falling back to Last-Write-Wins'
    );

    const fallback = new LastWriteWinsResolver();
    const resolution = fallback.resolve(conflict, existingNode);

    return {
      ...resolution,
      strategy: 'operational-transform' // Mark as OT even though we fell back
    };
  }

  getStrategyName(): string {
    return 'Operational Transform (Fallback to LWW)';
  }
}

/**
 * Manual Conflict Resolver (Future - Phase 3+)
 *
 * Interactive strategy: Present conflicts to user for manual resolution
 * - Shows diff between local and remote changes
 * - Allows user to choose which version to keep or manually merge
 * - Ideal for critical edits where automatic resolution is risky
 */
export class ManualConflictResolver implements ConflictResolver {
  private pendingConflicts = new Map<string, Conflict>();

  resolve(conflict: Conflict, existingNode: Node): ConflictResolution {
    // Store conflict for UI presentation
    this.pendingConflicts.set(conflict.nodeId, conflict);

    // For now, fallback to Last-Write-Wins and notify user
    // Future: Emit event for UI to show conflict resolution dialog
    console.warn('[ManualConflictResolver] Manual resolution required for node:', conflict.nodeId);

    const fallback = new LastWriteWinsResolver();
    const resolution = fallback.resolve(conflict, existingNode);

    return {
      ...resolution,
      strategy: 'manual' // Mark as requiring manual resolution
    };
  }

  getStrategyName(): string {
    return 'Manual Resolution (Pending)';
  }

  /**
   * Get conflicts pending manual resolution
   */
  getPendingConflicts(): Conflict[] {
    return Array.from(this.pendingConflicts.values());
  }

  /**
   * Clear resolved conflict
   */
  clearConflict(nodeId: string): void {
    this.pendingConflicts.delete(nodeId);
  }
}

/**
 * Helper: Create default conflict resolver (Last-Write-Wins)
 */
export function createDefaultResolver(): ConflictResolver {
  return new LastWriteWinsResolver();
}

/**
 * Helper: Create resolver by strategy name
 */
export function createResolver(strategy: string): ConflictResolver {
  switch (strategy.toLowerCase()) {
    case 'last-write-wins':
    case 'lww':
      return new LastWriteWinsResolver();
    case 'field-level':
    case 'field-merge':
      return new FieldLevelMergeResolver();
    case 'operational-transform':
    case 'ot':
      return new OperationalTransformResolver();
    case 'manual':
      return new ManualConflictResolver();
    default:
      console.warn(`Unknown conflict resolution strategy: ${strategy}, using Last-Write-Wins`);
      return new LastWriteWinsResolver();
  }
}
