/**
 * ListView — stale-title regression.
 *
 * `sharedNodeStore`'s cached `title` only refreshes via a backend round-trip; optimistic
 * content updates patch `content` and leave `title` untouched. ListView used to render
 * `node.title || node.content`, so a node whose title went stale (e.g. captured while it
 * was still `text`, before conversion to `task`) showed the wrong row text. It must now
 * show current content for non-template types, and still show the computed title for
 * title_template-driven custom entities.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import ListView from '$lib/components/query/list-view.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { pluginRegistry } from '$lib/plugins/index';

function baseNode(overrides: Partial<Node> = {}): Node {
  return {
    lifecycleStatus: 'active',
    id: 'n1',
    nodeType: 'task',
    content: 'current content',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    mentions: [],
    ...overrides
  };
}

describe('ListView — stale title regression', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    // Idempotent no-op if a test didn't register it.
    pluginRegistry.unregister('widget-entity');
  });

  it('shows current content, not a stale cached title, for a non-template type', () => {
    const node = baseNode({ title: '/', content: 'Another Task' });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(ListView, {
      props: { nodeIds: [node.id], onRowClick: () => {} }
    });

    const row = container.querySelector('.list-row');
    expect(row?.textContent?.trim()).toBe('Another Task');
  });

  it('still shows the computed title for a title_template-driven custom entity', () => {
    expect(pluginRegistry.hasPlugin('widget-entity')).toBe(false);
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

    const { container } = render(ListView, {
      props: { nodeIds: [node.id], onRowClick: () => {} }
    });

    const row = container.querySelector('.list-row');
    expect(row?.textContent?.trim()).toBe('Jane Doe');
  });
});
