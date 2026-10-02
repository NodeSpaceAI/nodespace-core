/**
 * GenericSchemaForm — unsaved placeholder instance.
 *
 * A new instance that is not saved yet marks the required fields still
 * needed, and drops the markers as soon as they are filled.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, waitFor } from '@testing-library/svelte';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: vi.fn().mockResolvedValue({ nodeType: 'widget', groups: [] })
}));

import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { createInstancePlaceholder } from '$lib/services/schema-authoring';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

const schema: SchemaNode = {
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
    field({ name: 'description', friendlyName: 'Description', type: 'text', required: true }),
    field({ name: 'notes', friendlyName: 'Notes', type: 'text' })
  ]
};

describe('GenericSchemaForm — unsaved placeholder', () => {
  beforeEach(() => {
    vi.spyOn(backendAdapter, 'createNode').mockImplementation(async (input) => ({
      id: (input as Node).id,
      placement: null
    }));
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
  });

  it('marks the required fields that are still needed', async () => {
    const node = createInstancePlaceholder(schema);
    render(GenericSchemaForm, { props: { nodeId: node.id, schema, autoOpen: true } });

    await waitFor(() => expect(screen.getByTestId('pending-fields-note')).toBeTruthy());
    expect(screen.getByTestId('pending-fields-note').textContent).toContain('Description');
    expect(screen.getAllByTestId('field-needed')).toHaveLength(1);
  });

  it('drops the markers once the required field is filled and the node is saved', async () => {
    const node = createInstancePlaceholder(schema);
    render(GenericSchemaForm, { props: { nodeId: node.id, schema, autoOpen: true } });
    await waitFor(() => expect(screen.getByTestId('pending-fields-note')).toBeTruthy());

    sharedNodeStore.updateNode(
      node.id,
      { properties: { description: 'Does things' } },
      { type: 'viewer', viewerId: 'test' }
    );

    await waitFor(() => expect(screen.queryByTestId('pending-fields-note')).toBeNull());
    expect(screen.queryAllByTestId('field-needed')).toHaveLength(0);
  });

  it('shows no markers for an ordinary saved node', async () => {
    sharedNodeStore.setNode(
      {
        lifecycleStatus: 'active',
        id: 'saved-1',
        nodeType: 'widget',
        content: '',
        createdAt: '2026-01-01T00:00:00Z',
        modifiedAt: '2026-01-01T00:00:00Z',
        version: 1,
        properties: {},
        mentions: []
      } as Node,
      { type: 'database', reason: 'test' }
    );
    render(GenericSchemaForm, { props: { nodeId: 'saved-1', schema, autoOpen: true } });

    await waitFor(() => expect(screen.getByLabelText('Description')).toBeTruthy());
    expect(screen.queryByTestId('pending-fields-note')).toBeNull();
  });
});
