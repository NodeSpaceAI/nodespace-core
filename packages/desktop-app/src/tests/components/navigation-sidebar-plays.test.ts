/**
 * navigation-sidebar.svelte — the "Plays" section (ADR-090).
 *
 * The section lists the database's plays with each one's state and opens a
 * play in its viewer. It is shown only while the Labs "Playbooks" flag is on,
 * in both the expanded and the collapsed sidebar, and it writes nothing: the
 * on/off switch is in the viewer.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';

const focusOrOpenNode = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({ focusOrOpenNode })
}));

// The sidebar's onMount loads plays through the backend adapter; serve them
// from a mutable fixture so the real load path is exercised. Every write the
// adapter offers is a spy, so a test can assert the section made none.
const backend = vi.hoisted(() => ({
  plays: [] as unknown[],
  writes: [] as string[]
}));
vi.mock('$lib/services/backend-adapter', () => {
  const write = (name: string) => async () => {
    backend.writes.push(name);
    return null;
  };
  return {
    backendAdapter: {
      getAllSchemas: async () => [],
      getNode: async () => null,
      queryNodes: async (query: { nodeType?: string; includeArchived?: boolean }) =>
        query.nodeType === 'play' && !query.includeArchived ? backend.plays : [],
      updateNode: write('updateNode'),
      updatePlayNode: write('updatePlayNode'),
      createNode: write('createNode'),
      deleteNode: write('deleteNode')
    }
  };
});

import NavigationSidebar from '$lib/components/layout/navigation-sidebar.svelte';
import { playsData } from '$lib/stores/plays.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { layoutStore } from '$lib/stores/layout.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import type { Node } from '$lib/types';

function play(id: string, content: string, fields: Record<string, unknown> = {}) {
  return {
    id,
    nodeType: 'play',
    content,
    properties: {},
    version: 1,
    isSeeded: false,
    rules: [],
    enabled: true,
    ...fields
  };
}

function setLayout(sidebarCollapsed: boolean, playsExpanded = true) {
  layoutStore.state = { ...layoutStore.state, sidebarCollapsed, playsExpanded };
}

/** Each row as `[title, state label]`, in DOM order. */
function rows(container: HTMLElement): Array<[string, string]> {
  return Array.from(container.querySelectorAll('[data-testid="play-item"]')).map((row) => [
    row.querySelector('.play-name')?.textContent?.trim() ?? '',
    row.querySelector('.play-state')?.textContent?.trim() ?? ''
  ]);
}

function row(container: HTMLElement, title: string): HTMLElement {
  const found = Array.from(container.querySelectorAll<HTMLElement>('[data-testid="play-item"]')).find(
    (el) => el.querySelector('.play-name')?.textContent?.trim() === title
  );
  if (!found) throw new Error(`No play row titled "${title}"`);
  return found;
}

describe('NavigationSidebar — Plays section', () => {
  beforeEach(() => {
    localStorage.clear();
    focusOrOpenNode.mockClear();
    backend.plays = [];
    backend.writes = [];
    playsData.reset();
    labsFlags.playbooksEnabled = true;
    setLayout(false);
  });

  afterEach(() => {
    cleanup();
    playsData.reset();
    sharedNodeStore.clearAll();
    labsFlags.playbooksEnabled = false;
    setLayout(false, false);
    localStorage.clear();
  });

  describe('Labs gating', () => {
    it('expanded sidebar: hides the section while the Playbooks flag is off', async () => {
      labsFlags.playbooksEnabled = false;
      backend.plays = [play('p', 'Task status')];
      const { container, queryByText } = render(NavigationSidebar);
      await waitFor(() => expect(playsData.has('p')).toBe(true));

      expect(queryByText('Plays')).toBeNull();
      expect(queryByText('Task status')).toBeNull();
      expect(container.querySelector('[aria-label="Collapse Plays"]')).toBeNull();
      expect(container.querySelector('[aria-label="Expand Plays"]')).toBeNull();
    });

    it('collapsed sidebar: hides the Plays icon button while the flag is off', () => {
      labsFlags.playbooksEnabled = false;
      setLayout(true);
      const { container } = render(NavigationSidebar);

      expect(container.querySelector('button[title="Plays"]')).toBeNull();
    });

    it('expanded sidebar: shows the section while the flag is on', () => {
      const { container, getByText } = render(NavigationSidebar);

      expect(getByText('Plays')).toBeTruthy();
      expect(container.querySelector('[aria-label="Collapse Plays"]')).not.toBeNull();
    });

    it('collapsed sidebar: shows the Plays icon button while the flag is on', () => {
      setLayout(true);
      const { container } = render(NavigationSidebar);

      expect(container.querySelector('button[title="Plays"]')).not.toBeNull();
    });

    it('turning the flag off hides the section and changes no play', async () => {
      backend.plays = [play('p', 'Task status')];
      const { container, queryByText } = render(NavigationSidebar);
      await waitFor(() => expect(rows(container)).toEqual([['Task status', 'On']]));

      labsFlags.playbooksEnabled = false;

      await waitFor(() => expect(queryByText('Plays')).toBeNull());
      expect(rows(container)).toEqual([]);
      // The play is still listed by the store, still on, and nothing was written.
      expect(playsData.plays.map((p) => [p.id, p.state])).toEqual([['p', 'on']]);
      expect(backend.writes).toEqual([]);
    });
  });

  it('remembers whether the section is expanded', async () => {
    const { container } = render(NavigationSidebar);
    const trigger = container.querySelector<HTMLElement>('[aria-label="Collapse Plays"]');
    if (!trigger) throw new Error('Plays trigger not found');

    await fireEvent.click(trigger);

    expect(layoutStore.state.playsExpanded).toBe(false);
    expect(container.querySelector('[aria-label="Expand Plays"]')).not.toBeNull();
  });

  it('lists the participating plays by title and leaves archived ones out', async () => {
    // The backend applies the participation rule: an archived play is returned
    // only to a query that opts in, and the section never opts in.
    backend.plays = [
      play('p-user', 'Weekly review'),
      play('p-core', 'Task status', { isSeeded: true })
    ];
    const { container, queryByText } = render(NavigationSidebar);

    await waitFor(() =>
      expect(rows(container)).toEqual([
        ['Task status', 'On'],
        ['Weekly review', 'On']
      ])
    );
    expect(queryByText('Archived play')).toBeNull();
  });

  it('says so when there are no plays', async () => {
    const { findByText } = render(NavigationSidebar);

    expect(await findByText('No plays yet')).toBeTruthy();
  });

  it('shows each row as on, off or suspended, with the suspension message as a tooltip', async () => {
    backend.plays = [
      play('p-on', 'A'),
      play('p-off', 'B', { enabled: false }),
      play('p-suspended', 'C', {
        suspendedAt: '2026-03-01T10:00:00.000Z',
        suspendedReason: 'error',
        suspendedMessage: 'Action 2 failed: no such field'
      })
    ];
    const { container } = render(NavigationSidebar);

    await waitFor(() =>
      expect(rows(container)).toEqual([
        ['A', 'On'],
        ['B', 'Off'],
        ['C', 'Suspended']
      ])
    );
    expect(row(container, 'A').querySelector('.play-state')?.getAttribute('data-state')).toBe('on');
    expect(row(container, 'B').querySelector('.play-state')?.getAttribute('data-state')).toBe('off');
    expect(row(container, 'C').querySelector('.play-state')?.getAttribute('data-state')).toBe(
      'suspended'
    );
    expect(row(container, 'C').getAttribute('title')).toBe('Action 2 failed: no such field');
    expect(row(container, 'A').getAttribute('title')).toBeNull();
  });

  it('rows have no switch, and clicking one opens the play without writing', async () => {
    backend.plays = [play('p', 'Task status')];
    const { container } = render(NavigationSidebar);
    await waitFor(() => expect(rows(container)).toHaveLength(1));

    expect(container.querySelector('.play-list [role="switch"]')).toBeNull();
    expect(container.querySelector('.play-list input')).toBeNull();

    await fireEvent.click(row(container, 'Task status'));

    expect(focusOrOpenNode).toHaveBeenCalledWith('p', { nodeType: 'play' });
    expect(backend.writes).toEqual([]);
  });

  it('follows a switch flipped in the play viewer without a reload', async () => {
    backend.plays = [play('p', 'Task status')];
    const { container } = render(NavigationSidebar);
    await waitFor(() => expect(rows(container)).toEqual([['Task status', 'On']]));

    // The viewer writes through the node store.
    sharedNodeStore.setNode(play('p', 'Task status', { enabled: false }) as unknown as Node, {
      type: 'database',
      reason: 'test'
    });

    await waitFor(() => expect(rows(container)).toEqual([['Task status', 'Off']]));
  });
});
