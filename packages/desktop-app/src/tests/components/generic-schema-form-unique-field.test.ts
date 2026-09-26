/**
 * GenericSchemaForm — the schema-driven `unique` rule (ADR-065).
 *
 * The duplicate suggestion is a property of the field (`unique: true`), not of
 * any one type: a user-defined type that declares a unique field gets the
 * same adopt-existing / keep-as-new suggestion person's email does. Suggest,
 * never block — the field's save is not gated on the lookup.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const navigateToNodeInOtherPane = vi.fn();
vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({ navigateToNodeInOtherPane })
}));

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';

function stringField(name: string, friendlyName: string, opts: Partial<SchemaField> = {}): SchemaField {
  return { name, friendlyName, type: 'string', protection: 'user', indexed: false, required: false, ...opts };
}

/** A user-defined type whose `sku` is unique and whose `notes` is not. */
const PRODUCT_SCHEMA: SchemaNode = {
  id: 'product',
  content: 'Product',
  createdAt: '2026-01-01T00:00:00Z',
  modifiedAt: '2026-01-01T00:00:00Z',
  version: 1,
  isCore: false,
  schemaVersion: 1,
  description: '',
  fields: [
    stringField('sku', 'SKU', { unique: true, uniqueCaseInsensitive: true }),
    stringField('notes', 'Notes')
  ]
} as SchemaNode;

function productNode(overrides: Partial<Node> = {}): Node {
  return {
    id: 'product-1',
    nodeType: 'product',
    content: 'Widget',
    title: 'Widget',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    ...overrides
  } as Node;
}

let updateNodeSpy: ReturnType<typeof vi.fn>;
let findDuplicateForSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(productNode());
  vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof sharedNodeStore.updateNode
  );
  findDuplicateForSpy = vi.fn().mockResolvedValue(null);
  vi.spyOn(backendAdapter, 'findDuplicateFor').mockImplementation(
    findDuplicateForSpy as unknown as typeof backendAdapter.findDuplicateFor
  );
  navigateToNodeInOtherPane.mockReset();
  loadNodeRelationshipsView.mockReset();
  loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'product', groups: [] });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderForm() {
  return render(GenericSchemaForm, {
    props: { nodeId: 'product-1', schema: PRODUCT_SCHEMA, autoOpen: true }
  });
}

async function commit(label: string, value: string) {
  const input = screen.getByLabelText(label);
  await fireEvent.input(input, { target: { value } });
  await fireEvent.blur(input, { target: { value } });
}

describe('GenericSchemaForm — unique field suggestion', () => {
  it('suggests the existing node when a unique field collides on blur', async () => {
    findDuplicateForSpy.mockResolvedValue(productNode({ id: 'product-existing', title: 'Old Widget' }));
    renderForm();

    await commit('SKU', 'W-100');

    expect(findDuplicateForSpy).toHaveBeenCalledWith('product', 'sku', 'W-100', 'product-1');
    await waitFor(() => expect(screen.getByText(/already exists: Old Widget/i)).toBeTruthy());
    // Suggest, never block: the write went through regardless.
    expect(updateNodeSpy).toHaveBeenCalledWith(
      'product-1',
      { properties: { sku: 'W-100' } },
      expect.anything()
    );
  });

  it('"Use existing" opens the match; "Keep as new" just dismisses', async () => {
    findDuplicateForSpy.mockResolvedValue(productNode({ id: 'product-existing', title: 'Old Widget' }));
    renderForm();

    await commit('SKU', 'W-100');
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());
    await fireEvent.click(screen.getByRole('button', { name: 'Keep as new' }));
    expect(screen.queryByText(/already exists/i)).toBeNull();
    expect(navigateToNodeInOtherPane).not.toHaveBeenCalled();

    await commit('SKU', 'W-200');
    await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());
    await fireEvent.click(screen.getByRole('button', { name: 'Use existing' }));
    expect(navigateToNodeInOtherPane).toHaveBeenCalledWith('product-existing');
    expect(screen.queryByText(/already exists/i)).toBeNull();
  });

  it('never looks up a field the schema does not flag unique', async () => {
    renderForm();

    await commit('Notes', 'anything');

    expect(updateNodeSpy).toHaveBeenCalled();
    expect(findDuplicateForSpy).not.toHaveBeenCalled();
  });

  it('ignores an empty value', async () => {
    renderForm();

    await commit('SKU', '');

    expect(findDuplicateForSpy).not.toHaveBeenCalled();
  });
});
