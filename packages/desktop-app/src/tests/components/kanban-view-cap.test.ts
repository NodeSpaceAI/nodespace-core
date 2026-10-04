/**
 * KanbanView — per-column render cap.
 *
 * kanban-view.svelte buckets and rendered ALL matching nodes per column with
 * no bound, while List/Table paginate at PAGE_SIZE = 25. These tests cover the fix: each column renders
 * at most a batch of cards, with a "+N more" control that grows the *set* of
 * revealed cards (tracked by id, not position) — a card already on screen
 * can't disappear because a different card's bucket membership shifted
 * elsewhere in the result order, which a plain positional cutoff can't
 * guarantee (see "keeps an already-visible card visible" below).
 *
 * DnD, rollback, and keyboard-move coverage live under the browser tier
 * (src/tests/browser/kanban-dnd.test.ts) — this file only covers the cap
 * itself, which is plain click-driven rendering Happy-DOM handles fine.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import KanbanView from '$lib/components/query/kanban-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

function schema(): SchemaNode {
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

function makeNodes(count: number): Node[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `n${i}`,
    lifecycleStatus: 'active' as const,
    nodeType: 'widget',
    content: `Card ${i}`,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { status: 'open' },
    mentions: []
  }));
}

function seed(nodes: Node[]): void {
  for (const n of nodes) sharedNodeStore.setNode(n, { type: 'database', reason: 'seed' });
}

describe('KanbanView — per-column cap', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
  });

  it('caps a large column at the first batch and shows a +N more control', () => {
    const nodes = makeNodes(40);
    seed(nodes);

    const { container, getByText } = render(KanbanView, {
      props: {
        nodeIds: nodes.map((n) => n.id),
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    expect(container.querySelectorAll('.kanban-card').length).toBe(25);
    expect(getByText('+15 more')).toBeTruthy();
  });

  it('grows the visible count on "+N more" click without removing already-shown cards', async () => {
    const nodes = makeNodes(40);
    seed(nodes);

    const { container, getByText } = render(KanbanView, {
      props: {
        nodeIds: nodes.map((n) => n.id),
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const initiallyShown = new Set(
      Array.from(container.querySelectorAll('.kanban-card-title')).map((el) => el.textContent?.trim())
    );

    await fireEvent.click(getByText('+15 more'));

    const nowShown = Array.from(container.querySelectorAll('.kanban-card-title')).map((el) =>
      el.textContent?.trim()
    );
    expect(nowShown.length).toBe(40);
    // Every card visible before the click is still visible after it.
    for (const title of initiallyShown) {
      expect(nowShown).toContain(title);
    }
    expect(container.querySelector('.kanban-show-more')).toBeNull();
  });

  it('does not show a cap control for a column at or under the batch size', () => {
    const nodes = makeNodes(10);
    seed(nodes);

    const { container } = render(KanbanView, {
      props: {
        nodeIds: nodes.map((n) => n.id),
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    expect(container.querySelectorAll('.kanban-card').length).toBe(10);
    expect(container.querySelector('.kanban-show-more')).toBeNull();
  });

  it('caps independently per column', () => {
    const openNodes = makeNodes(30);
    const closedNodes = Array.from({ length: 5 }, (_, i) => ({
      id: `closed-${i}`,
      nodeType: 'widget',
      content: `Closed ${i}`,
      lifecycleStatus: 'active' as const,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      version: 1,
      properties: { status: 'unassigned-column-probe' },
      mentions: []
    }));
    seed([...openNodes, ...closedNodes]);

    const { container, getByText } = render(KanbanView, {
      props: {
        nodeIds: [...openNodes, ...closedNodes].map((n) => n.id),
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    // Open column (30 nodes) is capped; Unassigned (the 5 nodes with an
    // unrecognized status) is not.
    expect(getByText('+5 more')).toBeTruthy();
    expect(container.querySelectorAll('.kanban-card').length).toBe(25 + 5);
  });

  it('keeps an already-visible card visible when a different card joins the column ahead of it', async () => {
    // n1..n25 (25 nodes) start "open" — exactly at the cap, all visible.
    // n0 starts "closed" (out of the Open bucket), positioned FIRST in
    // nodeIds. A positional cutoff (`ids.slice(0, 25)`) would, once n0 joins
    // Open, place n0 first and push the *last* already-visible card (n25)
    // out — even though n25's own bucket membership never changed. The fix
    // must keep n25 visible and treat n0 (the actual new arrival) as hidden.
    const openNodes = makeNodes(25).map((n, i) => ({ ...n, id: `open-${i}` }));
    const outsider: Node = {
      lifecycleStatus: 'active',
      id: 'outsider',
      nodeType: 'widget',
      content: 'Outsider',
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      version: 1,
      properties: { status: 'unassigned-column-probe' },
      mentions: []
    };
    seed([outsider, ...openNodes]);

    const { container, getByText } = render(KanbanView, {
      props: {
        // `outsider` sorts FIRST — its later move into Open must not
        // displace any of the already-visible open-* cards.
        nodeIds: [outsider.id, ...openNodes.map((n) => n.id)],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    // All 25 "open" cards visible at the cap (Open itself shows no "+more"
    // yet); "outsider" sits alone, uncapped, in Unassigned.
    expect(container.querySelectorAll('.kanban-card').length).toBe(25 + 1);
    expect(container.querySelector('.kanban-show-more')).toBeNull();
    const lastCardTitle = openNodes[openNodes.length - 1].content;
    expect(getByText(lastCardTitle)).toBeTruthy();

    // `outsider` moves into Open, sorting ahead of every open-* card.
    sharedNodeStore.updateNode(
      outsider.id,
      { properties: { status: 'open' } },
      { type: 'viewer', viewerId: 'test' },
      { skipPersistence: true }
    );
    await Promise.resolve();

    // The already-visible last card is still visible — it did not get
    // pushed out by outsider's insertion ahead of it.
    expect(getByText(lastCardTitle)).toBeTruthy();
    // outsider itself — the actual new arrival — is the one hidden behind
    // the cap control instead.
    expect(getByText('+1 more')).toBeTruthy();
    expect(container.querySelectorAll('.kanban-card').length).toBe(25);
  });

  it('reveals a card moved (via the keyboard menu) into an already-oversized column immediately', async () => {
    const schemaWithTwoColumns: SchemaNode = {
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
          userValues: []
        }
      ]
    };
    // Open is already over the cap (30 members, 25 shown); mover starts
    // Closed, alone.
    const openNodes = makeNodes(30);
    const mover: Node = {
      lifecycleStatus: 'active',
      id: 'mover',
      nodeType: 'widget',
      content: 'Mover',
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      version: 1,
      properties: { status: 'closed' },
      mentions: []
    };
    seed([...openNodes, mover]);

    const { getByText, getByRole } = render(KanbanView, {
      props: {
        nodeIds: [...openNodes.map((n) => n.id), mover.id],
        schema: schemaWithTwoColumns,
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    expect(getByText('+5 more')).toBeTruthy(); // Open: 30 total, 25 shown

    await fireEvent.keyDown(getByRole('button', { name: 'Mover' }), { key: 'm' });
    await fireEvent.click(getByRole('menuitem', { name: 'Open' }));

    // The card the user just placed into Open is visible immediately — not
    // waiting behind a "+6 more" nobody clicked.
    expect(getByText('Mover')).toBeTruthy();
  });
});
