/**
 * GenericSchemaForm rendering the REAL `project` schema.
 *
 * `project` is a core node type (`core_schemas.rs`) with no hardcoded schema
 * form and no plugin registration at all — `needsGenericSchemaForm('project')`
 * is true (node-type-predicates.ts), so opening a project node falls back to
 * this component for its properties panel. `project` used to be incorrectly
 * classified as core-and-therefore-excluded from the generic fallback, so
 * opening one rendered no properties panel whatsoever.
 *
 * schema-form-loader.svelte.test.ts and schema-field-resolution.test.ts prove
 * the *plumbing* (the loader fetches project's schema; the read/write helpers
 * resolve the flat shape correctly) with synthetic field shapes. This
 * file closes the remaining gap: that the rendered form actually shows
 * project's real fields — status, priority, start_date, end_date, mirroring
 * the schema exactly as `core_schemas.rs` declares it — with correct labels
 * and values, and persists an edit back in the flat shape every transport
 * delivers (the backend re-buckets it under `project` in storage).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';
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

function enumField(
  name: string,
  friendlyName: string,
  coreValues: Array<{ value: string; label: string }>,
  opts: Partial<SchemaField> = {}
): SchemaField {
  return {
    name,
    friendlyName,
    type: 'enum',
    protection: 'user',
    indexed: true,
    coreValues,
    userValues: [],
    required: false,
    ...opts
  };
}

function dateField(name: string, friendlyName: string): SchemaField {
  return { name, friendlyName, type: 'date', protection: 'user', indexed: false, required: false };
}

/** Mirrors the `project` SchemaNode exactly as declared in core_schemas.rs. */
const PROJECT_SCHEMA: SchemaNode = {
  id: 'project',
  content: 'Project',
  createdAt: '2026-01-01T00:00:00Z',
  modifiedAt: '2026-01-01T00:00:00Z',
  version: 1,
  isCore: true,
  schemaVersion: 1,
  fields: [
    enumField(
      'status',
      'Status',
      [
        { value: 'planning', label: 'Planning' },
        { value: 'active', label: 'Active' },
        { value: 'completed', label: 'Completed' },
        { value: 'archived', label: 'Archived' },
        { value: 'cancelled', label: 'Cancelled' }
      ],
      // protection: Core + extensible: true, matching core_schemas.rs exactly — this
      // schema is meant to mirror the real one field-for-field, and #2132's planned
      // protection-level filtering work needs a faithful fixture to test against.
      { required: true, default: 'planning', protection: 'core', extensible: true }
    ),
    enumField(
      'priority',
      'Priority',
      [
        { value: 'low', label: 'Low' },
        { value: 'medium', label: 'Medium' },
        { value: 'high', label: 'High' }
      ],
      { extensible: true }
    ),
    dateField('start_date', 'Start date'),
    dateField('end_date', 'End date')
  ]
  // `project` also declares an outbound `tasks` relationship in core_schemas.rs, but
  // GenericSchemaForm's Relationships gate resolves that via loadNodeRelationshipsView
  // (mocked above), not by reading a field on the SchemaNode object — omitted here as
  // out of scope for these field-rendering assertions.
};

function projectNode(properties: Record<string, unknown>): Node {
  return {
    id: 'project-1',
    nodeType: 'project',
    content: 'Website redesign',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties
  } as Node;
}

let updateNodeSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'project', groups: [] });
  updateNodeSpy = vi.fn();
  vi.spyOn(sharedNodeStore, 'updateNode').mockImplementation(
    updateNodeSpy as unknown as typeof sharedNodeStore.updateNode
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('GenericSchemaForm — real project schema', () => {
  it('renders all four project fields with their schema labels', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      projectNode({ status: 'planning' })
    );
    render(GenericSchemaForm, { props: { nodeId: 'project-1', schema: PROJECT_SCHEMA, autoOpen: true } });

    await waitFor(() => expect(screen.getByText('Status')).toBeTruthy());
    expect(screen.getByText('Priority')).toBeTruthy();
    expect(screen.getByText('Start date')).toBeTruthy();
    expect(screen.getByText('End date')).toBeTruthy();
  });

  it('shows the backend-applied default status and counts it as filled', async () => {
    // apply_schema_defaults_with_fields (crud.rs) writes `status: "planning"` into
    // the project's storage bucket at creation time — by the time this form ever sees a
    // project node, the required `status` field is never actually empty in
    // practice. This pins that the form displays whatever value it is given
    // (here, the backend-applied default) rather than needing its own
    // client-side default-application logic.
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      projectNode({ status: 'planning' })
    );
    render(GenericSchemaForm, { props: { nodeId: 'project-1', schema: PROJECT_SCHEMA, autoOpen: true } });

    await waitFor(() => expect(screen.getByText('Planning')).toBeTruthy());
    // 1 of 4 fields filled: status only.
    expect(screen.getByText('1/4 fields')).toBeTruthy();
  });

  it('shows placeholders for unset priority/start_date/end_date and counts them unfilled', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      projectNode({ status: 'active' })
    );
    render(GenericSchemaForm, { props: { nodeId: 'project-1', schema: PROJECT_SCHEMA, autoOpen: true } });

    await waitFor(() => expect(screen.getByText('1/4 fields')).toBeTruthy());
    expect(screen.getByText('Select Priority...')).toBeTruthy();
    expect(screen.getAllByText('Pick a date')).toHaveLength(2);
  });

  it('renders every field filled once all four are set', async () => {
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      projectNode({
        status: 'active',
        priority: 'high',
        start_date: '2026-01-01',
        end_date: '2026-03-31'
      })
    );
    render(GenericSchemaForm, { props: { nodeId: 'project-1', schema: PROJECT_SCHEMA, autoOpen: true } });

    await waitFor(() => expect(screen.getByText('4/4 fields')).toBeTruthy());
    expect(screen.getByText('Active')).toBeTruthy();
    expect(screen.getByText('High')).toBeTruthy();
  });

  it('persists a date-field edit flat, preserving siblings', async () => {
    // Exercises the write path through a real leaf control (the date Popover trigger is a
    // plain button, unlike the enum Select's portalled listbox, so it is a reliable target
    // in Happy-DOM). The enum write path itself —
    // is already proven directly against `buildFieldWrite`/`resolveFieldValue`
    // (schema-field-resolution.test.ts) and end-to-end for a nested field
    // (property-form-field-dispatch.test.ts); this test's job is only to confirm the real
    // project field set round-trips through the rendered component without dropping a
    // sibling field.
    vi.spyOn(sharedNodeStore, 'getNode').mockReturnValue(
      projectNode({ status: 'planning', priority: 'low' })
    );
    render(GenericSchemaForm, { props: { nodeId: 'project-1', schema: PROJECT_SCHEMA, autoOpen: true } });

    await waitFor(() => expect(screen.getByText('Planning')).toBeTruthy());
    await fireEvent.click(screen.getByLabelText('Start date'));
    const dayButton = await waitFor(() => {
      const btn = document.querySelector('[data-bits-day]:not([data-outside-month])');
      expect(btn).toBeTruthy();
      return btn as Element;
    });
    await fireEvent.click(dayButton);

    await waitFor(() => expect(updateNodeSpy).toHaveBeenCalledTimes(1));
    const [, changes] = updateNodeSpy.mock.calls[0] as [string, Partial<Node>];
    const persisted = changes.properties as Record<string, unknown>;
    // The exact date depends on "today" (the Calendar's default view), which is not the
    // point of this test — the point is that status/priority survive the write untouched
    // and end_date is not spuriously introduced.
    expect(persisted.status).toBe('planning');
    expect(persisted.priority).toBe('low');
    expect(persisted.start_date).toEqual(expect.any(String));
    expect(persisted.end_date).toBeUndefined();
  });
});
