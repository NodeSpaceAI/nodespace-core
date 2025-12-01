/**
 * Type-Safe Task Node Interface
 *
 * Flat structure matching Rust backend serialization (Issue #673, #700).
 *
 * The Rust backend serializes TaskNode as a flat JSON structure with spoke fields
 * at the top level, not nested under properties.task. This interface matches that output.
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

/**
 * Core task status values (protected, cannot be removed)
 * Users can extend with additional values via schema userValues
 */
export type CoreTaskStatus = 'open' | 'in_progress' | 'done' | 'cancelled';

/**
 * Task status - core values plus any user-defined extensions
 * Use CoreTaskStatus when you need to check against known values
 */
export type TaskStatus = CoreTaskStatus | string;

/**
 * Core task priority values (user-extensible)
 * Note: Rust backend uses integer priority (1-4), but string format is also supported
 */
export type CoreTaskPriority = 'low' | 'medium' | 'high';

/**
 * Task priority - supports both integer (1-4) and string formats
 */
export type TaskPriority = CoreTaskPriority | string | number;

/**
 * TaskNode - Flat structure matching Rust backend serialization
 *
 * When the backend serializes a TaskNode, it produces a flat JSON structure:
 * ```json
 * {
 *   "id": "task-123",
 *   "nodeType": "task",
 *   "content": "Fix the bug",
 *   "version": 1,
 *   "createdAt": "2025-01-01T00:00:00Z",
 *   "modifiedAt": "2025-01-01T00:00:00Z",
 *   "status": "open",
 *   "priority": 2,
 *   "dueDate": null,
 *   "assignee": null
 * }
 * ```
 */
export interface TaskNode {
  // Hub fields (from node table)
  id: string;
  nodeType: 'task';
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;

  // Spoke fields (flat, at top level)
  status: TaskStatus;
  priority?: TaskPriority;
  dueDate?: string | null;
  assignee?: string | null;
}

/**
 * Type guard to check if a node is a task node
 *
 * @param node - Node to check
 * @returns True if node is a task node
 */
export function isTaskNode(node: Node | TaskNode): node is TaskNode {
  return node.nodeType === 'task';
}

/**
 * Get the task status
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
export function getTaskPriority(node: TaskNode): TaskPriority | undefined {
  return node.priority;
}

/**
 * Set the task priority (immutable)
 *
 * @param node - Task node
 * @param priority - New priority value
 * @returns New node with updated priority
 */
export function setTaskPriority(node: TaskNode, priority: TaskPriority): TaskNode {
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
    dueDate: dueDate ?? null
  };
}

/**
 * Get the task assignee
 *
 * @param node - Task node
 * @returns Assignee ID string or undefined if not set
 */
export function getTaskAssignee(node: TaskNode): string | undefined {
  return node.assignee ?? undefined;
}

/**
 * Set the task assignee (immutable)
 *
 * @param node - Task node
 * @param assignee - Assignee ID string or undefined to clear
 * @returns New node with updated assignee
 */
export function setTaskAssignee(node: TaskNode, assignee: string | undefined): TaskNode {
  return {
    ...node,
    assignee: assignee ?? null
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
  getTaskAssignee,
  setTaskAssignee,

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
    return ['open', 'in_progress', 'done', 'cancelled'].includes(status as string);
  },

  /**
   * Check if priority is a core (protected) priority
   */
  isCorePriority(priority: TaskPriority): priority is CoreTaskPriority {
    return ['low', 'medium', 'high'].includes(priority as string);
  },

  /**
   * Get display-friendly status name
   */
  getStatusDisplayName(status: TaskStatus): string {
    const coreDisplayNames: Record<CoreTaskStatus, string> = {
      open: 'Open',
      in_progress: 'In Progress',
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
  getPriorityDisplayName(priority: TaskPriority): string {
    // Handle numeric priorities
    if (typeof priority === 'number') {
      const numericLabels: Record<number, string> = {
        1: 'Urgent',
        2: 'High',
        3: 'Medium',
        4: 'Low'
      };
      return numericLabels[priority] || `Priority ${priority}`;
    }

    const coreDisplayNames: Record<CoreTaskPriority, string> = {
      low: 'Low',
      medium: 'Medium',
      high: 'High'
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
      priority?: TaskPriority;
      dueDate?: string;
      assignee?: string;
    } = {}
  ): TaskNode {
    return {
      id: `task-${Date.now()}-${Math.random().toString(36).substring(2, 9)}`,
      nodeType: 'task',
      content,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: options.status ?? 'open',
      priority: options.priority,
      dueDate: options.dueDate ?? null,
      assignee: options.assignee ?? null
    };
  }
};
