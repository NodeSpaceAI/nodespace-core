/**
 * TableRow — stale-title regression.
 *
 * See list-view.test.ts for the full root-cause explanation. The 'content' column used to
 * prefer `node.title` unconditionally, so a row for a node whose title went stale showed the
 * wrong text. It must now show current content for non-template types, and still show the
 * computed title for title_template-driven custom entities. The row's tooltip (built from the
 * same source) is covered too.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import type { SchemaField } from '$lib/types/schema-node';
import type { Node } from '$lib/types';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import TableRow from '$lib/components/query/table-row.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { pluginRegistry } from '$lib/plugins/index';

function baseNode(overrides: Partial<Node> = {}): Node {
  return {
    lifecycleStatus: 'active',
    id: 'n1',
    nodeType: 'task',
    content: 'current content',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    mentions: [],
    ...overrides
  };
}

const columns = [{ field: 'content', label: 'Content' }];
const emptyFieldSchemaMap = new Map<string, SchemaField>();

describe('TableRow — stale title regression', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
    pluginRegistry.unregister('widget-entity');
  });

  it('shows current content, not a stale cached title, for a non-template type', () => {
    const node = baseNode({ title: '/', content: 'Another Task' });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container, getByRole } = render(TableRow, {
      props: { id: node.id, columns, fieldSchemaMap: emptyFieldSchemaMap, onRowClick: () => {} }
    });

    const cell = container.querySelector('td');
    expect(cell?.textContent?.trim()).toBe('Another Task');
    expect(getByRole('button', { name: /Another Task/ })).toBeTruthy();
  });

  it('still shows the computed title for a title_template-driven custom entity', () => {
    pluginRegistry.register({
      id: 'widget-entity',
      name: 'Widget',
      description: 'Custom entity with a title template',
      version: '1.0.0',
      config: { slashCommands: [] },
      hasTitleTemplate: true,
      titleTemplate: '{first_name} {last_name}'
    });

    const node = baseNode({ nodeType: 'widget-entity', content: 'raw', title: 'Jane Doe' });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(TableRow, {
      props: { id: node.id, columns, fieldSchemaMap: emptyFieldSchemaMap, onRowClick: () => {} }
    });

    const cell = container.querySelector('td');
    expect(cell?.textContent?.trim()).toBe('Jane Doe');
  });
});

describe('TableRow — field columns are read by their stored name', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    cleanup();
    sharedNodeStore.clearAll();
  });

  function cellTexts(container: HTMLElement): string[] {
    return Array.from(container.querySelectorAll('td')).map((td) => td.textContent?.trim() ?? '');
  }

  it('renders a snake_case custom field from properties', () => {
    const node = baseNode({
      nodeType: 'invoice',
      content: 'Invoice 7',
      properties: { billing_contact: 'Avery', unit_price: 12 }
    });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(TableRow, {
      props: {
        id: node.id,
        columns: [
          { field: 'content', label: 'Content' },
          { field: 'billing_contact', label: 'Billing contact' },
          { field: 'unit_price', label: 'Unit price' }
        ],
        fieldSchemaMap: emptyFieldSchemaMap,
        onRowClick: () => {}
      }
    });

    expect(cellTexts(container)).toEqual(['Invoice 7', 'Avery', '12']);
  });

  it('does not read a camelCase spelling of a custom field', () => {
    const node = baseNode({
      nodeType: 'invoice',
      properties: { billingContact: 'camelCase key' },
      // A top-level key under the camelCase spelling is not the field either.
      ...({ billingContact: 'top-level key' } as Partial<Node>)
    });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(TableRow, {
      props: {
        id: node.id,
        columns: [{ field: 'billing_contact', label: 'Billing contact' }],
        fieldSchemaMap: emptyFieldSchemaMap,
        onRowClick: () => {}
      }
    });

    expect(cellTexts(container)).toEqual(['']);
  });

  it("renders a core type's declared field from its typed key", () => {
    const node = baseNode({
      nodeType: 'person',
      content: '',
      ...({ firstName: 'Jane', lastName: 'Doe' } as Partial<Node>)
    });
    sharedNodeStore.setNode(node, { type: 'database', reason: 'seed' });

    const { container } = render(TableRow, {
      props: {
        id: node.id,
        columns: [
          { field: 'first_name', label: 'First name' },
          { field: 'last_name', label: 'Last name' }
        ],
        fieldSchemaMap: emptyFieldSchemaMap,
        onRowClick: () => {}
      }
    });

    expect(cellTexts(container)).toEqual(['Jane', 'Doe']);
  });
});
