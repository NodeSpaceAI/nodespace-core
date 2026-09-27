/**
 * Conflicts view merge action: a merge refused by a tree invariant is
 * explained inline in its row instead of only being logged, and the
 * confirmation names where the merged node will live when the two nodes sit
 * in different places.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({ focusOrOpenNode: vi.fn() })
}));

import ConflictsPane from '$lib/components/conflicts/conflicts-pane.svelte';
import { conflictsStore, type ConflictRecord } from '$lib/stores/conflicts.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types';

const LABELS: Record<string, string> = {
  alice: 'Alice',
  alice2: 'Alice (dup)',
  work: 'Work',
  p1: 'People',
  p2: 'Contacts'
};

const RECORD: ConflictRecord = {
  id: 'c1',
  kind: 'unique_field_collision',
  nodeIds: ['alice', 'alice2'],
  detail: { node_type: 'person', field: 'email', value: 'a@example.com' },
  status: 'open',
  detectedAt: '2026-01-01T00:00:00Z',
  detectedBy: null,
  occurrences: 1,
  lastSeenAt: '2026-01-01T00:00:00Z',
  resolvedAt: null,
  resolution: null
};

const MERGE_OUTCOME = {
  survivorId: 'alice',
  loserId: 'alice2',
  propertiesMerged: 0,
  edgesRepointed: 0,
  edgesDropped: 0
};

/** Route `invoke` by command; `preview` / `merge` override those two. */
function routeInvoke(handlers: { preview?: () => unknown; merge?: () => unknown }) {
  mockInvoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'list_conflicts':
        return [RECORD];
      case 'preview_merge':
        return handlers.preview ? handlers.preview() : noMove();
      case 'merge_nodes':
        return handlers.merge ? handlers.merge() : MERGE_OUTCOME;
      case 'conflicts_for_node':
        return [{ ...RECORD, status: 'resolved' }];
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
}

function noMove() {
  return { survivorParentId: null, loserParentId: null, resultingParentId: null };
}

function refusal(rule: string, nodeId: string, relatedIds: string[]) {
  return {
    code: 'TREE_INVARIANT_VIOLATION',
    message: `${rule}: refused`,
    conflictData: { rule, node_id: nodeId, related_ids: relatedIds, detail: 'refused' }
  };
}

async function clickKeepAlice() {
  const view = render(ConflictsPane);
  await fireEvent.click(await view.findByRole('button', { name: 'Keep Alice' }));
  return view;
}

describe('ConflictsPane merge', () => {
  let confirmSpy: MockInstance<(message?: string) => boolean>;

  beforeEach(() => {
    mockInvoke.mockReset();
    conflictsStore.records = [];
    conflictsStore.loaded = false;
    vi.spyOn(backendAdapter, 'getNode').mockImplementation(
      async (id: string) => ({ id, content: LABELS[id] ?? id, nodeType: 'person' }) as Node
    );
    confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('explains a refused merge in the row, without asking to confirm', async () => {
    routeInvoke({
      preview: () => {
        throw refusal('member_of_not_root', 'alice', ['work']);
      }
    });

    const view = await clickKeepAlice();

    const alert = await view.findByRole('alert');
    expect(alert.textContent).toContain('Remove "Alice" from "Work" first, then merge.');
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(mockInvoke).not.toHaveBeenCalledWith('merge_nodes', expect.anything());
  });

  it('names the position the survivor keeps when the two sit in different places', async () => {
    routeInvoke({
      preview: () => ({ survivorParentId: 'p1', loserParentId: 'p2', resultingParentId: 'p1' })
    });

    await clickKeepAlice();

    await vi.waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('merge_nodes', expect.anything()));
    const message = confirmSpy.mock.calls[0][0] ?? '';
    expect(message).toContain('The merged node "Alice" will sit under "People".');
    expect(message).toContain('"Alice (dup)" is removed from under "Contacts".');
  });

  it('says nothing about position when both already sit in the same place', async () => {
    routeInvoke({});

    await clickKeepAlice();

    await vi.waitFor(() => expect(confirmSpy).toHaveBeenCalled());
    const message = confirmSpy.mock.calls[0][0] ?? '';
    expect(message).not.toContain('merged node');
  });

  it('explains a refusal raised by the merge itself after confirming', async () => {
    routeInvoke({
      merge: () => {
        throw refusal('cycle', 'alice', ['alice2']);
      }
    });

    const view = await clickKeepAlice();

    const alert = await view.findByRole('alert');
    expect(alert.textContent).toContain(
      'Move "Alice" out of "Alice (dup)"\'s subtree first, or keep "Alice (dup)" instead.'
    );
  });

  it('shows a non-refusal failure instead of only logging it', async () => {
    routeInvoke({
      merge: () => {
        throw { code: 'GRPC_ERROR', message: 'daemon unavailable' };
      }
    });

    const view = await clickKeepAlice();

    const alert = await view.findByRole('alert');
    expect(alert.textContent).toBe('Merge failed: daemon unavailable');
  });

  it('does not merge when the user cancels the confirmation', async () => {
    routeInvoke({});
    confirmSpy.mockReturnValue(false);

    const view = await clickKeepAlice();

    await vi.waitFor(() => expect(confirmSpy).toHaveBeenCalled());
    expect(mockInvoke).not.toHaveBeenCalledWith('merge_nodes', expect.anything());
    expect(view.queryByRole('alert')).toBeNull();
  });
});
