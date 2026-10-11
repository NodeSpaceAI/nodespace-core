/**
 * Shared enum-field helpers: merging coreValues/userValues, and
 * resolving a stored value to its display label. Extracted out of
 * schema-field-leaf.svelte (the enum <Select> case) so TaskSchemaForm's
 * collapsed-header status badge can reuse the exact same lookup instead of
 * re-deriving its own copy — see task-schema-form.test.ts for the
 * component-level assertion that the header agrees with the field control.
 */
import { describe, it, expect } from 'vitest';
import type { SchemaField } from '$lib/types/schema-node';
import type { RawEdgeField } from '$lib/services/relationship-grouping';
import {
  getEnumValues,
  formatEnumFallbackLabel,
  enumValueLabel,
  enumValueAtScope
} from '$lib/utils/schema-enum-values';

function enumField(overrides: Partial<SchemaField> = {}): SchemaField {
  return {
    name: 'status',
    friendlyName: 'Status',
    type: 'enum',
    protection: 'user',
    indexed: true,
    ...overrides
  };
}

describe('getEnumValues', () => {
  it('returns an empty array when neither coreValues nor userValues are set', () => {
    expect(getEnumValues(enumField())).toEqual([]);
  });

  it('merges coreValues followed by userValues, in that order', () => {
    const field = enumField({
      coreValues: [{ value: 'open', label: 'Open' }],
      userValues: [{ value: 'blocked', label: 'Blocked' }]
    });
    expect(getEnumValues(field)).toEqual([
      { value: 'open', label: 'Open' },
      { value: 'blocked', label: 'Blocked' }
    ]);
  });

  it('tolerates an empty userValues array (the shape core_schemas.rs actually sends)', () => {
    const field = enumField({ coreValues: [{ value: 'open', label: 'Open' }], userValues: [] });
    expect(getEnumValues(field)).toEqual([{ value: 'open', label: 'Open' }]);
  });

  it('drops a userValue whose value collides with an existing coreValue, keeping the core label', () => {
    // Matches the original TaskSchemaForm behavior (statusOptionsWithExtensions'
    // `if (!coreValues.has(uv.value))` guard) — an extension adds choices, it
    // doesn't shadow one that already exists.
    const field = enumField({
      coreValues: [{ value: 'open', label: 'Open' }],
      userValues: [
        { value: 'open', label: 'Open (duplicate)' },
        { value: 'blocked', label: 'Blocked' }
      ]
    });
    const values = getEnumValues(field);
    expect(values).toEqual([
      { value: 'open', label: 'Open' },
      { value: 'blocked', label: 'Blocked' }
    ]);
  });

  it('drops a later userValue that collides with an earlier one, keeping the first', () => {
    const field = enumField({
      userValues: [
        { value: 'blocked', label: 'Blocked' },
        { value: 'blocked', label: 'Blocked (dup)' }
      ]
    });
    expect(getEnumValues(field)).toEqual([{ value: 'blocked', label: 'Blocked' }]);
  });
});

describe('formatEnumFallbackLabel', () => {
  it('capitalizes a single word', () => {
    expect(formatEnumFallbackLabel('open')).toBe('Open');
  });

  it('humanizes snake_case', () => {
    expect(formatEnumFallbackLabel('in_progress')).toBe('In Progress');
  });

  it('humanizes multi-word snake_case', () => {
    expect(formatEnumFallbackLabel('blocked_by_external')).toBe('Blocked By External');
  });

  it('normalizes an already-uppercase word', () => {
    expect(formatEnumFallbackLabel('TODO')).toBe('Todo');
  });
});

describe('enumValueLabel', () => {
  it('returns undefined for a null/undefined/empty value', () => {
    const field = enumField({ coreValues: [{ value: 'open', label: 'Open' }] });
    expect(enumValueLabel(field, null)).toBeUndefined();
    expect(enumValueLabel(field, undefined)).toBeUndefined();
    expect(enumValueLabel(field, '')).toBeUndefined();
  });

  it("resolves to the schema's declared label for a current option", () => {
    const field = enumField({ coreValues: [{ value: 'in_progress', label: 'In Progress' }] });
    expect(enumValueLabel(field, 'in_progress')).toBe('In Progress');
  });

  it('falls back to a humanized version of the raw value when the schema no longer declares it', () => {
    const field = enumField({ coreValues: [{ value: 'open', label: 'Open' }] });
    expect(enumValueLabel(field, 'archived_legacy')).toBe('Archived Legacy');
  });

  it('falls back to a humanized label when the matching option has a blank declared label', () => {
    // `||`, not `??`: a malformed `label: ''` is exactly as unhelpful as no match at
    // all — must not surface a blank where SchemaFieldLeaf's own Select trigger (which
    // now calls this same helper) would otherwise show something.
    const field = enumField({ coreValues: [{ value: 'in_progress', label: '' }] });
    expect(enumValueLabel(field, 'in_progress')).toBe('In Progress');
  });
});

describe('edge fields', () => {
  // An edge field is a closed vocabulary: it declares coreValues and has no
  // userValues/extensible half. These helpers were widened from SchemaField to
  // EnumFieldLike so the relationships modal's role picker resolves values
  // through the same lookup the node property forms use, rather than growing a
  // second copy that could drift on how a value renders.
  const roleEdgeField: RawEdgeField = {
    name: 'role',
    type: 'enum',
    coreValues: [
      { value: 'owner', label: 'Owner' },
      { value: 'editor', label: 'Editor' },
      { value: 'viewer', label: 'Viewer' }
    ]
  };

  it('returns an edge field\'s declared values in order', () => {
    expect(getEnumValues(roleEdgeField)).toEqual([
      { value: 'owner', label: 'Owner' },
      { value: 'editor', label: 'Editor' },
      { value: 'viewer', label: 'Viewer' }
    ]);
  });

  it('resolves an edge value to its declared label', () => {
    expect(enumValueLabel(roleEdgeField, 'owner')).toBe('Owner');
  });

  it('humanizes a stored edge value the schema no longer declares', () => {
    expect(enumValueLabel(roleEdgeField, 'legacy_admin')).toBe('Legacy Admin');
  });

  it('returns an empty list for an edge field with no declared values', () => {
    expect(getEnumValues({ name: 'role', type: 'enum' } as RawEdgeField)).toEqual([]);
  });
});

describe('enumValueAtScope', () => {
  // A task's status, and an issue's: the same field with two values added,
  // each mapped to a task status.
  const taskStatus = enumField({
    coreValues: [
      { value: 'open', label: 'Open' },
      { value: 'in_progress', label: 'In Progress' },
      { value: 'in_review', label: 'In Review' },
      { value: 'done', label: 'Done' }
    ],
    userValues: [{ value: 'waiting', label: 'Waiting' }]
  });
  const issueStatus = enumField({
    coreValues: [
      ...(taskStatus.coreValues ?? []),
      { value: 'changes_requested', label: 'Changes Requested', mapsTo: 'in_progress' },
      { value: 'ready_to_merge', label: 'Ready to Merge', mapsTo: 'in_review' }
    ],
    userValues: [{ value: 'qa', label: 'QA', mapsTo: 'ready_to_merge' }]
  });

  it('returns a value the scope declares as it is, core or user', () => {
    expect(enumValueAtScope(taskStatus, 'in_review', issueStatus)).toBe('in_review');
    expect(enumValueAtScope(taskStatus, 'waiting', undefined)).toBe('waiting');
  });

  it('maps a value the subtype added to the one it names', () => {
    expect(enumValueAtScope(taskStatus, 'changes_requested', issueStatus)).toBe('in_progress');
    expect(enumValueAtScope(taskStatus, 'ready_to_merge', issueStatus)).toBe('in_review');
  });

  it('follows a chain of mappings until the scope declares the value', () => {
    expect(enumValueAtScope(taskStatus, 'qa', issueStatus)).toBe('in_review');
    // At the subtype's own scope nothing is mapped.
    expect(enumValueAtScope(issueStatus, 'qa', issueStatus)).toBe('qa');
  });

  it('returns null for a value that maps to nothing the scope declares', () => {
    expect(enumValueAtScope(taskStatus, 'changes_requested', undefined)).toBeNull();
    expect(enumValueAtScope(taskStatus, 'unknown', issueStatus)).toBeNull();
    const orphan = enumField({ coreValues: [{ value: 'triage', label: 'Triage' }] });
    expect(enumValueAtScope(taskStatus, 'triage', orphan)).toBeNull();
  });

  it('ends on a mapping cycle instead of looping', () => {
    const cyclic = enumField({
      coreValues: [
        { value: 'a', label: 'A', mapsTo: 'b' },
        { value: 'b', label: 'B', mapsTo: 'a' }
      ]
    });
    expect(enumValueAtScope(taskStatus, 'a', cyclic)).toBeNull();
  });
});
