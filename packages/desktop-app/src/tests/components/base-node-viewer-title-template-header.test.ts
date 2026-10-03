/**
 * Opening a persisted person shows its computed `title` in the viewer header on first
 * open, with no field edit. The header reads `hasTitleTemplate` on its first render,
 * before the viewer has loaded the type's form; a person's template is declared on its
 * plugin, so the loader's current type is the only thing that changes when the form
 * loads, and the header must follow it.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import BaseNodeViewerInContext from '../fixtures/base-node-viewer-in-context.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { Node } from '$lib/types/node';

const PERSON_ID = 'title-header-person';
const PAGE_ID = 'title-header-page';
const databaseSource = { type: 'database' as const, reason: 'test' };

function seed(node: Partial<Node> & { id: string; nodeType: string }): void {
  sharedNodeStore.setNode(
    {
      content: '',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: {},
      lifecycleStatus: 'active',
      mentions: [],
      ...node
    } as unknown as Node,
    databaseSource
  );
}

function header(container: HTMLElement): HTMLInputElement {
  const input = container.querySelector<HTMLInputElement>('input[aria-label="Page title"]');
  if (!input) throw new Error('viewer header not rendered');
  return input;
}

describe('BaseNodeViewer header for a title-template type', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
  });

  afterEach(() => {
    cleanup();
    for (const id of [PERSON_ID, PAGE_ID]) structureTree.removeNode(id);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("shows a persisted person's stored title on first open, read-only", async () => {
    seed({
      id: PERSON_ID,
      nodeType: 'person',
      title: 'Ada Lovelace',
      properties: { person: { first_name: 'Ada', last_name: 'Lovelace' } }
    } as Partial<Node> & { id: string; nodeType: string });

    const { container } = render(BaseNodeViewerInContext, { props: { nodeId: PERSON_ID } });

    await waitFor(() => expect(header(container).value).toBe('Ada Lovelace'));
    expect(header(container).readOnly).toBe(true);
  });

  it('shows content for a type without a title template', async () => {
    seed({ id: PAGE_ID, nodeType: 'text', content: 'A plain page' });

    const { container } = render(BaseNodeViewerInContext, { props: { nodeId: PAGE_ID } });

    await waitFor(() => expect(header(container).value).toBe('A plain page'));
    expect(header(container).readOnly).toBe(false);
  });
});
