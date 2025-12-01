/**
 * Tests for TaskNode Type-Safe Wrapper
 *
 * TaskNode uses flat structure matching Rust backend serialization (Issue #673, #700).
 * Spoke fields (status, priority, dueDate, assignee) are at the top level, not nested.
 */

import { describe, it, expect } from 'vitest';
import type { Node } from '$lib/types/node';
import {
  type TaskNode,
  isTaskNode,
  getTaskStatus,
  setTaskStatus,
  getTaskPriority,
  setTaskPriority,
  getTaskDueDate,
  setTaskDueDate,
  getTaskAssignee,
  setTaskAssignee,
  TaskNodeHelpers
} from '$lib/types/task-node';

describe('TaskNode Type Guard', () => {
  it('identifies task nodes correctly', () => {
    const taskNode: TaskNode = {
      id: 'test-1',
      nodeType: 'task',
      content: 'Fix the bug',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open'
    };

    expect(isTaskNode(taskNode)).toBe(true);
  });

  it('rejects non-task nodes', () => {
    const textNode: Node = {
      id: 'test-2',
      nodeType: 'text',
      content: 'Regular text',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };

    expect(isTaskNode(textNode)).toBe(false);
  });
});

describe('getTaskStatus', () => {
  it('returns status from flat structure', () => {
    const node: TaskNode = {
      id: 'test-3',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'in_progress'
    };

    expect(getTaskStatus(node)).toBe('in_progress');
  });

  it('returns open as default when status is undefined', () => {
    const node = {
      id: 'test-4',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1
    } as TaskNode;

    expect(getTaskStatus(node)).toBe('open');
  });
});

describe('setTaskStatus', () => {
  it('sets status immutably', () => {
    const original: TaskNode = {
      id: 'test-5',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open'
    };

    const updated = setTaskStatus(original, 'done');

    // Original unchanged
    expect(original.status).toBe('open');
    // Updated has new status
    expect(updated.status).toBe('done');
    expect(updated.id).toBe(original.id);
    expect(updated.content).toBe(original.content);
  });
});

describe('getTaskPriority', () => {
  it('returns priority from flat structure', () => {
    const node: TaskNode = {
      id: 'test-6',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      priority: 'high'
    };

    expect(getTaskPriority(node)).toBe('high');
  });

  it('returns undefined when priority is not set', () => {
    const node: TaskNode = {
      id: 'test-7',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open'
    };

    expect(getTaskPriority(node)).toBeUndefined();
  });

  it('handles string priority', () => {
    const node: TaskNode = {
      id: 'test-8',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      priority: 'medium'
    };

    expect(getTaskPriority(node)).toBe('medium');
  });
});

describe('setTaskPriority', () => {
  it('sets priority immutably', () => {
    const original: TaskNode = {
      id: 'test-9',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      priority: 'low'
    };

    const updated = setTaskPriority(original, 'high');

    expect(original.priority).toBe('low');
    expect(updated.priority).toBe('high');
  });
});

describe('getTaskDueDate', () => {
  it('returns dueDate from flat structure', () => {
    const node: TaskNode = {
      id: 'test-10',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      dueDate: '2025-12-31'
    };

    expect(getTaskDueDate(node)).toBe('2025-12-31');
  });

  it('returns undefined when dueDate is null', () => {
    const node: TaskNode = {
      id: 'test-11',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      dueDate: null
    };

    expect(getTaskDueDate(node)).toBeUndefined();
  });
});

describe('setTaskDueDate', () => {
  it('sets dueDate immutably', () => {
    const original: TaskNode = {
      id: 'test-12',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open'
    };

    const updated = setTaskDueDate(original, '2025-06-15');

    expect(original.dueDate).toBeUndefined();
    expect(updated.dueDate).toBe('2025-06-15');
  });

  it('clears dueDate when set to undefined', () => {
    const original: TaskNode = {
      id: 'test-13',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      dueDate: '2025-06-15'
    };

    const updated = setTaskDueDate(original, undefined);

    expect(updated.dueDate).toBeNull();
  });
});

describe('getTaskAssignee', () => {
  it('returns assignee from flat structure', () => {
    const node: TaskNode = {
      id: 'test-14',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      assignee: 'user-123'
    };

    expect(getTaskAssignee(node)).toBe('user-123');
  });

  it('returns undefined when assignee is null', () => {
    const node: TaskNode = {
      id: 'test-15',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      assignee: null
    };

    expect(getTaskAssignee(node)).toBeUndefined();
  });
});

describe('setTaskAssignee', () => {
  it('sets assignee immutably', () => {
    const original: TaskNode = {
      id: 'test-16',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open'
    };

    const updated = setTaskAssignee(original, 'user-456');

    expect(original.assignee).toBeUndefined();
    expect(updated.assignee).toBe('user-456');
  });

  it('clears assignee when set to undefined', () => {
    const original: TaskNode = {
      id: 'test-17',
      nodeType: 'task',
      content: 'Test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      assignee: 'user-456'
    };

    const updated = setTaskAssignee(original, undefined);

    expect(updated.assignee).toBeNull();
  });
});

describe('TaskNodeHelpers', () => {
  describe('isCompleted', () => {
    it('returns true for done status', () => {
      const node: TaskNode = {
        id: 'test-18',
        nodeType: 'task',
        content: 'Test task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status: 'done'
      };

      expect(TaskNodeHelpers.isCompleted(node)).toBe(true);
    });

    it('returns true for cancelled status', () => {
      const node: TaskNode = {
        id: 'test-19',
        nodeType: 'task',
        content: 'Test task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status: 'cancelled'
      };

      expect(TaskNodeHelpers.isCompleted(node)).toBe(true);
    });

    it('returns false for open status', () => {
      const node: TaskNode = {
        id: 'test-20',
        nodeType: 'task',
        content: 'Test task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status: 'open'
      };

      expect(TaskNodeHelpers.isCompleted(node)).toBe(false);
    });
  });

  describe('isActive', () => {
    it('returns true for in_progress status', () => {
      const node: TaskNode = {
        id: 'test-21',
        nodeType: 'task',
        content: 'Test task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status: 'in_progress'
      };

      expect(TaskNodeHelpers.isActive(node)).toBe(true);
    });

    it('returns false for other statuses', () => {
      const node: TaskNode = {
        id: 'test-22',
        nodeType: 'task',
        content: 'Test task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status: 'open'
      };

      expect(TaskNodeHelpers.isActive(node)).toBe(false);
    });
  });

  describe('isPending', () => {
    it('returns true for open status', () => {
      const node: TaskNode = {
        id: 'test-23',
        nodeType: 'task',
        content: 'Test task',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status: 'open'
      };

      expect(TaskNodeHelpers.isPending(node)).toBe(true);
    });
  });

  describe('isCoreStatus', () => {
    it('identifies core statuses', () => {
      expect(TaskNodeHelpers.isCoreStatus('open')).toBe(true);
      expect(TaskNodeHelpers.isCoreStatus('in_progress')).toBe(true);
      expect(TaskNodeHelpers.isCoreStatus('done')).toBe(true);
      expect(TaskNodeHelpers.isCoreStatus('cancelled')).toBe(true);
    });

    it('rejects non-core statuses', () => {
      expect(TaskNodeHelpers.isCoreStatus('blocked')).toBe(false);
      expect(TaskNodeHelpers.isCoreStatus('custom_status')).toBe(false);
    });
  });

  describe('isCorePriority', () => {
    it('identifies core priorities', () => {
      expect(TaskNodeHelpers.isCorePriority('low')).toBe(true);
      expect(TaskNodeHelpers.isCorePriority('medium')).toBe(true);
      expect(TaskNodeHelpers.isCorePriority('high')).toBe(true);
    });

    it('rejects non-core priorities', () => {
      expect(TaskNodeHelpers.isCorePriority('critical')).toBe(false);
      expect(TaskNodeHelpers.isCorePriority('urgent')).toBe(false);
    });
  });

  describe('getStatusDisplayName', () => {
    it('returns display names for core statuses', () => {
      expect(TaskNodeHelpers.getStatusDisplayName('open')).toBe('Open');
      expect(TaskNodeHelpers.getStatusDisplayName('in_progress')).toBe('In Progress');
      expect(TaskNodeHelpers.getStatusDisplayName('done')).toBe('Done');
      expect(TaskNodeHelpers.getStatusDisplayName('cancelled')).toBe('Cancelled');
    });

    it('formats user-defined statuses', () => {
      expect(TaskNodeHelpers.getStatusDisplayName('blocked_by_external')).toBe(
        'Blocked By External'
      );
      expect(TaskNodeHelpers.getStatusDisplayName('needs_review')).toBe('Needs Review');
    });
  });

  describe('getPriorityDisplayName', () => {
    it('returns display names for core priorities', () => {
      expect(TaskNodeHelpers.getPriorityDisplayName('low')).toBe('Low');
      expect(TaskNodeHelpers.getPriorityDisplayName('medium')).toBe('Medium');
      expect(TaskNodeHelpers.getPriorityDisplayName('high')).toBe('High');
    });

    it('formats user-defined priorities', () => {
      expect(TaskNodeHelpers.getPriorityDisplayName('critical')).toBe('Critical');
      expect(TaskNodeHelpers.getPriorityDisplayName('urgent')).toBe('Urgent');
      expect(TaskNodeHelpers.getPriorityDisplayName('very_high')).toBe('Very High');
    });
  });

  describe('createTaskNode', () => {
    it('creates a task node with default values', () => {
      const task = TaskNodeHelpers.createTaskNode('My new task');

      expect(task.content).toBe('My new task');
      expect(task.nodeType).toBe('task');
      expect(task.status).toBe('open');
      expect(task.priority).toBeUndefined();
      expect(task.dueDate).toBeNull();
      expect(task.assignee).toBeNull();
      expect(task.id).toMatch(/^task-/);
    });

    it('creates a task node with custom options', () => {
      const task = TaskNodeHelpers.createTaskNode('Important task', {
        status: 'in_progress',
        priority: 'high',
        dueDate: '2025-12-31',
        assignee: 'user-123'
      });

      expect(task.content).toBe('Important task');
      expect(task.status).toBe('in_progress');
      expect(task.priority).toBe('high');
      expect(task.dueDate).toBe('2025-12-31');
      expect(task.assignee).toBe('user-123');
    });
  });
});

describe('Integration', () => {
  it('works with type guard and helpers together', () => {
    const node: TaskNode = {
      id: 'integration-test',
      nodeType: 'task',
      content: 'Integration test task',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      status: 'open',
      priority: 'medium'
    };

    if (isTaskNode(node)) {
      // Type guard should work
      const status = getTaskStatus(node);
      expect(status).toBe('open');

      // Immutable updates should chain
      const updated = setTaskStatus(setTaskPriority(node, 'high'), 'in_progress');
      expect(updated.status).toBe('in_progress');
      expect(updated.priority).toBe('high');

      // Original should be unchanged
      expect(node.status).toBe('open');
      expect(node.priority).toBe('medium');
    }
  });

  it('handles various task scenarios', () => {
    const scenarios = [
      { status: 'open', priority: 'low', expectedComplete: false },
      { status: 'in_progress', priority: 'high', expectedComplete: false },
      { status: 'done', priority: 'medium', expectedComplete: true },
      { status: 'cancelled', priority: undefined, expectedComplete: true }
    ];

    scenarios.forEach(({ status, priority, expectedComplete }) => {
      const node: TaskNode = {
        id: `test-${status}`,
        nodeType: 'task',
        content: `Task with ${status}`,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        status,
        priority
      };

      expect(TaskNodeHelpers.isCompleted(node)).toBe(expectedComplete);
      expect(getTaskStatus(node)).toBe(status);
      expect(getTaskPriority(node)).toBe(priority);
    });
  });
});
