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
 * Rust backend now uses string enum format (Issue #709)
 */
export type CoreTaskPriority = 'low' | 'medium' | 'high';

/**
 * Task priority - string enum format
 * Core values: 'low', 'medium', 'high'
 * User-defined values allowed via schema extension
 */
export type TaskPriority = CoreTaskPriority | string;

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
 *   "priority": "medium",
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
  startedAt?: string | null;
  completedAt?: string | null;
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
 * Partial update structure for task nodes
 *
 * Supports updating task-specific spoke fields (status, priority, dueDate, assignee)
 * and hub fields (content). All fields are optional - only include fields to update.
 *
 * This interface matches the Rust `TaskNodeUpdate` struct for type-safe CRUD operations.
 *
 * @example
 * ```typescript
 * // Update only status
 * const update: TaskNodeUpdate = { status: 'in_progress' };
 *
 * // Update status and clear due date
 * const update: TaskNodeUpdate = {
 *   status: 'done',
 *   dueDate: null  // Explicitly clear the field
 * };
 *
 * // Update content (hub field)
 * const update: TaskNodeUpdate = { content: 'Updated task description' };
 * ```
 */
export interface TaskNodeUpdate {
  /** Update task status (spoke field) */
  status?: TaskStatus;

  /** Update task priority (spoke field) - null to clear */
  priority?: TaskPriority | null;

  /** Update due date (spoke field) - null to clear */
  dueDate?: string | null;

  /** Update assignee (spoke field) - null to clear */
  assignee?: string | null;

  /** Update started_at date (spoke field) - null to clear */
  startedAt?: string | null;

  /** Update completed_at date (spoke field) - null to clear */
  completedAt?: string | null;

  /** Update content (hub field) */
  content?: string;
}

/**
 * Convert a generic Node to a TaskNode by extracting spoke fields from properties
 *
 * SSE events send generic Node objects where task-specific fields are stored in
 * `properties.status`, `properties.priority`, etc. This function normalizes them
 * to the flat TaskNode structure expected by the frontend.
 *
 * Handles both formats:
 * - Nested: `properties.task.status` (legacy)
 * - Flat: `properties.status` (current)
 * - Already flat: `node.status` (already a TaskNode)
 *
 * @param node - Generic Node with task data in properties
 * @returns TaskNode with flat spoke fields
 */
export function nodeToTaskNode(node: Node): TaskNode {
  // If already has flat status, it's already a TaskNode
  if ('status' in node && typeof (node as TaskNode).status === 'string') {
    return node as TaskNode;
  }

  // Extract from properties (try nested task.* first, then flat)
  // Per naming conventions: API returns camelCase (dueDate, startedAt, completedAt)
  const props = node.properties as Record<string, unknown> | undefined;
  const taskProps = (props?.task as Record<string, unknown>) ?? props ?? {};

  const status = (taskProps.status as TaskStatus) ?? 'open';
  const priority = taskProps.priority as TaskPriority | undefined;
  const dueDate = taskProps.dueDate as string | null | undefined;
  const assignee = taskProps.assignee as string | null | undefined;
  const startedAt = taskProps.startedAt as string | null | undefined;
  const completedAt = taskProps.completedAt as string | null | undefined;

  return {
    id: node.id,
    nodeType: 'task',
    content: node.content,
    version: node.version,
    createdAt: node.createdAt,
    modifiedAt: node.modifiedAt,
    status,
    priority,
    dueDate: dueDate ?? null,
    assignee: assignee ?? null,
    startedAt: startedAt ?? null,
    completedAt: completedAt ?? null
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
