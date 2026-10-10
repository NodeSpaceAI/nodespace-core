/**
 * Task node helpers.
 *
 * `TaskNode`, `TaskNodeUpdate`, `TaskStatus` and `Priority` are generated
 * from Rust's `nodespace-types` (`./generated`): the task schema's core fields
 * travel at the top level, not nested under `properties.task`.
 *
 * @example
 * ```typescript
 * import { TaskNode, isTaskNode, getTaskStatus, setTaskStatus } from '$lib/types/task-node';
 *
 * // Type guard
 * if (isTaskNode(node)) {
 *   const status = getTaskStatus(node); // Type-safe access
 *   console.log(`Task is ${status}`);
 * }
 *
 * // Immutable update
 * const updated = setTaskStatus(node, 'in_progress');
 * ```
 */

import type { Node } from './node';
import type { TaskNode, Priority, TaskStatus } from './generated';
import { isExactly } from './core-node-types';

export type { TaskNode, TaskNodeUpdate, Priority, TaskStatus } from './generated';

/**
 * Core task status values (protected, cannot be removed)
 * Users can extend with additional values via schema userValues
 */
export type CoreTaskStatus = 'open' | 'in_progress' | 'in_review' | 'done' | 'cancelled';

/**
 * Core task priority values (user-extensible)
 * Rust backend now uses string enum format
 */
export type CoreTaskPriority = 'highest' | 'high' | 'medium' | 'low' | 'lowest';

/**
 * Type guard to check if a node is a task node
 *
 * @param node - Node to check
 * @returns True if node is a task node
 */
export function isTaskNode(node: Node | TaskNode): node is TaskNode {
  return isExactly(node.nodeType, 'task');
}

/**
 * Get the task status
 *
 * TaskNode has flat structure with `status` at top level (from backend serialization).
 * See TaskNode interface documentation for structure details.
 *
 * @param node - Task node
 * @returns Task status (defaults to "open")
 */
export function getTaskStatus(node: TaskNode): TaskStatus {
  return node.status ?? 'open';
}

/**
 * Set the task status (immutable)
 *
 * @param node - Task node
 * @param status - New status value
 * @returns New node with updated status
 */
export function setTaskStatus(node: TaskNode, status: TaskStatus): TaskNode {
  return {
    ...node,
    status
  };
}

/**
 * Get the task priority
 *
 * @param node - Task node
 * @returns Task priority or undefined if not set
 */
export function getTaskPriority(node: TaskNode): Priority | undefined {
  return node.priority;
}

/**
 * Set the task priority (immutable)
 *
 * @param node - Task node
 * @param priority - New priority value
 * @returns New node with updated priority
 */
export function setTaskPriority(node: TaskNode, priority: Priority): TaskNode {
  return {
    ...node,
    priority
  };
}

/**
 * Get the task due date
 *
 * @param node - Task node
 * @returns Due date string or undefined if not set
 */
export function getTaskDueDate(node: TaskNode): string | undefined {
  return node.dueDate ?? undefined;
}

/**
 * Set the task due date (immutable)
 *
 * @param node - Task node
 * @param dueDate - Due date string (ISO 8601) or undefined to clear
 * @returns New node with updated due date
 */
export function setTaskDueDate(node: TaskNode, dueDate: string | undefined): TaskNode {
  return {
    ...node,
    dueDate
  };
}

/**
 * Convert a generic Node to a TaskNode by extracting type-specific fields from properties
 *
 * The backend (`node_to_typed_value` in `nodespace-types`) is the single typing
 * authority: for every transport (Tauri IPC and HTTP/SSE) it promotes task fields
 * to the TOP LEVEL of the node and flattens the `properties.task` namespace away.
 * See the `wire_contract` tests in `nodespace-types/src/convert.rs`. This converter
 * therefore trusts the flat contract and only fills defaults for missing fields.
 * Every other node field (`properties`, `title`, `lifecycleStatus`, …) is kept.
 *
 * @param node - Generic Node carrying a task (fields promoted to top level)
 * @returns TaskNode with flat type-specific fields
 */
export function nodeToTaskNode(node: Node): TaskNode {
  const task = node as Node & Partial<TaskNode>;
  // Every typed key is written, set or not, so a merge over an older copy of
  // the node clears a field the backend no longer carries.
  return {
    ...task,
    nodeType: 'task',
    properties: node.properties ?? {},
    status: task.status ?? 'open',
    priority: task.priority,
    dueDate: task.dueDate,
    startedAt: task.startedAt,
    completedAt: task.completedAt,
    requiresSpec: task.requiresSpec ?? true
  };
}

/**
 * Helper namespace for task node operations
 */
export const TaskNodeHelpers = {
  isTaskNode,
  getTaskStatus,
  setTaskStatus,
  getTaskPriority,
  setTaskPriority,
  getTaskDueDate,
  setTaskDueDate,
  nodeToTaskNode,

  /**
   * Check if task is completed (done or cancelled)
   */
  isCompleted(node: TaskNode): boolean {
    return node.status === 'done' || node.status === 'cancelled';
  },

  /**
   * Check if task is active (in_progress)
   */
  isActive(node: TaskNode): boolean {
    return node.status === 'in_progress';
  },

  /**
   * Check if task is pending (open)
   */
  isPending(node: TaskNode): boolean {
    return node.status === 'open';
  },

  /**
   * Check if status is a core (protected) status
   */
  isCoreStatus(status: TaskStatus): status is CoreTaskStatus {
    return ['open', 'in_progress', 'in_review', 'done', 'cancelled'].includes(status as string);
  },

  /**
   * Check if priority is a core (protected) priority
   */
  isCorePriority(priority: Priority): priority is CoreTaskPriority {
    return ['highest', 'high', 'medium', 'low', 'lowest'].includes(priority as string);
  },

  /**
   * Get display-friendly status name
   */
  getStatusDisplayName(status: TaskStatus): string {
    const coreDisplayNames: Record<CoreTaskStatus, string> = {
      open: 'Open',
      in_progress: 'In Progress',
      in_review: 'In Review',
      done: 'Done',
      cancelled: 'Cancelled'
    };

    if (this.isCoreStatus(status)) {
      return coreDisplayNames[status];
    }

    // Format user-defined status: replace underscores, capitalize words
    return String(status)
      .split('_')
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
      .join(' ');
  },

  /**
   * Get display-friendly priority name
   */
  getPriorityDisplayName(priority: Priority): string {
    const coreDisplayNames: Record<CoreTaskPriority, string> = {
      highest: 'Highest',
      high: 'High',
      medium: 'Medium',
      low: 'Low',
      lowest: 'Lowest'
    };

    if (this.isCorePriority(priority)) {
      return coreDisplayNames[priority];
    }

    // Format user-defined priority: replace underscores, capitalize words
    return String(priority)
      .split('_')
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
      .join(' ');
  },

  /**
   * Create a new task node with specified content
   *
   * @param content - The task content/description
   * @param options - Optional task properties
   * @returns New task node
   */
  createTaskNode(
    content: string,
    options: {
      status?: TaskStatus;
      priority?: Priority;
      dueDate?: string;
    } = {}
  ): TaskNode {
    return {
      lifecycleStatus: 'active',
      id: `task-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
      nodeType: 'task',
      content,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      status: options.status ?? 'open',
      priority: options.priority,
      dueDate: options.dueDate,
      requiresSpec: true
    };
  }
};
