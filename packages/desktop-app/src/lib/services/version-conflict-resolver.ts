/**
 * Version Conflict Resolution Service
 *
 * Implements automatic merge heuristics and provides conflict resolution strategies
 * for optimistic concurrency control version conflicts.
 *
 * Architecture:
 * - Auto-merge when possible (non-overlapping changes)
 * - Provide structured data for manual resolution UI
 * - Preserve user intent while maintaining data integrity
 */

import type { Node } from '$lib/types';
import type { VersionConflictData } from '$lib/types/errors';

/**
 * Conflict resolution result
 */
export interface ConflictResolution {
  /** Whether auto-merge was successful */
  autoMerged: boolean;

  /** Merged node (if auto-merge succeeded) */
  mergedNode?: Node;

  /** Resolution strategy used */
  strategy: 'auto-merged' | 'user-choice-required' | 'last-write-wins';

  /** Human-readable explanation of what was merged */
  explanation: string;
}

/**
 * ConflictResolver - Auto-merge logic for version conflicts
 */
export class ConflictResolver {
  /**
   * Attempt to automatically resolve a version conflict
   *
   * Auto-merge strategies (in priority order):
   * 1. Non-overlapping field changes → Merge both
   * 2. Only properties changed → Merge JSON properties
   * 3. Only content changed → Use newer version (last-write-wins)
   * 4. Both changed → Require manual resolution
   *
   * @param yourChanges - The changes you attempted to save
   * @param currentNode - The current node state from database
   * @param expectedVersion - The version you expected when making changes
   * @returns Resolution result with merged node or null if manual resolution needed
   */
  static tryAutoMerge(
    yourChanges: Partial<Node>,
    currentNode: Node,
    expectedVersion: number
  ): ConflictResolution {
    // Calculate what fields changed
    const changedFields = Object.keys(yourChanges);

    // Strategy 1: Non-overlapping changes (safest auto-merge)
    // Example: You changed properties, they changed content
    if (!this.hasOverlappingChanges(yourChanges, currentNode, expectedVersion)) {
      const mergedProperties = this.deepMergeProperties(
        currentNode.properties,
        yourChanges.properties
      );
      const mergedNode: Node = {
        ...currentNode,
        ...yourChanges,
        // Deep merge properties to prevent data loss (must be explicit for type safety)
        properties: mergedProperties ?? {},
        // Always use current version + 1 for next attempt
        version: currentNode.version
      };

      return {
        autoMerged: true,
        mergedNode,
        strategy: 'auto-merged',
        explanation: `Auto-merged: Your changes (${changedFields.join(', ')}) don't conflict with recent updates`
      };
    }

    // Strategy 2: Only properties changed → Merge JSON
    if (changedFields.length === 1 && changedFields[0] === 'properties' && yourChanges.properties) {
      const mergedProperties = {
        ...currentNode.properties,
        ...(yourChanges.properties as Record<string, unknown>)
      };

      const mergedNode: Node = {
        ...currentNode,
        properties: mergedProperties,
        version: currentNode.version
      };

      return {
        autoMerged: true,
        mergedNode,
        strategy: 'auto-merged',
        explanation:
          'Auto-merged: Properties combined (your properties override current properties for same keys)'
      };
    }

    // Strategy 3: Only content changed → Last-write-wins
    if (changedFields.length === 1 && changedFields[0] === 'content') {
      // Content conflicts usually mean simultaneous editing
      // Use last-write-wins but inform user
      const mergedNode: Node = {
        ...currentNode,
        content: yourChanges.content as string,
        version: currentNode.version
      };

      return {
        autoMerged: true,
        mergedNode,
        strategy: 'last-write-wins',
        explanation: 'Content conflict resolved: Using your version (last-write-wins)'
      };
    }

    // Strategy 4: Multiple conflicting changes → Manual resolution required
    return {
      autoMerged: false,
      strategy: 'user-choice-required',
      explanation: `Cannot auto-merge: Multiple fields changed (${changedFields.join(', ')}). Manual resolution required.`
    };
  }

  /**
   * Check if changes overlap with what was modified in current version
   *
   * This is a conservative check - if we can't determine what changed,
   * we assume overlap (safe default).
   *
   * @param yourChanges - Fields you're trying to change
   * @param currentNode - Current node state
   * @param expectedVersion - Version you expected
   * @returns True if changes likely overlap
   */
  private static hasOverlappingChanges(
    yourChanges: Partial<Node>,
    currentNode: Node,
    expectedVersion: number
  ): boolean {
    // If version difference > 1, multiple updates happened
    // Too risky to auto-merge without knowing full history
    const versionGap = currentNode.version - expectedVersion;
    if (versionGap > 1) {
      return true; // Assume overlap for safety
    }

    // For single version gap, check if same fields would be modified
    // This is heuristic-based since we don't have full history
    const changedFields = new Set(Object.keys(yourChanges));

    // Empty changes with version gap = 1 don't overlap (safe to auto-merge)
    // IMPORTANT: Only skip conflict if version gap is exactly 1
    if (changedFields.size === 0 && versionGap === 1) {
      return false; // Safe: no changes and only one version behind
    }

    // Content changes are most likely to overlap
    if (changedFields.has('content')) {
      return true;
    }

    // Structural changes (parent, sibling) rarely overlap
    if (changedFields.has('parentId') || changedFields.has('beforeSiblingId')) {
      return false;
    }

    // Properties-only changes can usually be merged
    if (changedFields.size === 1 && changedFields.has('properties')) {
      return false;
    }

    // Default: assume overlap for safety
    return true;
  }

  /**
   * Create resolution from VersionConflictData (from MCP error)
   *
   * @param conflictData - Conflict data from MCP error response
   * @param yourChanges - The changes you attempted to save
   * @returns Conflict resolution result
   */
  static resolveFromConflictData(
    conflictData: VersionConflictData,
    yourChanges: Partial<Node>
  ): ConflictResolution {
    return this.tryAutoMerge(yourChanges, conflictData.current_node, conflictData.expected_version);
  }

  /**
   * Create user-choice resolution strategies for manual resolution UI
   *
   * @param yourChanges - Your attempted changes
   * @param currentNode - Current database state
   * @returns Possible resolution options for user to choose from
   */
  static getUserChoiceOptions(
    yourChanges: Partial<Node>,
    currentNode: Node
  ): {
    useYours: Node;
    useCurrent: Node;
    yourChangesDescription: string;
    currentChangesDescription: string;
  } {
    // Option 1: Use your version (overwrites current)
    const useYours: Node = {
      ...currentNode,
      ...yourChanges,
      version: currentNode.version
    };

    // Option 2: Use current version (discard your changes)
    const useCurrent: Node = currentNode;

    // Describe what changed
    const yourChangedFields = Object.keys(yourChanges);
    const yourChangesDescription = yourChangedFields
      .map((field) => {
        if (field === 'content') {
          return `Content: "${yourChanges.content}"`;
        } else if (field === 'properties') {
          return `Properties: ${JSON.stringify(yourChanges.properties)}`;
        }
        return `${field}: ${yourChanges[field as keyof Node]}`;
      })
      .join(', ');

    const currentChangesDescription = `Version ${currentNode.version} (last modified: ${currentNode.modifiedAt})`;

    return {
      useYours,
      useCurrent,
      yourChangesDescription,
      currentChangesDescription
    };
  }

  /**
   * Deep merge properties objects to prevent data loss
   *
   * Recursively merges two properties objects, with yourProperties taking
   * precedence for conflicting keys.
   *
   * Example:
   * Current: { status: "done", priority: "high" }
   * Yours: { assignee: "alice" }
   * Result: { status: "done", priority: "high", assignee: "alice" }
   *
   * @param currentProperties - Properties from current node
   * @param yourProperties - Properties you're trying to set
   * @returns Deeply merged properties object
   */
  private static deepMergeProperties(
    currentProperties: Record<string, unknown> | undefined,
    yourProperties: Record<string, unknown> | undefined
  ): Record<string, unknown> | undefined {
    // If neither has properties, return undefined
    if (!currentProperties && !yourProperties) {
      return undefined;
    }

    // If only one has properties, return that one
    if (!currentProperties) {
      return yourProperties;
    }
    if (!yourProperties) {
      return currentProperties;
    }

    // Deep merge both objects
    const merged: Record<string, unknown> = { ...currentProperties };

    for (const key in yourProperties) {
      const yourValue = yourProperties[key];
      const currentValue = merged[key];

      // If both values are objects (and not arrays/null), recursively merge
      if (this.isPlainObject(yourValue) && this.isPlainObject(currentValue)) {
        merged[key] = this.deepMergeProperties(
          currentValue as Record<string, unknown>,
          yourValue as Record<string, unknown>
        );
      } else {
        // Otherwise, your value takes precedence
        merged[key] = yourValue;
      }
    }

    return merged;
  }

  /**
   * Check if a value is a plain object (not array, null, Date, etc.)
   *
   * @param value - Value to check
   * @returns True if value is a plain object
   */
  private static isPlainObject(value: unknown): boolean {
    return (
      typeof value === 'object' &&
      value !== null &&
      !Array.isArray(value) &&
      Object.getPrototypeOf(value) === Object.prototype
    );
  }
}

export default ConflictResolver;
