/**
 * BaseNodeViewer — an expanded schema form is capped and scrolls inside itself.
 *
 * The form sits between the viewer's header and its children area. Expanded, it
 * takes its own height up to a share of the viewer, keeps its header row in
 * place and scrolls its fields; the children area below never drops under its
 * minimum height. Heights, overflow and scroll positions are layout, which
 * Happy-DOM does not compute, so this needs a real browser.
 *
 * The viewed node is a person: its form is hardcoded and opens by itself.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import '../../app.css';
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/services/relationship-viewer-service')>()),
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import BaseNodeViewerInContext from '../fixtures/base-node-viewer-in-context.svelte';
import { buildRelationshipsView } from '$lib/services/relationship-grouping';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { pluginRegistry } from '$lib/plugins/index';
import { registerCorePlugins } from '$lib/plugins/core-plugins';
import type { Node } from '$lib/types';

// Browser mode's setup does not register core plugins, and the viewer finds
// the person form through its plugin.
if (!pluginRegistry.hasPlugin('person')) {
  registerCorePlugins(pluginRegistry);
}

const PERSON_ID = '0c6f1f0e-58c1-4d0c-9a53-3f0f8f1c2a77';

function seedPerson(): void {
  sharedNodeStore.setNode(
    {
      id: PERSON_ID,
      nodeType: 'person',
      content: '',
      title: 'Ada Lovelace',
      version: 1,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-01T00:00:00Z',
      properties: { person: { first_name: 'Ada', last_name: 'Lovelace' } },
      lifecycleStatus: 'active',
      mentions: []
    } as unknown as Node,
    { type: 'database', reason: 'seed' }
  );
}

interface Parts {
  host: HTMLElement;
  form: HTMLElement;
  header: HTMLElement;
  scroll: HTMLElement;
  children: HTMLElement;
}

/** Mount the viewer in a host of the given height and wait for the open form. */
async function mountViewer(height: number): Promise<Parts> {
  const host = document.createElement('div');
  host.style.width = '640px';
  host.style.height = `${height}px`;
  document.body.appendChild(host);
  render(BaseNodeViewerInContext, { target: host, props: { nodeId: PERSON_ID } });

  const find = <T extends HTMLElement>(selector: string) => host.querySelector<T>(selector);
  await waitFor(() => {
    expect(find('.schema-form-scroll input')).toBeTruthy();
  });
  const form = find('.schema-form-container');
  const header = find('.schema-form-header');
  const scroll = find('.schema-form-scroll');
  const children = find('.node-content-area');
  if (!form || !header || !scroll || !children) throw new Error('Viewer parts not rendered');
  return { host, form, header, scroll, children };
}

function rem(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize);
}

describe('BaseNodeViewer — schema form height and scroll (browser mode)', () => {
  let host: HTMLElement | undefined;

  beforeEach(() => {
    sharedNodeStore.clearAll();
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')));
    loadNodeRelationshipsView.mockResolvedValue(
      buildRelationshipsView({ nodeId: PERSON_ID, nodeType: 'person', groups: [] })
    );
    seedPerson();
  });

  afterEach(() => {
    cleanup();
    host?.remove();
    host = undefined;
    structureTree.removeNode(PERSON_ID);
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('gives a form that fits only the height it needs, with nothing to scroll', async () => {
    const parts = await mountViewer(1200);
    host = parts.host;

    expect(parts.scroll.scrollHeight).toBeLessThanOrEqual(parts.scroll.clientHeight);
    expect(parts.form.getBoundingClientRect().height).toBeLessThan(600);
    // No empty space: the form ends where its fields do, plus its border.
    expect(
      parts.form.getBoundingClientRect().bottom - parts.scroll.getBoundingClientRect().bottom
    ).toBeLessThanOrEqual(2);
  });

  it('caps a long form and scrolls its fields under a fixed header row', async () => {
    const viewerHeight = 360;
    const parts = await mountViewer(viewerHeight);
    host = parts.host;
    const { form, header, scroll, children } = parts;

    // Capped at half the viewer, with the overflow scrollable inside the form.
    expect(form.getBoundingClientRect().height).toBeLessThanOrEqual(viewerHeight / 2 + 1);
    expect(scroll.scrollHeight).toBeGreaterThan(scroll.clientHeight);
    expect(getComputedStyle(scroll).overflowY).toBe('auto');

    // The children area keeps the rest of the viewer.
    expect(children.getBoundingClientRect().height).toBeGreaterThanOrEqual(8 * rem() - 1);
    expect(children.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      parts.host.getBoundingClientRect().bottom + 1
    );

    // Scrolling the fields leaves the header row where it was.
    const headerTop = header.getBoundingClientRect().top;
    scroll.scrollTop = scroll.scrollHeight;
    expect(scroll.scrollTop).toBeGreaterThan(0);
    expect(header.getBoundingClientRect().top).toBeCloseTo(headerTop, 0);
  });

  it('scrolls a focused control into view inside the form', async () => {
    const parts = await mountViewer(360);
    host = parts.host;
    const { scroll } = parts;

    const controls = scroll.querySelectorAll<HTMLElement>('input, select, textarea');
    const last = controls[controls.length - 1];
    scroll.scrollTop = 0;
    const before = last.getBoundingClientRect();
    expect(before.bottom).toBeGreaterThan(scroll.getBoundingClientRect().bottom);

    last.focus();

    const region = scroll.getBoundingClientRect();
    const after = last.getBoundingClientRect();
    expect(after.top).toBeGreaterThanOrEqual(region.top - 1);
    expect(after.bottom).toBeLessThanOrEqual(region.bottom + 1);
  });

  it('keeps the children area at its minimum height on a short window', async () => {
    const parts = await mountViewer(260);
    host = parts.host;
    const { form, header, scroll, children } = parts;

    expect(children.getBoundingClientRect().height).toBeGreaterThanOrEqual(8 * rem() - 1);
    // The form gave up the height: it is under its cap and still scrolls.
    expect(form.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      children.getBoundingClientRect().top + 1
    );
    expect(header.getBoundingClientRect().height).toBeGreaterThan(0);
    expect(header.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      form.getBoundingClientRect().bottom + 1
    );
    expect(scroll.scrollHeight).toBeGreaterThan(scroll.clientHeight);
  });
});
