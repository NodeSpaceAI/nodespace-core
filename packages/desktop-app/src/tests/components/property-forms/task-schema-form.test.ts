/**
 * Tests for TaskSchemaForm Component Logic (Issue #709)
 *
 * Tests the utility functions, data transformations, and business logic
 * used by TaskSchemaForm. UI rendering tests would require browser context.
 */

import { describe, it, expect } from 'vitest';
import type { TaskNode, TaskStatus } from '$lib/types/task-node';
import type { EnumValue } from '$lib/types/schema-node';

// ============================================================================
// Test Fixtures
// ============================================================================

/**
 * Minimal schema field type for testing - excludes required fields that
 * aren't used by the utility functions being tested
 */
interface TestSchemaField {
  name: string;
  type: string;
  required?: boolean;
  coreValues?: EnumValue[];
  userValues?: EnumValue[];
  default?: unknown;
  description?: string;
}

function createTestTaskNode(overrides: Partial<TaskNode> = {}): TaskNode {
  return {
    id: 'task-test-1',
    nodeType: 'task',
    content: 'Test task content',
    version: 1,
    createdAt: '2025-01-01T00:00:00Z',
    modifiedAt: '2025-01-01T00:00:00Z',
    status: 'open',
    ...overrides
  };
}

interface TestSchemaNode {
  id: string;
  nodeType: 'schema';
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  targetNodeType: string;
  fields: TestSchemaField[];
}

function createTestSchema(fields: TestSchemaField[] = []): TestSchemaNode {
  return {
    id: 'schema-task',
    nodeType: 'schema',
    content: 'task',
    version: 1,
    createdAt: '2025-01-01T00:00:00Z',
    modifiedAt: '2025-01-01T00:00:00Z',
    targetNodeType: 'task',
    fields: [
      // Core task fields
      {
        name: 'status',
        type: 'enum',
        required: true,
        coreValues: [
          { value: 'open', label: 'Open' },
          { value: 'in_progress', label: 'In Progress' },
          { value: 'done', label: 'Done' },
          { value: 'cancelled', label: 'Cancelled' }
        ]
      },
      {
        name: 'priority',
        type: 'enum',
        required: false,
        coreValues: [
          { value: 'low', label: 'Low' },
          { value: 'medium', label: 'Medium' },
          { value: 'high', label: 'High' }
        ]
      },
      {
        name: 'dueDate',
        type: 'date',
        required: false
      },
      {
        name: 'assignee',
        type: 'string',
        required: false
      },
      ...fields
    ]
  };
}

// ============================================================================
// Utility Functions (Extracted from TaskSchemaForm for testing)
// ============================================================================

const CORE_FIELD_NAMES = ['status', 'priority', 'dueDate', 'due_date', 'assignee'];

const STATUS_OPTIONS: Array<{ value: TaskStatus; label: string }> = [
  { value: 'open', label: 'Open' },
  { value: 'in_progress', label: 'In Progress' },
  { value: 'done', label: 'Done' },
  { value: 'cancelled', label: 'Cancelled' }
];

const PRIORITY_OPTIONS: Array<{ value: string; label: string }> = [
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' }
];

function getStatusOptionsWithExtensions(
  schema: TestSchemaNode | null
): Array<{ value: TaskStatus; label: string }> {
  const options = [...STATUS_OPTIONS];

  if (schema) {
    const statusField = schema.fields.find((f) => f.name === 'status');
    if (statusField?.userValues) {
      const coreValues = new Set(STATUS_OPTIONS.map((o) => o.value));
      for (const uv of statusField.userValues) {
        if (!coreValues.has(uv.value as TaskStatus)) {
          options.push({ value: uv.value as TaskStatus, label: uv.label });
        }
      }
    }
  }

  return options;
}

function getPriorityOptionsWithExtensions(
  schema: TestSchemaNode | null
): Array<{ value: string; label: string }> {
  const options = [...PRIORITY_OPTIONS];

  if (schema) {
    const priorityField = schema.fields.find((f) => f.name === 'priority');
    if (priorityField?.userValues) {
      const coreValues = new Set(PRIORITY_OPTIONS.map((o) => o.value));
      for (const uv of priorityField.userValues) {
        if (!coreValues.has(uv.value)) {
          options.push({ value: uv.value, label: uv.label });
        }
      }
    }
  }

  return options;
}

function getUserDefinedFields(schema: TestSchemaNode | null): TestSchemaField[] {
  if (!schema) return [];
  return schema.fields.filter((f) => !CORE_FIELD_NAMES.includes(f.name));
}

function calculateFieldStats(
  node: TaskNode | null,
  userFields: TestSchemaField[],
  getUserFieldValue: (fieldName: string) => unknown
): { filled: number; total: number } {
  if (!node) return { filled: 0, total: 4 };

  let filled = 0;
  const total = 4 + userFields.length; // 4 core fields + user fields

  // Core fields
  if (node.status) filled++;
  if (node.priority !== undefined && node.priority !== null) filled++;
  if (node.dueDate) filled++;
  if (node.assignee) filled++;

  // User-defined fields
  for (const field of userFields) {
    const value = getUserFieldValue(field.name);
    if (value !== null && value !== undefined && value !== '') {
      filled++;
    }
  }

  return { filled, total };
}

function getEnumValues(field: TestSchemaField): EnumValue[] {
  const values: EnumValue[] = [];
  if (field.coreValues) values.push(...field.coreValues);
  if (field.userValues) values.push(...field.userValues);
  return values;
}

function formatEnumLabel(value: string): string {
  return value
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
}

function formatDateDisplay(value: string | null | undefined): string {
  if (!value) return 'Pick a date';
  return value; // Simplified for testing - actual component uses parseDate
}

function formatDateForStorage(
  value: { year: number; month: number; day: number } | undefined
): string | null {
  if (!value) return null;
  return `${value.year}-${String(value.month).padStart(2, '0')}-${String(value.day).padStart(2, '0')}`;
}

// ============================================================================
// Tests
// ============================================================================

describe('TaskSchemaForm Utility Functions', () => {
  describe('getStatusOptionsWithExtensions', () => {
    it('returns core status options when no schema', () => {
      const options = getStatusOptionsWithExtensions(null);

      expect(options).toHaveLength(4);
      expect(options.map((o) => o.value)).toEqual(['open', 'in_progress', 'done', 'cancelled']);
    });

    it('returns core options when schema has no user values', () => {
      const schema = createTestSchema();
      const options = getStatusOptionsWithExtensions(schema);

      expect(options).toHaveLength(4);
    });

    it('adds user-defined status values from schema', () => {
      const schema = createTestSchema();
      const statusField = schema.fields.find((f) => f.name === 'status');
      if (statusField) {
        statusField.userValues = [
          { value: 'blocked', label: 'Blocked' },
          { value: 'review', label: 'In Review' }
        ];
      }

      const options = getStatusOptionsWithExtensions(schema);

      expect(options).toHaveLength(6);
      expect(options.map((o) => o.value)).toContain('blocked');
      expect(options.map((o) => o.value)).toContain('review');
    });

    it('does not duplicate core values from userValues', () => {
      const schema = createTestSchema();
      const statusField = schema.fields.find((f) => f.name === 'status');
      if (statusField) {
        // Try to add 'open' again via userValues - should be ignored
        statusField.userValues = [
          { value: 'open', label: 'Open (duplicate)' },
          { value: 'blocked', label: 'Blocked' }
        ];
      }

      const options = getStatusOptionsWithExtensions(schema);

      // Should only have 5 options (4 core + 1 new user value)
      expect(options).toHaveLength(5);
      const openOptions = options.filter((o) => o.value === 'open');
      expect(openOptions).toHaveLength(1);
      expect(openOptions[0].label).toBe('Open'); // Original label preserved
    });
  });

  describe('getPriorityOptionsWithExtensions', () => {
    it('returns core priority options when no schema', () => {
      const options = getPriorityOptionsWithExtensions(null);

      expect(options).toHaveLength(3);
      expect(options.map((o) => o.value)).toEqual(['low', 'medium', 'high']);
    });

    it('adds user-defined priority values from schema', () => {
      const schema = createTestSchema();
      const priorityField = schema.fields.find((f) => f.name === 'priority');
      if (priorityField) {
        priorityField.userValues = [
          { value: 'critical', label: 'Critical' },
          { value: 'urgent', label: 'Urgent' }
        ];
      }

      const options = getPriorityOptionsWithExtensions(schema);

      expect(options).toHaveLength(5);
      expect(options.map((o) => o.value)).toContain('critical');
      expect(options.map((o) => o.value)).toContain('urgent');
    });
  });

  describe('getUserDefinedFields', () => {
    it('returns empty array when no schema', () => {
      const fields = getUserDefinedFields(null);
      expect(fields).toEqual([]);
    });

    it('filters out core fields', () => {
      const schema = createTestSchema([
        { name: 'custom_field', type: 'text', required: false },
        { name: 'estimate', type: 'number', required: false }
      ]);

      const fields = getUserDefinedFields(schema);

      expect(fields).toHaveLength(2);
      expect(fields.map((f) => f.name)).toEqual(['custom_field', 'estimate']);
    });

    it('includes fields with user-defined names', () => {
      const schema = createTestSchema([
        { name: 'sprint', type: 'enum', required: false },
        { name: 'story_points', type: 'number', required: false },
        { name: 'due_date', type: 'date', required: false } // Should be filtered (core)
      ]);

      const fields = getUserDefinedFields(schema);

      // due_date is a core field name, should be filtered
      expect(fields).toHaveLength(2);
      expect(fields.map((f) => f.name)).not.toContain('due_date');
    });
  });

  describe('calculateFieldStats', () => {
    it('returns zeros when node is null', () => {
      const stats = calculateFieldStats(null, [], () => undefined);
      expect(stats).toEqual({ filled: 0, total: 4 });
    });

    it('counts filled core fields correctly', () => {
      const node = createTestTaskNode({
        status: 'open',
        priority: 'high',
        dueDate: '2025-12-31',
        assignee: null
      });

      const stats = calculateFieldStats(node, [], () => undefined);

      expect(stats.filled).toBe(3); // status, priority, dueDate
      expect(stats.total).toBe(4);
    });

    it('includes user-defined fields in total', () => {
      const node = createTestTaskNode();
      const userFields: TestSchemaField[] = [
        { name: 'estimate', type: 'number', required: false },
        { name: 'sprint', type: 'text', required: false }
      ];

      const stats = calculateFieldStats(node, userFields, () => undefined);

      expect(stats.total).toBe(6); // 4 core + 2 user
    });

    it('counts filled user-defined fields', () => {
      const node = createTestTaskNode({ status: 'open' });
      const userFields: TestSchemaField[] = [
        { name: 'estimate', type: 'number', required: false },
        { name: 'sprint', type: 'text', required: false }
      ];

      const getUserFieldValue = (name: string) => {
        if (name === 'estimate') return 5;
        return undefined;
      };

      const stats = calculateFieldStats(node, userFields, getUserFieldValue);

      expect(stats.filled).toBe(2); // status + estimate
      expect(stats.total).toBe(6);
    });

    it('treats empty string as unfilled', () => {
      const node = createTestTaskNode();
      const userFields: TestSchemaField[] = [{ name: 'notes', type: 'text', required: false }];

      const stats = calculateFieldStats(node, userFields, () => '');

      // Only status counts as filled (from core fields)
      expect(stats.filled).toBe(1);
    });
  });

  describe('getEnumValues', () => {
    it('returns empty array when no values', () => {
      const field: TestSchemaField = { name: 'test', type: 'enum', required: false };
      const values = getEnumValues(field);
      expect(values).toEqual([]);
    });

    it('returns core values only', () => {
      const field: TestSchemaField = {
        name: 'status',
        type: 'enum',
        required: true,
        coreValues: [
          { value: 'open', label: 'Open' },
          { value: 'done', label: 'Done' }
        ]
      };

      const values = getEnumValues(field);

      expect(values).toHaveLength(2);
      expect(values.map((v) => v.value)).toEqual(['open', 'done']);
    });

    it('combines core and user values', () => {
      const field: TestSchemaField = {
        name: 'status',
        type: 'enum',
        required: true,
        coreValues: [{ value: 'open', label: 'Open' }],
        userValues: [{ value: 'blocked', label: 'Blocked' }]
      };

      const values = getEnumValues(field);

      expect(values).toHaveLength(2);
      expect(values.map((v) => v.value)).toEqual(['open', 'blocked']);
    });
  });

  describe('formatEnumLabel', () => {
    it('capitalizes single word', () => {
      expect(formatEnumLabel('open')).toBe('Open');
      expect(formatEnumLabel('high')).toBe('High');
    });

    it('handles snake_case', () => {
      expect(formatEnumLabel('in_progress')).toBe('In Progress');
      expect(formatEnumLabel('needs_review')).toBe('Needs Review');
    });

    it('handles multi-word snake_case', () => {
      expect(formatEnumLabel('blocked_by_external')).toBe('Blocked By External');
    });

    it('handles already capitalized words', () => {
      expect(formatEnumLabel('TODO')).toBe('Todo');
    });
  });

  describe('formatDateDisplay', () => {
    it('returns placeholder for null/undefined', () => {
      expect(formatDateDisplay(null)).toBe('Pick a date');
      expect(formatDateDisplay(undefined)).toBe('Pick a date');
    });

    it('returns date string as-is', () => {
      expect(formatDateDisplay('2025-12-31')).toBe('2025-12-31');
    });
  });

  describe('formatDateForStorage', () => {
    it('returns null for undefined', () => {
      expect(formatDateForStorage(undefined)).toBeNull();
    });

    it('formats date object to ISO string', () => {
      const date = { year: 2025, month: 6, day: 15 };
      expect(formatDateForStorage(date)).toBe('2025-06-15');
    });

    it('pads single digit months and days', () => {
      const date = { year: 2025, month: 1, day: 5 };
      expect(formatDateForStorage(date)).toBe('2025-01-05');
    });
  });
});

describe('TaskSchemaForm Core Field Constants', () => {
  it('has correct core field names', () => {
    expect(CORE_FIELD_NAMES).toContain('status');
    expect(CORE_FIELD_NAMES).toContain('priority');
    expect(CORE_FIELD_NAMES).toContain('dueDate');
    expect(CORE_FIELD_NAMES).toContain('due_date'); // Both formats for compatibility
    expect(CORE_FIELD_NAMES).toContain('assignee');
  });

  it('has correct status options', () => {
    expect(STATUS_OPTIONS).toHaveLength(4);

    const values = STATUS_OPTIONS.map((o) => o.value);
    expect(values).toContain('open');
    expect(values).toContain('in_progress');
    expect(values).toContain('done');
    expect(values).toContain('cancelled');
  });

  it('has correct priority options', () => {
    expect(PRIORITY_OPTIONS).toHaveLength(3);

    const values = PRIORITY_OPTIONS.map((o) => o.value);
    expect(values).toContain('low');
    expect(values).toContain('medium');
    expect(values).toContain('high');
  });
});

describe('TaskSchemaForm Integration Scenarios', () => {
  it('correctly identifies all core task fields', () => {
    const schema = createTestSchema();
    const coreFields = schema.fields.filter((f) => CORE_FIELD_NAMES.includes(f.name));
    const userFields = getUserDefinedFields(schema);

    expect(coreFields.length).toBeGreaterThan(0);
    expect(userFields).toHaveLength(0);
  });

  it('handles task with all fields filled', () => {
    const node = createTestTaskNode({
      status: 'in_progress',
      priority: 'high',
      dueDate: '2025-12-31',
      assignee: 'user-123'
    });

    const stats = calculateFieldStats(node, [], () => undefined);

    expect(stats.filled).toBe(4);
    expect(stats.total).toBe(4);
  });

  it('handles minimal task node', () => {
    const node = createTestTaskNode({
      status: 'open',
      priority: undefined,
      dueDate: undefined,
      assignee: undefined
    });

    const stats = calculateFieldStats(node, [], () => undefined);

    expect(stats.filled).toBe(1); // Only status
    expect(stats.total).toBe(4);
  });

  it('schema with user extensions provides complete options', () => {
    const schema = createTestSchema();

    // Add user-defined status and priority
    const statusField = schema.fields.find((f) => f.name === 'status');
    const priorityField = schema.fields.find((f) => f.name === 'priority');

    if (statusField) {
      statusField.userValues = [{ value: 'blocked', label: 'Blocked' }];
    }
    if (priorityField) {
      priorityField.userValues = [{ value: 'critical', label: 'Critical' }];
    }

    const statusOptions = getStatusOptionsWithExtensions(schema);
    const priorityOptions = getPriorityOptionsWithExtensions(schema);

    // Core + user values
    expect(statusOptions).toHaveLength(5);
    expect(priorityOptions).toHaveLength(4);

    // User values are at the end
    expect(statusOptions[statusOptions.length - 1].value).toBe('blocked');
    expect(priorityOptions[priorityOptions.length - 1].value).toBe('critical');
  });
});
