/**
 * KanbanView — the card carries no status control, a card can still be moved
 * from the keyboard, and columns follow the query's stored order.
 *
 * A card's column is its value, so the card shows only its title. Moving one
 * without a mouse goes through a "Move to" menu that opens on demand (M on
 * the focused card, or its context menu) and writes through the same
 * `moveCard` path a drag does. Real drag-and-drop coverage lives in the
 * browser tier (src/tests/browser/kanban-dnd.test.ts).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import KanbanView from '$lib/components/query/kanban-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

function schema(): SchemaNode {
  return {
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id: 'ticket',
    content: 'Ticket',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    relationships: [],
    fields: [
      {
        name: 'status',
        friendlyName: 'Status',
        type: 'enum',
        protection: 'user',
        indexed: false,
        coreValues: [
          { value: 'open', label: 'Open' },
          { value: 'closed', label: 'Closed' }
        ],
        userValues: [{ value: 'blocked', label: 'Blocked' }]
      }
    ]
  };
}

function ticket(id: string, status: string, title: string): Node {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType: 'ticket',
    content: title,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { status },
    mentions: []
  };
}

function renderBoard(nodes: Node[], columnOrder?: Record<string, string[]>) {
  for (const n of nodes) sharedNodeStore.setNode(n, { type: 'database', reason: 'seed' });
  return render(KanbanView, {
    props: {
      nodeIds: nodes.map((n) => n.id),
      schema: schema(),
      groupBy: 'status',
      columnOrder,
      onGroupByChange: () => {},
      onRowClick: () => {}
    }
  });
}

function columnTitles(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('.kanban-column-title')).map(
    (el) => el.textContent?.trim() ?? ''
  );
}

function cardsIn(container: HTMLElement, label: string): string[] {
  const column = Array.from(container.querySelectorAll('.kanban-column')).find(
    (col) => col.querySelector('.kanban-column-title')?.textContent?.trim() === label
  );
  if (!column) throw new Error(`No column found for "${label}"`);
  return Array.from(column.querySelectorAll('.kanban-card-title')).map(
    (el) => el.textContent?.trim() ?? ''
  );
}

function menuItems(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll('[role="menuitem"]')).map(
    (el) => el.textContent?.trim() ?? ''
  );
}

describe('KanbanView — card', () => {
  beforeEach(() => sharedNodeStore.clearAll());

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    sharedNodeStore.clearAll();
  });

  it('shows the title and no status control', async () => {
    const onRowClick = vi.fn();
    sharedNodeStore.setNode(ticket('t1', 'open', 'Fix the bug'), {
      type: 'database',
      reason: 'seed'
    });
    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick
      }
    });

    const card = container.querySelector('.kanban-card') as HTMLElement;
    expect(card.querySelector('select')).toBeNull();
    expect(card.querySelector('[role="menu"]')).toBeNull();
    expect(card.textContent?.trim()).toBe('Fix the bug');

    await fireEvent.click(getByRole('button', { name: 'Fix the bug' }));
    expect(onRowClick).toHaveBeenCalledWith('t1');
  });

  it('moves the focused card to another column from the keyboard, with the write a drag makes', async () => {
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockResolvedValue({ ...ticket('t1', 'closed', 'Fix the bug'), version: 2 });
    const { container, getByRole } = renderBoard([ticket('t1', 'open', 'Fix the bug')]);

    const card = getByRole('button', { name: 'Fix the bug' });
    card.focus();
    await fireEvent.keyDown(card, { key: 'm' });

    // Every column but the card's own, with the first one focused.
    const menu = getByRole('menu', { name: 'Move Fix the bug to' });
    expect(menuItems(container)).toEqual(['Closed', 'Blocked', 'Unassigned']);
    expect(document.activeElement?.textContent?.trim()).toBe('Closed');

    await fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement?.textContent?.trim()).toBe('Blocked');
    await fireEvent.keyDown(menu, { key: 'ArrowUp' });
    expect(document.activeElement?.textContent?.trim()).toBe('Closed');
    await fireEvent.keyDown(menu, { key: 'End' });
    expect(document.activeElement?.textContent?.trim()).toBe('Unassigned');
    await fireEvent.keyDown(menu, { key: 'Home' });

    await fireEvent.click(document.activeElement as HTMLElement);

    await waitFor(() => expect(cardsIn(container, 'Closed')).toEqual(['Fix the bug']));
    expect(cardsIn(container, 'Open')).toEqual([]);
    expect(container.querySelector('[role="menu"]')).toBeNull();
    // Focus follows the card into its new column.
    await waitFor(() =>
      expect(document.activeElement).toBe(getByRole('button', { name: 'Fix the bug' }))
    );
    await waitFor(() =>
      expect(updateSpy).toHaveBeenCalledWith(
        't1',
        1,
        expect.objectContaining({ properties: expect.objectContaining({ status: 'closed' }) })
      )
    );
  });

  it('Escape closes the menu without a write and returns focus to the card', async () => {
    const updateSpy = vi.spyOn(backendAdapter, 'updateNode');
    const { container, getByRole } = renderBoard([ticket('t1', 'open', 'Fix the bug')]);

    const card = getByRole('button', { name: 'Fix the bug' });
    card.focus();
    await fireEvent.keyDown(card, { key: 'M' });
    await fireEvent.keyDown(getByRole('menu'), { key: 'Escape' });

    expect(container.querySelector('[role="menu"]')).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(card));
    expect(cardsIn(container, 'Open')).toEqual(['Fix the bug']);
    expect(updateSpy).not.toHaveBeenCalled();
  });

  it('opens the same menu from the card context menu, and leaves shortcuts with a modifier alone', async () => {
    const { container, getByRole } = renderBoard([ticket('t1', 'open', 'Fix the bug')]);
    const card = getByRole('button', { name: 'Fix the bug' });

    await fireEvent.keyDown(card, { key: 'm', metaKey: true });
    expect(container.querySelector('[role="menu"]')).toBeNull();

    await fireEvent.contextMenu(container.querySelector('.kanban-card') as HTMLElement);
    expect(menuItems(container)).toEqual(['Closed', 'Blocked', 'Unassigned']);
  });
});

describe('KanbanView — column order', () => {
  beforeEach(() => sharedNodeStore.clearAll());

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
  });

  const nodes = () => [ticket('t1', 'open', 'One'), ticket('t2', 'blocked', 'Two')];

  it('keeps enum order with no stored order', () => {
    const { container } = renderBoard(nodes());
    expect(columnTitles(container)).toEqual(['Open', 'Closed', 'Blocked', 'Unassigned']);
  });

  it('follows the stored order for the group-by field, with Unassigned last', () => {
    const { container } = renderBoard(nodes(), { status: ['blocked', 'open', 'closed'] });
    expect(columnTitles(container)).toEqual(['Blocked', 'Open', 'Closed', 'Unassigned']);
    expect(cardsIn(container, 'Blocked')).toEqual(['Two']);
    expect(cardsIn(container, 'Open')).toEqual(['One']);
  });

  it('appends values a partial order omits and ignores values the enum lacks', () => {
    const { container } = renderBoard(nodes(), { status: ['gone', 'closed'] });
    expect(columnTitles(container)).toEqual(['Closed', 'Open', 'Blocked', 'Unassigned']);
  });

  it('ignores an order stored for another field', () => {
    const { container } = renderBoard(nodes(), { priority: ['blocked', 'open'] });
    expect(columnTitles(container)).toEqual(['Open', 'Closed', 'Blocked', 'Unassigned']);
  });
});
