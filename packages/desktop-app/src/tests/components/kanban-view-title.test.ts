/**
 * KanbanView — stale-title regression.
 *
 * See list-view.test.ts for the full root-cause explanation. KanbanView's `titleOf` used
 * `node.title || node.content`, so a card for a node whose title went stale showed the wrong
 * text. It must now show current content for non-template types, and still show the computed
 * title for title_template-driven custom entities.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import KanbanView from '$lib/components/query/kanban-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { pluginRegistry } from '$lib/plugins/index';

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

function baseNode(overrides: Partial<Node> = {}): Node {
  return {
    lifecycleStatus: 'active',
    id: 'n1',
    nodeType: 'task',
    content: 'current content',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: { status: 'open' },
    mentions: [],
    ...overrides
  };
}

describe('KanbanView — stale title regression', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    pluginRegistry.unregister('widget-entity');
  });

  it('shows current content, not a stale cached title, for a non-template type', () => {
    const node = baseNode({ title: '/', content: 'Another Task' });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(KanbanView, {
      props: {
        nodeIds: [node.id],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const card = container.querySelector('.kanban-card-title');
    expect(card?.textContent?.trim()).toBe('Another Task');
  });

  it('still shows the computed title for a title_template-driven custom entity', () => {
    pluginRegistry.register({
      id: 'widget-entity',
      name: 'Widget',
      description: 'Custom entity with a title template',
      version: '1.0.0',
      config: { slashCommands: [] },
      hasTitleTemplate: true,
      titleTemplate: '{first_name} {last_name}'
    });

    const node = baseNode({ nodeType: 'widget-entity', content: 'raw', title: 'Jane Doe' });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(KanbanView, {
      props: {
        nodeIds: [node.id],
        schema: schema(),
        groupBy: 'status',
        onGroupByChange: () => {},
        onRowClick: () => {}
      }
    });

    const card = container.querySelector('.kanban-card-title');
    expect(card?.textContent?.trim()).toBe('Jane Doe');
  });
});
