/**
 * Regression coverage for consecutive Kanban status moves on the same card,
 * via a real rendered KanbanView + real sharedNodeStore, using a `task` node
 * so the write goes through the type-specific `updateTaskNode` updater path.
 * Existing kanban-dnd.test.ts coverage only exercises ONE move per test (or a
 * move plus a rejection) — never two successive successful moves on the same
 * card, which is the sequence that used to fail after the first move.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';
import type { TaskNode, TaskNodeUpdate } from '$lib/types';
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import KanbanView from '$lib/components/query/kanban-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';
import { pluginRegistry } from '$lib/plugins/index';
import { registerCorePlugins } from '$lib/plugins/core-plugins';

// Browser-mode's setup-browser.ts does not auto-register core plugins (unlike
// setup.ts for Happy-DOM tests) — the task type's status update needs its
// type-specific `updater` (routes to `updateTaskNode`, not generic
// `updateNode`) registered for this suite to exercise the real path the
// issue's repro node used.
if (!pluginRegistry.hasPlugin('task')) {
  registerCorePlugins(pluginRegistry);
}

function taskSchema(): SchemaNode {
  return {
    id: 'task',
    content: 'Task',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: true,
    schemaVersion: 1,
    fields: [
      {
        name: 'status',
        friendlyName: 'Status',
        type: 'enum',
        protection: 'core',
        indexed: false,
        // Mirrors the real task schema (packages/core/src/models/core_schemas.rs):
        // `status` is required — a task's `TaskStatus` is non-nullable on the
        // backend, with no clear semantics (unlike priority/dueDate).
        required: true,
        coreValues: [
          { value: 'open', label: 'Open' },
          { value: 'in_progress', label: 'In Progress' },
          { value: 'done', label: 'Done' }
        ],
        userValues: []
      }
    ]
  };
}

function task(id: string, status: string, title: string, version = 1): Node {
  return {
    id,
    nodeType: 'task',
    content: title,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version,
    properties: { status },
    mentions: [],
    status
  } as unknown as Node;
}

function seed(node: Node): void {
  sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });
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

describe('KanbanView — consecutive status moves on a task node (browser mode)', () => {
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

  it('persists a second consecutive move (open -> in_progress -> done) with the correct incrementing version, without remounting', async () => {
    seed(task('t1', 'open', 'Ship it', 1));

    const versionsSent: number[] = [];
    vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(
      async (id: string, version: number, update: TaskNodeUpdate) => {
        versionsSent.push(version);
        return {
          id,
          nodeType: 'task',
          content: 'Ship it',
          createdAt: '2026-01-01T00:00:00Z',
          modifiedAt: new Date().toISOString(),
          version: version + 1,
          status: update.status ?? 'open'
        } as unknown as TaskNode;
      }
    );

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: taskSchema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const moveSelect = getByRole('combobox', {
      name: 'Move Ship it to another column'
    }) as HTMLSelectElement;
    expect(moveSelect.value).toBe('open');

    // First move: open -> in_progress
    await fireEvent.change(moveSelect, { target: { value: 'in_progress' } });
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'In Progress'))).toEqual(['Ship it']);
    });

    // Second move, on the SAME rendered instance (no remount): in_progress -> done
    const moveSelectAgain = getByRole('combobox', {
      name: 'Move Ship it to another column'
    }) as HTMLSelectElement;
    await fireEvent.change(moveSelectAgain, { target: { value: 'done' } });
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Done'))).toEqual(['Ship it']);
    });
    expect(cardsIn(columnFor(container, 'Open'))).toEqual([]);
    expect(cardsIn(columnFor(container, 'In Progress'))).toEqual([]);

    // The critical assertion: each write sent the version RETURNED by the
    // previous write, not a stale/repeated one — this is exactly the OCC
    // mismatch the issue describes ("every subsequent status change fails").
    // The store applies each move before sending its write, so wait for the
    // second write rather than asserting on the render's timing.
    await waitFor(() => {
      expect(versionsSent).toEqual([1, 2]);
    });
    expect(conflictNotifications.notifications).toEqual([]);
  });

  it('persists a second consecutive drag move with the correct incrementing version', async () => {
    seed(task('t1', 'open', 'Ship it', 1));
    seed(task('t2', 'done', 'Already done', 1));

    const versionsSent: number[] = [];
    vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(
      async (id: string, version: number, update: TaskNodeUpdate) => {
        versionsSent.push(version);
        return {
          id,
          nodeType: 'task',
          content: id === 't1' ? 'Ship it' : 'Already done',
          createdAt: '2026-01-01T00:00:00Z',
          modifiedAt: new Date().toISOString(),
          version: version + 1,
          status: update.status ?? 'open'
        } as unknown as TaskNode;
      }
    );

    const { container } = render(KanbanView, {
      props: {
        nodeIds: ['t1', 't2'],
        schema: taskSchema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

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

    await dragAndDrop(cardFor(container, 'Ship it'), columnFor(container, 'In Progress'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'In Progress'))).toEqual(['Ship it']);
    });

    await dragAndDrop(cardFor(container, 'Ship it'), columnFor(container, 'Done'));
    await waitFor(() => {
      expect(cardsIn(columnFor(container, 'Done'))).toEqual(
        expect.arrayContaining(['Ship it', 'Already done'])
      );
    });
    expect(cardsIn(columnFor(container, 'In Progress'))).toEqual([]);

    // The store applies the move before sending its write, so the card lands
    // in its new column first — wait for the second write rather than
    // asserting on the render's timing.
    await waitFor(() => {
      expect(versionsSent).toEqual([1, 2]);
    });
    expect(conflictNotifications.notifications).toEqual([]);
  });

  it('does not offer Unassigned as a column or a move-to target when grouped by a required field (task status)', async () => {
    seed(task('t1', 'open', 'Ship it', 1));

    const updateSpy = vi.spyOn(backendAdapter, 'updateTaskNode');

    const { container, getByRole } = render(KanbanView, {
      props: {
        nodeIds: ['t1'],
        schema: taskSchema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    // No Unassigned column at all — `status` can never be cleared, so there
    // is nothing to bucket into it.
    expect(
      Array.from(container.querySelectorAll('.kanban-column-title')).map(
        (el) => el.textContent?.trim()
      )
    ).toEqual(['Open', 'In Progress', 'Done']);

    // The per-card "Move to" select offers the same three columns, no
    // Unassigned option a user could pick to trigger a silent no-op write.
    const moveSelect = getByRole('combobox', {
      name: 'Move Ship it to another column'
    }) as HTMLSelectElement;
    const optionValues = Array.from(moveSelect.options).map((o) => o.value);
    expect(optionValues).toEqual(['open', 'in_progress', 'done']);

    // Defense-in-depth: even a direct write attempt (bypassing the select)
    // must not silently clear status — moveCard's own guard rejects it
    // before ever reaching the backend.
    expect(updateSpy).not.toHaveBeenCalled();
  });
});
