/**
 * Property-form field dispatch — one shared editor, two form implementations.
 *
 * GenericSchemaForm and TaskSchemaForm render the same leaf controls and open
 * the SAME nested (object/array) editor modal. Both read and write fields flat,
 * as properties[<field>] — the shape every transport delivers; the backend
 * re-buckets bare keys under the node's type in storage.
 *
 * The modal therefore owns no persistence: each form supplies the current value
 * and the write. These tests drive a real edit through each form's modal and
 * assert the rebuilt value lands at the right path without dropping siblings.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

// GenericSchemaForm gates its Relationships trigger on this service; stub it so
// the gate never reaches a daemon. It is a plain function export, not a singleton.
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: vi.fn().mockResolvedValue({ nodeType: 'gadget', groups: [] })
}));

import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
import TaskSchemaForm from '$lib/components/property-forms/task-schema-form.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { loadNodeRelationshipsView } from '$lib/services/relationship-viewer-service';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

// One object field with a single string sub-field: enough to make an edit and
// observe the whole rebuilt object being persisted.
const ADDRESS_FIELD = field({
  name: 'address',
  type: 'object',
  fields: [field({ name: 'street', friendlyName: 'Street', type: 'string' })]
});

function schemaWith(fields: SchemaField[], nodeType: string): SchemaNode {
  return {
    id: `schema-${nodeType}`,
    content: nodeType,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    fields
  };
}

function nodeWith(nodeType: string, properties: Record<string, unknown>): Node {
  return {
    id: 'node-1',
    nodeType,
    content: 'A node',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties
  } as Node;
}

/** The `properties` bag of the single updateNode call under test. */
function persistedProperties(updateNode: ReturnType<typeof vi.fn>): Record<string, unknown> {
  expect(updateNode).toHaveBeenCalledTimes(1);
  const changes = updateNode.mock.calls[0][1] as Partial<Node>;
  return changes.properties as Record<string, unknown>;
}

/**
 * Open the nested editor for the only nested field on screen and type into its
 * one string sub-field, returning once the edit has been emitted. The modal is
 * portalled to <body>, so this queries via `screen` rather than the container.
 */
async function editStreetThroughModal(): Promise<void> {
  // The nested field's trigger summarises an empty object as "0 fields".
  await fireEvent.click(screen.getByText('0 fields'));
  await waitFor(() => expect(screen.getByLabelText('Street')).toBeTruthy());
  await fireEvent.input(screen.getByLabelText('Street'), { target: { value: '1 Main' } });
}

let updateNodeSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  // Spy (never module-mock) the shared singletons so nothing leaks across the fork.
  updateNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof sharedNodeStore.updateNode
  );

  // Re-arm every test: `vi.restoreAllMocks()` below calls `.mockRestore()` on this
  // `vi.fn()` too, which — unlike a `vi.spyOn` of a real method — clears its
  // implementation entirely rather than restoring one. Without re-arming here, only
  // the FIRST test in the file to render GenericSchemaForm gets a resolved value;
  // every later one sees the bare `vi.fn()` return `undefined` and throws calling
  // `.then()` on it inside the component's relationships-gate effect.
  vi.mocked(loadNodeRelationshipsView).mockResolvedValue({
    nodeId: 'node-1',
    nodeType: 'gadget',
    groups: [],
    isEmpty: true
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('GenericSchemaForm — nested values persist FLAT', () => {
  beforeEach(() => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith('gadget', { unrelated: 'keep me' })
    );
  });

  it('writes properties[<field>] and leaves other properties intact', async () => {
    render(GenericSchemaForm, {
      props: { nodeId: 'node-1', schema: schemaWith([ADDRESS_FIELD], 'gadget'), autoOpen: true }
    });

    await editStreetThroughModal();

    expect(persistedProperties(updateNodeSpy)).toEqual({
      unrelated: 'keep me',
      address: { street: '1 Main' }
    });
  });
});

describe('TaskSchemaForm — nested values persist FLAT', () => {
  beforeEach(() => {
    vi.spyOn(backendAdapter, 'getSchema').mockResolvedValue(
      schemaWith([ADDRESS_FIELD], 'task') as never
    );
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith('task', { estimate: 3 })
    );
  });

  it('writes properties[<field>] and preserves sibling task fields', async () => {
    const { container } = render(TaskSchemaForm, { props: { nodeId: 'node-1' } });

    // The form's Collapsible starts collapsed; its trigger is the first button.
    await waitFor(() => expect(container.querySelector('button')).toBeTruthy());
    await fireEvent.click(container.querySelector('button')!);
    await editStreetThroughModal();

    expect(persistedProperties(updateNodeSpy)).toEqual({
      estimate: 3,
      address: { street: '1 Main' }
    });
  });
});

describe('GenericSchemaForm — existing nested values', () => {
  beforeEach(() => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith('invoice', { address: { street: 'A', city: 'B' }, total: 10 })
    );
  });

  it('shows an existing nested value instead of an empty editor', async () => {
    render(GenericSchemaForm, {
      props: { nodeId: 'node-1', schema: schemaWith([ADDRESS_FIELD], 'invoice'), autoOpen: true }
    });

    await waitFor(() => expect(screen.getByText('2 fields')).toBeTruthy());
  });

  it('does not destroy sibling keys the user was never shown', async () => {
    render(GenericSchemaForm, {
      props: { nodeId: 'node-1', schema: schemaWith([ADDRESS_FIELD], 'invoice'), autoOpen: true }
    });

    await fireEvent.click(screen.getByText('2 fields'));
    await waitFor(() => expect(screen.getByLabelText('Street')).toBeTruthy());
    await fireEvent.input(screen.getByLabelText('Street'), { target: { value: 'X' } });

    // `city` was never rendered (it is not a declared sub-field) but must survive the
    // write — NestedPropertyModal rebuilds the whole object, not just the edited key.
    expect(persistedProperties(updateNodeSpy)).toEqual({
      total: 10,
      address: { street: 'X', city: 'B' }
    });
  });
});

describe('GenericSchemaForm — boolean fields', () => {
  beforeEach(() => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith('invoice', { total: 10 })
    );
  });

  it('renders a checkbox for a boolean field', async () => {
    render(GenericSchemaForm, {
      props: {
        nodeId: 'node-1',
        schema: schemaWith([field({ name: 'paid', friendlyName: 'Paid', type: 'boolean' })], 'invoice'),
        autoOpen: true
      }
    });

    const checkbox = (await waitFor(() =>
      screen.getByLabelText('Paid')
    )) as HTMLInputElement;
    expect(checkbox.type).toBe('checkbox');
    expect(checkbox.checked).toBe(false);
  });

  it('persists the toggled value flat, preserving siblings', async () => {
    render(GenericSchemaForm, {
      props: {
        nodeId: 'node-1',
        schema: schemaWith([field({ name: 'paid', friendlyName: 'Paid', type: 'boolean' })], 'invoice'),
        autoOpen: true
      }
    });

    await waitFor(() => expect(screen.getByLabelText('Paid')).toBeTruthy());
    await fireEvent.click(screen.getByLabelText('Paid'));

    expect(persistedProperties(updateNodeSpy)).toEqual({ total: 10, paid: true });
  });
});
