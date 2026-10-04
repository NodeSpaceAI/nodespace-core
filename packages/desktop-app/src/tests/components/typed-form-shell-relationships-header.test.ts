/**
 * TypedFormShell — the Relationships entry point in the form's header row.
 *
 * The button sits at the right of the header row, beside the collapsible's
 * trigger and never inside it, so opening the modal leaves the form as it was.
 * It carries the number of related nodes the modal lists; single-valued
 * relationships promoted to form fields are not counted, and a count that is
 * zero or could not be loaded is not shown. An edge written elsewhere updates
 * the count without a reload.
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
    Array.from(container.querySelectorAll<HTMLElement>('button')).find((button) =>
      button.textContent?.trim().startsWith('Relationships')
    ) ?? null
  );
}

describe('TypedFormShell — Relationships in the header row', () => {
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

  it('sits in the header row beside the trigger, counting only what the modal lists', async () => {
    // `owner` is single-valued, so it is a form field; its edge is not counted.
    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 1), group('parts', 'many', 2), group('tags', 'many', 1)])
    );
    const { container } = renderForm();

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    const button = relationshipsButton(container)!;
    expect(button.textContent?.trim()).toBe('Relationships (3)');

    const header = container.querySelector('.schema-form-header');
    const trigger = container.querySelector('[data-collapsible-trigger]');
    expect(header?.contains(button)).toBe(true);
    expect(trigger).toBeTruthy();
    expect(trigger?.contains(button)).toBe(false);
    expect(header?.lastElementChild).toBe(button);
  });

  it('opens the modal without expanding or collapsing the form', async () => {
    loadNodeRelationshipsView.mockResolvedValue(
      view([group('owner', 'one', 0), group('parts', 'many', 1)])
    );
    const { container } = renderForm();
    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());

    const trigger = container.querySelector('[data-collapsible-trigger]')!;
    expect(trigger.getAttribute('aria-expanded')).toBe('false');

    await fireEvent.click(relationshipsButton(container)!);
    await waitFor(() => expect(document.body.querySelector('[role="dialog"]')).toBeTruthy());
    expect(trigger.getAttribute('aria-expanded')).toBe('false');

    // And the trigger still toggles the form on its own.
    await fireEvent.click(trigger);
    expect(trigger.getAttribute('aria-expanded')).toBe('true');
  });

  it('shows no number when the modal lists no related nodes', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 0)]));
    const { container } = renderForm();

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    expect(relationshipsButton(container)!.textContent?.trim()).toBe('Relationships');
  });

  it('shows no number when the relationships failed to load', async () => {
    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    const { container } = renderForm();

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    expect(relationshipsButton(container)!.textContent?.trim()).toBe('Relationships');
  });

  it('drops the number when a later reload fails, since it may be stale', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    const { container } = renderForm();
    await waitFor(() =>
      expect(relationshipsButton(container)?.textContent?.trim()).toBe('Relationships (2)')
    );

    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    notifyRelationshipChanged(NODE_ID, 'parts-1');
    await waitFor(() =>
      expect(relationshipsButton(container)?.textContent?.trim()).toBe('Relationships')
    );
  });

  it('keeps a header row for a type with relationships but no fields', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    const { container } = renderForm();

    await waitFor(() => expect(relationshipsButton(container)).toBeTruthy());
    const button = relationshipsButton(container)!;
    expect(button.textContent?.trim()).toBe('Relationships (2)');
    expect(container.querySelector('.schema-form-header')?.contains(button)).toBe(true);
    expect(container.querySelector('[data-collapsible-trigger]')).toBeNull();
  });

  it('updates the count when an edge of this node changes elsewhere', async () => {
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 1)]));
    const { container } = renderForm();
    await waitFor(() =>
      expect(relationshipsButton(container)?.textContent?.trim()).toBe('Relationships (1)')
    );

    // An edge between two other nodes is not this form's concern.
    notifyRelationshipChanged('other-a', 'other-b');
    expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(1);

    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 2)]));
    notifyRelationshipChanged(NODE_ID, 'parts-1');
    await waitFor(() =>
      expect(relationshipsButton(container)?.textContent?.trim()).toBe('Relationships (2)')
    );

    expect(loadNodeRelationshipsView).toHaveBeenCalledTimes(2);

    // Inbound edges name this node as their far end; a burst is one fetch.
    loadNodeRelationshipsView.mockResolvedValue(view([group('parts', 'many', 0)]));
    notifyRelationshipChanged('parts-0', NODE_ID);
    notifyRelationshipChanged('parts-1', NODE_ID);
    notifyRelationshipChanged(NODE_ID, 'parts-2');
    await waitFor(() =>
      expect(relationshipsButton(container)?.textContent?.trim()).toBe('Relationships')
    );
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
