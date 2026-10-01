/**
 * KanbanView — group-by empty states.
 *
 * Two states, previously conflated:
 *  - No eligible enum field on the schema at all: the "Group by" control
 *    must stay visible (disabled), with a message explaining why, rather
 *    than replacing the whole view — its absence shouldn't read as a
 *    rendering failure.
 *  - Eligible fields exist but none is selected yet (no stored group-by):
 *    no arbitrary field is pre-selected and no board renders until the
 *    user picks one.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import KanbanView from '$lib/components/query/kanban-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

function schemaWithEnum(): SchemaNode {
  return {
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id: 'widget',
    content: 'Widget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    description: '',
    relationships: [],
    fields: [
      {
        name: 'status',
        friendlyName: 'Status',
        type: 'enum',
        protection: 'user',
        indexed: false,
        coreValues: [{ value: 'open', label: 'Open' }],
        userValues: []
      }
    ]
  };
}

function schemaWithoutEnum(): SchemaNode {
  return {
    id: 'widget',
    nodeType: 'schema',
    lifecycleStatus: 'active',
    properties: {},
    content: 'Widget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    description: '',
    relationships: [],
    fields: [
      { name: 'note', friendlyName: 'Note', type: 'text', protection: 'user', indexed: false }
    ]
  };
}

function node(id: string, overrides: Partial<Node> = {}): Node {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType: 'widget',
    content: `Card ${id}`,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { status: 'open' },
    mentions: [],
    ...overrides
  };
}

describe('KanbanView — group-by empty states', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
  });

  it('renders no columns and a prompt when no group-by is stored, without pre-selecting a field', () => {
    const n = node('n1');
    sharedNodeStore.setNode(n, { type: 'database', reason: 'seed' });

    const { container, getByLabelText } = render(KanbanView, {
      props: {
        nodeIds: [n.id],
        schema: schemaWithEnum(),
        groupBy: undefined,
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const select = getByLabelText('Group by') as HTMLSelectElement;
    expect(select.disabled).toBe(false);
    expect(select.value).toBe('');
    expect(container.querySelector('.kanban-column')).toBeNull();
    expect(container.querySelector('.kanban-board')).toBeNull();
    expect(container.textContent).toContain('Choose a field to group this board by.');
  });

  it('picking a field renders the board and reports the choice via onGroupByChange', async () => {
    const n = node('n1');
    sharedNodeStore.setNode(n, { type: 'database', reason: 'seed' });
    const onGroupByChange = vi.fn();

    const { getByLabelText, container } = render(KanbanView, {
      props: {
        nodeIds: [n.id],
        schema: schemaWithEnum(),
        groupBy: undefined,
        onGroupByChange,
        onRowClick: () => {}
      }
    });

    const select = getByLabelText('Group by') as HTMLSelectElement;
    await fireEvent.change(select, { target: { value: 'status' } });

    expect(onGroupByChange).toHaveBeenCalledWith('status');
    expect(container.querySelectorAll('.kanban-column').length).toBeGreaterThan(0);
  });

  it('restores directly from a stored group-by without requiring a re-pick', () => {
    const n = node('n1');
    sharedNodeStore.setNode(n, { type: 'database', reason: 'seed' });

    const { container, getByLabelText } = render(KanbanView, {
      props: {
        nodeIds: [n.id],
        schema: schemaWithEnum(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const select = getByLabelText('Group by') as HTMLSelectElement;
    expect(select.value).toBe('status');
    expect(container.querySelectorAll('.kanban-column').length).toBeGreaterThan(0);
  });

  it('keeps the Group by control visible but disabled when the schema has no eligible enum field', () => {
    const n = node('n1');
    sharedNodeStore.setNode(n, { type: 'database', reason: 'seed' });

    const { container, getByLabelText } = render(KanbanView, {
      props: {
        nodeIds: [n.id],
        schema: schemaWithoutEnum(),
        groupBy: undefined,
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const select = getByLabelText('Group by') as HTMLSelectElement;
    expect(select.disabled).toBe(true);
    expect(container.querySelector('.kanban-board')).toBeNull();
    expect(container.textContent).toContain('This type has no enum field to group by');
  });
});
