/**
 * GenericSchemaForm — protection-level filtering.
 *
 * GenericSchemaForm used to iterate `schema.fields` unconditionally, with no
 * filter for `protection: 'system'` fields. Not reachable in production today
 * (the only core types with system-protected fields — `ai-chat`, `collection` —
 * both bypass the generic-form branch via dedicated viewers), but a real gap:
 * if any type with a system field were ever rendered through generic
 * per-field iteration without this filter, the system-managed field would
 * render as a raw editable control.
 *
 * These tests use a synthetic system-protected field to pin the exact
 * regression this closes, and confirm `core`/`user` fields on the same schema
 * are unaffected.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, waitFor } from '@testing-library/svelte';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

const loadNodeRelationshipsView = vi.fn();
vi.mock('$lib/services/relationship-viewer-service', () => ({
  loadNodeRelationshipsView: (...args: unknown[]) => loadNodeRelationshipsView(...args)
}));

import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

/** A generic System-protected field, shaped like a typical internal marker. */
const SYSTEM_MARKER_FIELD = field({
  name: '_synthetic_system_field',
  friendlyName: 'Synthetic system field',
  type: 'boolean',
  protection: 'system',
  default: false
});

const NAME_FIELD = field({ name: 'name', friendlyName: 'Name', type: 'string', protection: 'core' });
const EMAIL_FIELD = field({ name: 'email', friendlyName: 'Email', type: 'string', protection: 'core' });

function schemaWith(fields: SchemaField[]): SchemaNode {
  return {
    id: 'person-like',
    content: 'PersonLike',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore: false,
    schemaVersion: 1,
    fields
  };
}

function nodeWith(properties: Record<string, unknown>): Node {
  return {
    id: 'node-1',
    nodeType: 'person-like',
    content: 'A node',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties
  } as Node;
}

let updateNodeSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof sharedNodeStore.updateNode
  );
  loadNodeRelationshipsView.mockReset();
  loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'person-like', groups: [] });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('GenericSchemaForm — protection-level filtering', () => {
  it('never renders a system-protected field as an editable control', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith({ 'person-like': { name: 'Alice', email: 'alice@example.com', _synthetic_system_field: true } })
    );
    render(GenericSchemaForm, {
      props: {
        nodeId: 'node-1',
        schema: schemaWith([NAME_FIELD, EMAIL_FIELD, SYSTEM_MARKER_FIELD]),
        autoOpen: true
      }
    });

    await waitFor(() => expect(screen.getByLabelText('Name')).toBeTruthy());
    expect(screen.getByLabelText('Email')).toBeTruthy();
    // No control, no label — the system field is excluded entirely, not merely
    // rendered read-only.
    expect(screen.queryByLabelText('Synthetic system field')).toBeNull();
    expect(screen.queryByText('Synthetic system field')).toBeNull();
  });

  it('excludes the system field from the filled/total field-count badge', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith({ 'person-like': { name: 'Alice', _synthetic_system_field: true } })
    );
    render(GenericSchemaForm, {
      props: {
        nodeId: 'node-1',
        schema: schemaWith([NAME_FIELD, EMAIL_FIELD, SYSTEM_MARKER_FIELD]),
        autoOpen: true
      }
    });

    // 2 visible fields (name, email), 1 filled (name) — _synthetic_system_field counts
    // toward neither the numerator nor the denominator despite being `true` and
    // present on the node.
    await waitFor(() => expect(screen.getByText('1/2 fields')).toBeTruthy());
  });

  it('renders every non-system field normally, unaffected by the filter', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith({ 'person-like': { name: 'Alice', email: 'alice@example.com' } })
    );
    render(GenericSchemaForm, {
      props: { nodeId: 'node-1', schema: schemaWith([NAME_FIELD, EMAIL_FIELD]), autoOpen: true }
    });

    await waitFor(() => expect(screen.getByLabelText('Name')).toBeTruthy());
    expect(screen.getByLabelText('Email')).toBeTruthy();
    expect(screen.getByText('2/2 fields')).toBeTruthy();
  });

  it('hides the Collapsible entirely when every field on the schema is system-protected', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      nodeWith({ 'person-like': { _synthetic_system_field: true } })
    );
    const { container } = render(GenericSchemaForm, {
      props: { nodeId: 'node-1', schema: schemaWith([SYSTEM_MARKER_FIELD]), autoOpen: true }
    });

    // No field grid renders at all — same "0 visible fields" behavior as a schema
    // with a genuinely empty fields array.
    await waitFor(() => expect(container.querySelector('.schema-form-wrapper')).toBeTruthy());
    expect(screen.queryByText(/fields$/)).toBeNull();
    expect(screen.queryByLabelText('Synthetic system field')).toBeNull();
  });
});
