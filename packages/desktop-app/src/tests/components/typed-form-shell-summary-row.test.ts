/**
 * TypedFormShell — the summary row and the Relationships entry point.
 *
 * The summary row is the collapsible's trigger: "N/M fields | N related nodes"
 * with the chevron at the far edge. The related-node count is what the
 * Relationships modal lists; single-valued relationships promoted to form
 * fields are not counted, and a count that could not be loaded is not shown.
 * The Relationships button is the first thing in the expanded form, above its
 * fields, and opening the modal leaves the form as it was. An edge written
 * elsewhere updates the count without a reload.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
import {
  buildRelationshipsView,
  type RawRelationshipGroup
} from '$lib/services/relationship-grouping';
import { notifyRelationshipChanged } from '$lib/services/relationship-changes';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

const NODE_ID = 'gadget-1';

function schema(): SchemaNode {
  return {
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id: 'gadget',
    content: 'Gadget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    relationships: [],
    fields: []
  };
}

/** An outbound group holding `related` edges. */
function group(
  name: string,
  cardinality: 'one' | 'many',
  related: number
): RawRelationshipGroup {
  return {
    relationshipName: name,
    direction: 'out',
    targetType: 'widget',
    reverseName: `${name}_of`,
    sourceType: 'gadget',
    cardinality,
    farCardinality: 'many',
    required: null,
    edgeFields: null,
    description: null,
    related: Array.from({ length: related }, (_, i) => ({
      id: `${name}-${i}`,
      nodeType: 'widget',
      title: `${name} ${i}`,
      contentPreview: '',
      edgeProperties: {}
    })),
    count: related
  };
}

const view = (groups: RawRelationshipGroup[]) =>
  buildRelationshipsView({ nodeId: NODE_ID, nodeType: 'gadget', groups });

function renderForm() {
  return render(GenericSchemaForm, { props: { nodeId: NODE_ID, schema: schema() } });
}

function relationshipsButton(container: HTMLElement): HTMLElement | null {
  return (
    Array.from(container.querySelectorAll<HTMLElement>('button')).find(
      (button) => button.textContent?.trim() === 'Relationships'
    ) ?? null
  );
}

function trigger(container: HTMLElement): HTMLElement {
  const el = container.querySelector<HTMLElement>('[data-collapsible-trigger]');
  if (!el) throw new Error('No summary row rendered');
  return el;
}

/** The summary row's text, with whitespace collapsed. */
function summary(container: HTMLElement): string {
  return (
    container
      .querySelector('[data-collapsible-trigger]')
      ?.textContent?.replace(/\s+/g, ' ')
      .trim() ?? ''
  );
}

describe('TypedFormShell — summary row and Relationships entry point', () => {
  beforeEach(() => {
    loadNodeRelationshipsView.mockReset();
    vi.spyOn(sharedNodeStore, 'getNode').mockImplementation(
      (id: string) => ({ id, nodeType: 'gadget', content: '', properties: {} }) as never
    );
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('summarizes fields and related nodes, with the chevron at the far edge', async () => {
    // `owner` is single-valued, so it is a form field; its edge is not counted.
    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 1), group('parts', 'many', 2), group('tags', 'many', 1)])
    );
    const { container } = renderForm();

    await waitFor(() => expect(summary(container)).toBe('1/1 fields | 3 related nodes'));
    const row = trigger(container);
    expect(row.classList.contains('schema-form-header')).toBe(true);
    expect(row.lastElementChild?.lastElementChild?.tagName.toLowerCase()).toBe('svg');
  });

  it('puts Relationships in the expanded form, above its fields', async () => {
    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 0), group('parts', 'many', 2)])
    );
    const { container, getByLabelText } = renderForm();

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    const button = relationshipsButton(container)!;
    const content = container.querySelector('[data-collapsible-content]');
    expect(content?.contains(button)).toBe(true);
    expect(trigger(container).contains(button)).toBe(false);

    const field = getByLabelText('Owner');
    expect(
      button.compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
  });

  it('opens the modal without expanding or collapsing the form', async () => {
    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 0), group('parts', 'many', 1)])
    );
    const { container } = renderForm();
    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());

    const row = trigger(container);
    expect(row.getAttribute('aria-expanded')).toBe('false');
    await fireEvent.click(row);
    expect(row.getAttribute('aria-expanded')).toBe('true');

    await fireEvent.click(relationshipsButton(container)!);
    await waitFor(() => expect(document.body.querySelector('[role="dialog"]')).toBeTruthy());
    expect(row.getAttribute('aria-expanded')).toBe('true');
  });

  it('says "1 related node" for a single one, and "0 related nodes" for none', async () => {
    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 0), group('parts', 'many', 1)])
    );
    const first = renderForm();
    await waitFor(() => expect(summary(first.container)).toBe('0/1 fields | 1 related node'));
    first.unmount();

    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 0), group('parts', 'many', 0)])
    );
    const second = renderForm();
    await waitFor(() => expect(summary(second.container)).toBe('0/1 fields | 0 related nodes'));
  });

  it('leaves relationships out of the summary when the modal has nothing to show', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('owner', 'one', 1)]));
    const { container } = renderForm();

    await waitFor(() => expect(summary(container)).toBe('1/1 fields'));
    expect(relationshipsButton(container)).toBeNull();
  });

  it('shows no count when the relationships failed to load, and keeps the button', async () => {
    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    const { container } = renderForm();

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    // No fields and no count: the row names what it opens.
    expect(summary(container)).toBe('Relationships');
  });

  it('drops the count when a later reload fails, since it may be stale', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    const { container } = renderForm();
    await waitFor(() => expect(summary(container)).toBe('2 related nodes'));

    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    notifyRelationshipChanged(NODE_ID, 'parts-1');
    await waitFor(() => expect(summary(container)).toBe('Relationships'));
    expect(relationshipsButton(container)).toBeTruthy();
  });

  it('gives a type with relationships but no fields a summary row of its own', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    const { container } = renderForm();

    await waitFor(() => expect(summary(container)).toBe('2 related nodes'));
    const button = relationshipsButton(container)!;
    expect(container.querySelector('[data-collapsible-content]')?.contains(button)).toBe(true);

    // Expanding it and pressing the button reaches the modal.
    await fireEvent.click(trigger(container));
    await fireEvent.click(button);
    await waitFor(() => expect(document.body.querySelector('[role="dialog"]')).toBeTruthy());
  });

  it('focuses the first field, not the Relationships button, when it opens by itself', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    const withField: SchemaNode = {
      ...schema(),
      fields: [
        {
          name: 'label',
          friendlyName: 'Label',
          type: 'text',
          protection: 'user',
          indexed: false,
          required: false
        }
      ]
    };
    const { container, getByLabelText } = render(GenericSchemaForm, {
      props: { nodeId: NODE_ID, schema: withField, autoOpen: true }
    });

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    await waitFor(() => expect(document.activeElement).toBe(getByLabelText('Label')));
  });

  it('updates the count when an edge of this node changes elsewhere', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 1)]));
    const { container } = renderForm();
    await waitFor(() => expect(summary(container)).toBe('1 related node'));

    // An edge between two other nodes is not this form's concern.
    notifyRelationshipChanged('other-a', 'other-b');
    expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);

    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    notifyRelationshipChanged(NODE_ID, 'parts-1');
    await waitFor(() => expect(summary(container)).toBe('2 related nodes'));
    expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(2);

    // Inbound edges name this node as their far end; a burst is one fetch.
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 0)]));
    notifyRelationshipChanged('parts-0', NODE_ID);
    notifyRelationshipChanged('parts-1', NODE_ID);
    notifyRelationshipChanged(NODE_ID, 'parts-2');
    await waitFor(() => expect(summary(container)).toBe('0 related nodes'));
    expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(3);
  });

  // A reload is scheduled a moment after the event, so these run the clock
  // past that window before counting fetches.
  describe('once the form is gone', () => {
    afterEach(() => vi.useRealTimers());

    it('stops listening', async () => {
      loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 1)]));
      const { container, unmount } = renderForm();
      await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());

      vi.useFakeTimers();
      unmount();
      notifyRelationshipChanged(NODE_ID, 'parts-1');
      await vi.advanceTimersByTimeAsync(1000);
      expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);
    });

    it('drops a reload it had already scheduled', async () => {
      loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 1)]));
      const { container, unmount } = renderForm();
      await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());

      vi.useFakeTimers();
      notifyRelationshipChanged(NODE_ID, 'parts-1');
      unmount();
      await vi.advanceTimersByTimeAsync(1000);
      expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);
    });
  });
});
