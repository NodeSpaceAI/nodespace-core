/**
 * KanbanView — real drag-and-drop, rollback, and keyboard-move coverage.
 *
 * `kanban-grouping.test.ts` unit-tests the extracted grouping/write-shape
 * logic, but the drag/drop → store-write → reactive-regroup → rollback flow,
 * and the keyboard move-select, need a real browser: Happy-DOM cannot
 * originate real `DragEvent`s with a working `DataTransfer`. This exercises
 * the actual component against a real Chromium DOM: a real
 * `dragstart`/`dragover`/`drop` sequence, dispatched with a real
 * `DataTransfer`, driving the same handlers a real drag would.
 *
 * `sharedNodeStore` is the real singleton (not a stub) so the reactive
 * regroup is genuine: cards are bucketed by reading the store, and a move
 * writes back through the store's real `updateNode`. Only the daemon RPC
 * boundary (`backendAdapter`) is mocked, matching the pattern in
 * shared-node-store.test.ts.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';
import { mockTauriCore } from '../helpers/mock-tauri-core';

// mockTauriCore() declares the full mocked export surface once, so a binding
// any transitive import needs — e.g. `daemon-status.ts` importing `isTauri`
// — can never be silently omitted here. Its default `isTauri` returns false:
// the true branch calls `listen` from @tauri-apps/api/event, which this file
// does not mock.
vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import KanbanView from '$lib/components/query/kanban-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';

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
        userValues: []
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

/** Seed a node as already-persisted, so a subsequent edit takes the real
 *  update-and-persist path (not the create path). */
function seed(node: Node): void {
  sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });
}

/** A second, unrelated type — for cross-board isolation coverage below. Its
 *  group-by field ('payment_state') doesn't exist on the 'ticket' type at all. */
function invoiceSchema(): SchemaNode {
  return {
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id: 'invoice',
    content: 'Invoice',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    fields: [
      {
        name: 'payment_state',
        friendlyName: 'Payment State',
        type: 'enum',
        protection: 'user',
        indexed: false,
        coreValues: [
          { value: 'pending', label: 'Pending' },
          { value: 'paid', label: 'Paid' }
        ],
        userValues: []
      }
    ]
  };
}

function invoice(id: string, paymentState: string, title: string): Node {
  return {
    lifecycleStatus: 'active',
    id,
    nodeType: 'invoice',
    content: title,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { payment_state: paymentState },
    mentions: []
  };
}

function cardFor(container: HTMLElement, title: string): HTMLElement {
  const el = Array.from(container.querySelectorAll('.kanban-card')).find(
    (card) => card.querySelector('.kanban-card-title')?.textContent?.trim() === title
  );
  if (!el) throw new Error(`No card found for "${title}"`);
  return el as HTMLElement;
}

function columnFor(container: HTMLElement, label: string): HTMLElement {
  const el = Array.from(container.querySelectorAll('.kanban-column')).find(
    (col) => col.querySelector('.kanban-column-title')?.textContent?.trim() === label
  );
  if (!el) throw new Error(`No column found for "${label}"`);
  return el as HTMLElement;
}

function cardsIn(column: HTMLElement): string[] {
  return Array.from(column.querySelectorAll('.kanban-card-title')).map(
    (el) => el.textContent?.trim() ?? ''
  );
}

/** Drive a real HTML5 drag-and-drop sequence with a real DataTransfer. */
async function dragAndDrop(source: HTMLElement, target: HTMLElement): Promise<void> {
  const dataTransfer = new DataTransfer();
  source.dispatchEvent(
    new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer })
  );
  target.dispatchEvent(
    new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer })
  );
  target.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }));
  source.dispatchEvent(new DragEvent('dragend', { bubbles: true, cancelable: true, dataTransfer }));
}

describe('KanbanView — drag-and-drop (browser mode)', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    sharedNodeStore.clearAll();
    conflictNotifications.dismissAll();
  });

  it('dragging a card to another column moves it and persists the write', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));
    seed(ticket('t2', 'closed', 'Ship the feature'));

    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
      ...ticket('t1', 'closed', 'Fix the bug'),
      version: 2
    });

    const { container } = render(KanbanView, {
      props: {
        nodeIds: ['t1', 't2'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    expect(cardsIn(columnFor(container, 'Open'))).toEqual(['Fix the bug']);
    expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Ship the feature']);

    await dragAndDrop(cardFor(container, 'Fix the bug'), columnFor(container, 'Closed'));

    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(
        expect.arrayContaining(['Ship the feature', 'Fix the bug'])
      );
    });
    expect(cardsIn(columnFor(container, 'Open'))).toEqual([]);

    expect(updateSpy).toHaveBeenCalledWith(
      't1',
      1,
      expect.objectContaining({ properties: expect.objectContaining({ status: 'closed' }) })
    );
  });

  it('dropping a card into its own column performs no write and leaves it in place', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));

    const updateSpy = vi.spyOn(backendAdapter, 'updateNode');

    const { container } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    await dragAndDrop(cardFor(container, 'Fix the bug'), columnFor(container, 'Open'));

    // Give any (wrongly-fired) write a tick to land before asserting its absence.
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect(updateSpy).not.toHaveBeenCalled();
    expect(cardsIn(columnFor(container, 'Open'))).toEqual(['Fix the bug']);
  });

  it('rolls a card back to its original column when the persisted write is rejected', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));

    vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue(new Error('daemon offline'));

    const { container } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    await dragAndDrop(cardFor(container, 'Fix the bug'), columnFor(container, 'Closed'));

    // Optimistic move lands first — the card visibly moves before the
    // rejected write is even known to have failed.
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Fix the bug']);
    });

    // Once the rejection is processed, the card returns to its original column.
    await waitFor(
      () => {
        expect(cardsIn(columnFor(container, 'Open'))).toEqual(['Fix the bug']);
      },
      { timeout: 3000 }
    );
    expect(cardsIn(columnFor(container, 'Closed'))).toEqual([]);

    await waitFor(() => {
      expect(
        conflictNotifications.notifications.some(
          (n) => n.nodeId === 't1' && n.conflictType === 'write-failure'
        )
      ).toBe(true);
    });
  });

  it('does not let a late-arriving revert stomp a newer move to the same card', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));

    // Hold the first write pending so its failure can be triggered on
    // demand, after the second move has already landed.
    let rejectFirstWrite: (error: Error) => void = () => {};
    const firstWrite = new Promise<never>((_resolve, reject) => {
      rejectFirstWrite = reject;
    });
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementationOnce(() => firstWrite)
      .mockResolvedValueOnce({ ...ticket('t1', '', 'Fix the bug'), version: 2 });

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    // First move: Open -> Closed. Its write never settles until triggered below.
    await dragAndDrop(cardFor(container, 'Fix the bug'), columnFor(container, 'Closed'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Fix the bug']);
    });

    // Second move, before the first write is known to have failed: Closed ->
    // Unassigned, via the keyboard select. The persistence coordinator
    // collapses this behind the still-executing first write, but the
    // optimistic apply — and this view's reveal bookkeeping — land
    // immediately regardless of when its own RPC actually runs.
    const moveSelect = getByRole('combobox', {
      name: 'Move Fix the bug to another column'
    }) as HTMLSelectElement;
    await fireEvent.change(moveSelect, { target: { value: '__unassigned__' } });
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);
    });

    // Now the first move's write fails. Its revert target (Closed) no
    // longer matches the card's current column (Unassigned, set by the
    // second move) — the revert must recognize that and no-op, not stomp
    // the second move's result back to its own original column (Open).
    rejectFirstWrite(new Error('daemon offline'));

    // Give the (would-be, incorrect) revert a chance to fire before asserting.
    await new Promise((resolve) => setTimeout(resolve, 100));

    expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);
    expect(cardsIn(columnFor(container, 'Open'))).toEqual([]);
    expect(cardsIn(columnFor(container, 'Closed'))).toEqual([]);
    expect(updateSpy).toHaveBeenCalledTimes(2);
  });

  it('reverts all the way to the true original column when two chained moves both fail', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));

    // Both writes are held pending so their rejection order can be
    // controlled explicitly.
    let rejectFirstWrite: (error: Error) => void = () => {};
    const firstWrite = new Promise<never>((_resolve, reject) => {
      rejectFirstWrite = reject;
    });
    let rejectSecondWrite: (error: Error) => void = () => {};
    const secondWrite = new Promise<never>((_resolve, reject) => {
      rejectSecondWrite = reject;
    });
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementationOnce(() => firstWrite)
      .mockImplementationOnce(() => secondWrite);

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    // First move: Open -> Closed.
    await dragAndDrop(cardFor(container, 'Fix the bug'), columnFor(container, 'Closed'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Fix the bug']);
    });

    // Second move, before the first write's outcome is known: Closed ->
    // Unassigned. Its own `from` reads "Closed" — a value the first move
    // set optimistically but hasn't actually been confirmed by anyone.
    const moveSelect = getByRole('combobox', {
      name: 'Move Fix the bug to another column'
    }) as HTMLSelectElement;
    await fireEvent.change(moveSelect, { target: { value: '__unassigned__' } });
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);
    });

    // First move fails: its revert target (Closed) no longer matches the
    // card's current column (Unassigned), so it correctly no-ops rather
    // than reverting to Open here — the chain isn't resolved yet.
    rejectFirstWrite(new Error('daemon offline'));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);

    // Second move ALSO fails. A revert using only its own locally-read
    // `from` ("Closed") would land the card back on a value nobody ever
    // actually persisted — the first move's failed intermediate target.
    // The chain-origin must win: the true original was Open.
    rejectSecondWrite(new Error('daemon offline'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Open'))).toEqual(['Fix the bug']);
    });
    expect(cardsIn(columnFor(container, 'Closed'))).toEqual([]);
    expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual([]);
    expect(updateSpy).toHaveBeenCalledTimes(2);
  });

  it('reverts to Unassigned (not an intermediate column) when a card that started Unassigned has two chained moves both fail', async () => {
    // The chain-origin map stores `null` for a card that started in
    // Unassigned — a real, meaningful value, not "nothing recorded". A
    // revert that treats a stored `null` the same as a missing map entry
    // (e.g. via `??`) would silently fall back to the wrong thing.
    seed(ticket('t1', '', 'Fix the bug'));

    let rejectFirstWrite: (error: Error) => void = () => {};
    const firstWrite = new Promise<never>((_resolve, reject) => {
      rejectFirstWrite = reject;
    });
    let rejectSecondWrite: (error: Error) => void = () => {};
    const secondWrite = new Promise<never>((_resolve, reject) => {
      rejectSecondWrite = reject;
    });
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementationOnce(() => firstWrite)
      .mockImplementationOnce(() => secondWrite);

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);

    // First move: Unassigned -> Open.
    await dragAndDrop(cardFor(container, 'Fix the bug'), columnFor(container, 'Open'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Open'))).toEqual(['Fix the bug']);
    });

    // Second move, before the first write's outcome is known: Open -> Closed.
    const moveSelect = getByRole('combobox', {
      name: 'Move Fix the bug to another column'
    }) as HTMLSelectElement;
    await fireEvent.change(moveSelect, { target: { value: 'closed' } });
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Fix the bug']);
    });

    // First move fails: its revert target (Open) no longer matches the
    // card's current column (Closed), so it correctly no-ops.
    rejectFirstWrite(new Error('daemon offline'));
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Fix the bug']);

    // Second move ALSO fails. The true origin — Unassigned, stored as
    // `null` — must win, not "Open" (the first move's own intermediate
    // target) and not a `??`-induced fallback to "Closed" (the second
    // move's own `from`, since a stored `null` is not the same as "no
    // origin recorded").
    rejectSecondWrite(new Error('daemon offline'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);
    });
    expect(cardsIn(columnFor(container, 'Open'))).toEqual([]);
    expect(cardsIn(columnFor(container, 'Closed'))).toEqual([]);
    expect(updateSpy).toHaveBeenCalledTimes(2);
  });
});

describe('KanbanView — cross-board drag isolation (browser mode)', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    sharedNodeStore.clearAll();
    conflictNotifications.dismissAll();
  });

  it('rejects a drop whose dataTransfer id belongs to a different board\'s nodeIds (split view)', async () => {
    // Board A: tasks grouped by 'status'. Board B: an unrelated type grouped
    // by a field the task type doesn't even have ('payment_state') — the
    // exact split-view scenario from the issue. kanban-view is the only
    // component that sets node-id drag data, so two live instances on
    // screen at once can cross-talk through the DOM's single DataTransfer.
    seed(ticket('t1', 'open', 'Fix the bug'));
    seed(invoice('i1', 'pending', 'Invoice #1'));

    const updateSpy = vi.spyOn(backendAdapter, 'updateNode');

    const boardA = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });
    const boardB = render(KanbanView, {
      props: {
        nodeIds: ['i1'],
        schema: invoiceSchema(),
        groupBy: 'payment_state',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const sourceCard = cardFor(boardA.container, 'Fix the bug'); // t1, board A's own card
    const targetColumn = columnFor(boardB.container, 'Paid'); // board B's column

    // dragstart fires on board A's own card, so board A's local draggingId
    // is 't1' and dataTransfer carries 't1' too — but the drop lands on
    // board B's column. Board B's own draggingId was never set (its
    // onDragStart never fired), so its onDrop falls back to dataTransfer
    // and must recognize 't1' isn't one of ITS nodeIds (['i1']) before
    // doing anything with it.
    await dragAndDrop(sourceCard, targetColumn);

    // Give any (wrongly-fired) write a tick to land before asserting its absence.
    await new Promise((resolve) => setTimeout(resolve, 50));

    // No write at all — not the task's own 'status', and definitely not
    // board B's 'payment_state' onto the task node.
    expect(updateSpy).not.toHaveBeenCalled();
    // The task node is untouched, still in board A's Open column.
    expect(cardsIn(columnFor(boardA.container, 'Open'))).toEqual(['Fix the bug']);
    // Board B's Paid column stays empty — the foreign card was never adopted.
    expect(cardsIn(columnFor(boardB.container, 'Paid'))).toEqual([]);
    expect(cardsIn(columnFor(boardB.container, 'Pending'))).toEqual(['Invoice #1']);
    // The shared store's task node was never given board B's field at all.
    const node = sharedNodeStore.getNode('t1');
    expect(node?.properties?.payment_state).toBeUndefined();
  });

  it('still allows a same-board drop that falls back to dataTransfer (draggingId cleared mid-drag)', async () => {
    // Guards against an over-broad fix: the dataTransfer fallback path
    // itself is legitimate (e.g. draggingId reset by an intervening
    // dragend/state change within the SAME board) as long as the id really
    // is one of this board's own nodeIds — only a FOREIGN id must be
    // rejected, not the fallback mechanism itself.
    seed(ticket('t1', 'open', 'Fix the bug'));
    seed(ticket('t2', 'closed', 'Ship the feature'));

    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
      ...ticket('t1', 'closed', 'Fix the bug'),
      version: 2
    });

    const { container } = render(KanbanView, {
      props: {
        nodeIds: ['t1', 't2'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const targetColumn = columnFor(container, 'Closed');

    // Only dispatch dragover + drop (skip dragstart) so draggingId stays
    // null on this instance, forcing the dataTransfer fallback — but seed
    // dataTransfer with this board's own valid id.
    const dataTransfer = new DataTransfer();
    dataTransfer.setData('text/plain', 't1');
    targetColumn.dispatchEvent(
      new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer })
    );
    targetColumn.dispatchEvent(
      new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer })
    );

    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(
        expect.arrayContaining(['Ship the feature', 'Fix the bug'])
      );
    });
    expect(updateSpy).toHaveBeenCalledWith(
      't1',
      1,
      expect.objectContaining({ properties: expect.objectContaining({ status: 'closed' }) })
    );
  });
});

describe('KanbanView — keyboard-accessible move (browser mode)', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
    conflictNotifications.dismissAll();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    sharedNodeStore.clearAll();
    conflictNotifications.dismissAll();
  });

  it('moves a card to another column via the per-card "Move to" select and persists it', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));

    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
      ...ticket('t1', 'closed', 'Fix the bug'),
      version: 2
    });

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const moveSelect = getByRole('combobox', {
      name: 'Move Fix the bug to another column'
    }) as HTMLSelectElement;
    expect(moveSelect.value).toBe('open');

    await fireEvent.change(moveSelect, { target: { value: 'closed' } });

    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Closed'))).toEqual(['Fix the bug']);
    });
    expect(cardsIn(columnFor(container, 'Open'))).toEqual([]);
    expect(updateSpy).toHaveBeenCalledWith(
      't1',
      1,
      expect.objectContaining({ properties: expect.objectContaining({ status: 'closed' }) })
    );
  });

  it('moving a card to Unassigned via the select clears the field and no-ops on re-selecting it', async () => {
    seed(ticket('t1', 'open', 'Fix the bug'));

    const updateSpy = vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
      ...ticket('t1', '', 'Fix the bug'),
      version: 2
    });

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const moveSelect = getByRole('combobox', {
      name: 'Move Fix the bug to another column'
    }) as HTMLSelectElement;

    await fireEvent.change(moveSelect, { target: { value: '__unassigned__' } });

    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Unassigned'))).toEqual(['Fix the bug']);
    });
    expect(updateSpy).toHaveBeenCalledTimes(1);
    // Clearing to Unassigned must write `null`, not `''` — an empty string
    // is a real (if unusual) value some backends persist as a genuine
    // user-defined enum value rather than treating it as "cleared".
    expect(updateSpy).toHaveBeenCalledWith(
      't1',
      1,
      expect.objectContaining({ properties: expect.objectContaining({ status: null }) })
    );

    // Re-selecting the column the card is already in is a no-op — no second write.
    const settledSelect = getByRole('combobox', {
      name: 'Move Fix the bug to another column'
    }) as HTMLSelectElement;
    await fireEvent.change(settledSelect, { target: { value: '__unassigned__' } });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(updateSpy).toHaveBeenCalledTimes(1);
  });
});
