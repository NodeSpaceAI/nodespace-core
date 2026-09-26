/**
 * GenericSchemaForm — Relationships trigger visibility gate.
 *
 * The Relationships entry point must render only when the modal has something
 * to show: a populated group, or an outbound one it can offer to add. A schema
 * with no relationships in either direction hides the button, and so does one
 * whose only relationships are single-valued — those render as form fields
 * instead. A transient load error fails open (button shown) rather than hiding
 * a real feature.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';
import type { SchemaNode } from '$lib/types/schema-node';

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
import { buildRelationshipsView } from '$lib/services/relationship-grouping';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

function schema(): SchemaNode {
  return {
    id: 'gadget',
    content: 'Gadget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    fields: []
  };
}

const groupsResult = (n: number, cardinality: 'one' | 'many' = 'many') =>
  buildRelationshipsView({
    nodeId: 'n',
    nodeType: 'gadget',
    groups: Array.from({ length: n }, (_, i) => ({
      relationshipName: `rel_${i}`,
      direction: 'out' as const,
      targetType: 'widget',
      reverseName: `gadgets_${i}`,
      sourceType: 'gadget',
      cardinality,
      farCardinality: 'many' as const,
      required: null,
      edgeFields: null,
      description: null,
      related: [],
      count: 0
    }))
  });

describe('GenericSchemaForm — Relationships trigger gate', () => {
  beforeEach(() => {
    loadNodeRelationshipsView.mockReset();
    // Spy (don't module-mock) the shared singleton so this never leaks into other
    // suites in the same fork: the form only renders its body when a node resolves
    // (`{#if node}`), so return a minimal instance to make the gate reachable.
    vi.spyOn(sharedNodeStore, 'getNode').mockImplementation(
      (id: string) => ({ id, nodeType: 'gadget', content: '', properties: {} }) as never
    );
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('shows the Relationships trigger when the type has typed relationships', async () => {
    loadNodeRelationshipsView.mockResolvedValue(groupsResult(1));
    const { queryByText } = render(GenericSchemaForm, {
      props: { nodeId: 'n1', schema: schema(), autoOpen: true }
    });
    await waitFor(() => expect(queryByText('Relationships')).toBeTruthy());
    expect(loadNodeRelationshipsView).toHaveBeenCalledWith('n1');
  });

  it('hides the Relationships trigger when the type has none in either direction', async () => {
    loadNodeRelationshipsView.mockResolvedValue(groupsResult(0));
    const { queryByText } = render(GenericSchemaForm, {
      props: { nodeId: 'n2', schema: schema(), autoOpen: true }
    });
    // Give the resolved gate a chance to settle, then assert it stays hidden.
    await waitFor(() => expect(loadNodeRelationshipsView).toHaveBeenCalledWith('n2'));
    await Promise.resolve();
    expect(queryByText('Relationships')).toBeNull();
  });

  it('renders a single-valued relationship as a field, and hides a trigger left with nothing', async () => {
    loadNodeRelationshipsView.mockResolvedValue(groupsResult(1, 'one'));
    const { queryByText, getByLabelText } = render(GenericSchemaForm, {
      props: { nodeId: 'n4', schema: schema(), autoOpen: true }
    });
    // A schema with no fields of its own still gets the form, for the field.
    await waitFor(() => expect(getByLabelText('Rel 0')).toBeTruthy());
    expect(queryByText('0/1 fields')).toBeTruthy();
    expect(queryByText('Relationships')).toBeNull();
  });

  it('fails open (shows the trigger) when the relationship check errors', async () => {
    loadNodeRelationshipsView.mockRejectedValue(new Error('daemon offline'));
    const { queryByText } = render(GenericSchemaForm, {
      props: { nodeId: 'n3', schema: schema(), autoOpen: true }
    });
    await waitFor(() => expect(queryByText('Relationships')).toBeTruthy());
  });
});
