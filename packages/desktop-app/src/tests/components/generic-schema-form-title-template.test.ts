/**
 * GenericSchemaForm — client-side title preview for a title_template schema
 * (ADR-077), covering a NON-person title-templated type.
 *
 * person-schema-form.test.ts covers PersonSchemaForm's own title
 * preview. Per the issue's explicit "not person-specific" framing, this file
 * proves the same mechanism works for GenericSchemaForm — the path any
 * user-defined `titleTemplate` schema actually renders through — using a
 * synthetic "ticket" schema with a `{severity}: {subject}` template.
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
import { backendAdapter } from '$lib/services/backend-adapter';

function stringField(name: string, friendlyName: string): SchemaField {
  return { name, friendlyName, type: 'string', protection: 'user', indexed: false, required: false };
}

function enumField(
  name: string,
  friendlyName: string,
  coreValues: Array<{ value: string; label: string }>
): SchemaField {
  return {
    name,
    friendlyName,
    type: 'enum',
    protection: 'user',
    indexed: false,
    required: false,
    coreValues,
    userValues: []
  };
}

const TICKET_SCHEMA: SchemaNode = {
  id: 'ticket',
  content: 'Ticket',
  createdAt: '2026-01-01T00:00:00Z',
  modifiedAt: '2026-01-01T00:00:00Z',
  version: 1,
  isCore: false,
  schemaVersion: 1,
  fields: [stringField('severity', 'Severity'), stringField('subject', 'Subject')],
  titleTemplate: '{severity}: {subject}'
};

// A variant with `severity` as an enum instead of a plain string, for the
// enum-label-resolution test below. Kept separate from TICKET_SCHEMA so the
// other tests above (which type directly into Severity as a text input)
// are unaffected — an enum field renders as a Select, not an Input.
const TICKET_SCHEMA_ENUM_SEVERITY: SchemaNode = {
  ...TICKET_SCHEMA,
  fields: [
    enumField('severity', 'Severity', [
      { value: 'p1', label: 'P1 - Critical' },
      { value: 'p2', label: 'P2 - High' }
    ]),
    stringField('subject', 'Subject')
  ]
};

function ticketNode(overrides: Partial<Node> = {}): Node {
  return {
    id: 'ticket-1',
    nodeType: 'ticket',
    content: '',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    title: '',
    properties: { severity: '', subject: '' },
    ...overrides
  } as Node;
}

beforeEach(() => {
  loadNodeRelationshipsView.mockResolvedValue({ nodeType: 'ticket', groups: [] });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('GenericSchemaForm — title_template client-side preview (ADR-077), non-person type', () => {
  it('computes and displays the title as the user types, with no dependency on the backend round trip', async () => {
    sharedNodeStore.setNode(
      ticketNode(),
      { type: 'database', reason: 'test-seed' },
      true
    );
    // Never resolves — proves the preview does not wait on this at all.
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(() => new Promise(() => {}));

    render(GenericSchemaForm, {
      props: { nodeId: 'ticket-1', schema: TICKET_SCHEMA, autoOpen: true }
    });

    await waitFor(() => expect(screen.getByLabelText('Severity')).toBeTruthy());
    const severity = screen.getByLabelText('Severity') as HTMLInputElement;
    const subject = screen.getByLabelText('Subject') as HTMLInputElement;

    await fireEvent.input(severity, { target: { value: 'P1' } });
    expect(sharedNodeStore.getNode('ticket-1')?.title).toBe('P1:');

    await fireEvent.input(subject, { target: { value: 'Disk full' } });
    expect(sharedNodeStore.getNode('ticket-1')?.title).toBe('P1: Disk full');
  });

  it('does not compute or push a title for a schema with no titleTemplate', async () => {
    const NO_TEMPLATE_SCHEMA: SchemaNode = { ...TICKET_SCHEMA, titleTemplate: undefined };
    sharedNodeStore.setNode(
      ticketNode({ title: undefined }),
      { type: 'database', reason: 'test-seed' },
      true
    );
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(() => new Promise(() => {}));

    render(GenericSchemaForm, {
      props: { nodeId: 'ticket-1', schema: NO_TEMPLATE_SCHEMA, autoOpen: true }
    });

    await waitFor(() => expect(screen.getByLabelText('Severity')).toBeTruthy());
    const severity = screen.getByLabelText('Severity') as HTMLInputElement;
    await fireEvent.input(severity, { target: { value: 'P1' } });

    expect(sharedNodeStore.getNode('ticket-1')?.title).toBeUndefined();
  });

  it('resolves an enum field referenced by the title template to its label, not the raw stored value', async () => {
    // severity is pre-seeded as an enum value ('p1') rather than driven
    // through the Select control (Happy-DOM interaction with a portalled
    // listbox is unreliable — see generic-schema-form-project.test.ts's
    // note on the same trade-off); only `subject`, a plain text field, is
    // edited to trigger the recompute this test actually checks.
    sharedNodeStore.setNode(
      ticketNode({
        title: '',
        properties: { severity: 'p1', subject: '' }
      }),
      { type: 'database', reason: 'test-seed' },
      true
    );
    vi.spyOn(backendAdapter, 'updateNode').mockImplementation(() => new Promise(() => {}));

    render(GenericSchemaForm, {
      props: { nodeId: 'ticket-1', schema: TICKET_SCHEMA_ENUM_SEVERITY, autoOpen: true }
    });

    await waitFor(() => expect(screen.getByLabelText('Subject')).toBeTruthy());
    const subject = screen.getByLabelText('Subject') as HTMLInputElement;
    await fireEvent.input(subject, { target: { value: 'Disk full' } });

    expect(sharedNodeStore.getNode('ticket-1')?.title).toBe('P1 - Critical: Disk full');
  });

  it('a server-provided title from a normal fetch/first-load is displayed correctly and untouched by merely mounting the form', async () => {
    sharedNodeStore.setNode(
      ticketNode({
        title: 'P0: Already Set',
        properties: { severity: 'P0', subject: 'Already Set' }
      }),
      { type: 'database', reason: 'test-seed' },
      true
    );

    render(GenericSchemaForm, {
      props: { nodeId: 'ticket-1', schema: TICKET_SCHEMA, autoOpen: true }
    });

    await waitFor(() => expect(screen.getByLabelText('Severity')).toBeTruthy());
    expect(sharedNodeStore.getNode('ticket-1')?.title).toBe('P0: Already Set');
  });
});
