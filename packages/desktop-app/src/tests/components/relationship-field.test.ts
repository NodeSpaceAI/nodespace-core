/**
 * RelationshipField — a single-valued relationship edited as a form field.
 *
 * Covers the write contract: an inbound group is written from the declaring
 * side under its forward name (a task's assignee is `person —tasks→ task`),
 * picking a node over an existing one is a plain create (the daemon replaces
 * the edge into a `one` end), clearing removes the edge, and a required
 * relationship offers no clear.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import RelationshipField from '$lib/components/relationships/relationship-field.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import {
  buildRelationshipsView,
  type RawRelationshipGroup
} from '$lib/services/relationship-grouping';
import type { Node } from '$lib/types';

function assigneeGroup(overrides: Partial<RawRelationshipGroup> = {}) {
  return buildRelationshipsView({
    nodeId: 'task-1',
    nodeType: 'task',
    groups: [
      {
        relationshipName: 'tasks',
        direction: 'in',
        targetType: 'person',
        reverseName: 'assignee',
        sourceType: 'person',
        cardinality: 'one',
        required: null,
        edgeFields: null,
        description: null,
        related: [],
        count: 0,
        ...overrides
      }
    ]
  }).groups[0];
}

const sam = {
  id: 'person-sam',
  nodeType: 'person',
  title: 'Sam Lee',
  contentPreview: '',
  edgeProperties: {}
};

function personNode(id: string, title: string): Node {
  return { id, nodeType: 'person', title, content: '', properties: {} } as unknown as Node;
}

let createRelationship: ReturnType<typeof vi.fn>;
let deleteRelationship: ReturnType<typeof vi.fn>;
let searchNodesByTitle: ReturnType<typeof vi.fn>;
let onChanged: ReturnType<typeof vi.fn>;

beforeEach(() => {
  createRelationship = vi.fn().mockResolvedValue(undefined);
  deleteRelationship = vi.fn().mockResolvedValue(undefined);
  searchNodesByTitle = vi.fn().mockResolvedValue([]);
  onChanged = vi.fn().mockResolvedValue(undefined);
  vi.spyOn(backendAdapter, 'createRelationship').mockImplementation(
    createRelationship as unknown as typeof backendAdapter.createRelationship
  );
  vi.spyOn(backendAdapter, 'deleteRelationship').mockImplementation(
    deleteRelationship as unknown as typeof backendAdapter.deleteRelationship
  );
  vi.spyOn(backendAdapter, 'searchNodesByTitle').mockImplementation(
    searchNodesByTitle as unknown as typeof backendAdapter.searchNodesByTitle
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

async function search(input: HTMLElement, query: string) {
  await fireEvent.input(input, { target: { value: query } });
  await waitFor(() => expect(searchNodesByTitle).toHaveBeenCalledWith('person', query, 10));
}

describe('RelationshipField', () => {
  it('assigns an empty inbound field with transposed arguments', async () => {
    searchNodesByTitle.mockResolvedValue([personNode('person-sam', 'Sam Lee')]);
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    const input = screen.getByRole('combobox');
    await search(input, 'sam');
    const option = await screen.findByRole('option', { name: 'Sam Lee' });
    await fireEvent.mouseDown(option);

    // Stored under the declared forward name, from the declaring person.
    await waitFor(() =>
      expect(createRelationship).toHaveBeenCalledWith('person-sam', 'tasks', 'task-1', undefined)
    );
    expect(onChanged).toHaveBeenCalled();
  });

  it('replaces an existing value with a plain create, never offering the current one', async () => {
    searchNodesByTitle.mockResolvedValue([
      personNode('person-sam', 'Sam Lee'),
      personNode('person-ana', 'Ana Ruiz')
    ]);
    render(RelationshipField, {
      props: {
        nodeId: 'task-1',
        group: assigneeGroup({ related: [sam], count: 1 }),
        fieldId: 'f',
        onChanged
      }
    });

    await fireEvent.click(screen.getByText('Sam Lee'));
    const input = await screen.findByRole('combobox');
    await search(input, 'a');
    await screen.findByRole('option', { name: 'Ana Ruiz' });
    expect(screen.queryByRole('option', { name: 'Sam Lee' })).toBeNull();

    await fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() =>
      expect(createRelationship).toHaveBeenCalledWith('person-ana', 'tasks', 'task-1', undefined)
    );
    expect(deleteRelationship).not.toHaveBeenCalled();
  });

  it('clears the field by removing the edge', async () => {
    render(RelationshipField, {
      props: {
        nodeId: 'task-1',
        group: assigneeGroup({ related: [sam], count: 1 }),
        fieldId: 'f',
        onChanged
      }
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Clear assignee' }));
    await waitFor(() =>
      expect(deleteRelationship).toHaveBeenCalledWith('person-sam', 'tasks', 'task-1')
    );
    expect(onChanged).toHaveBeenCalled();
  });

  it('offers no clear on a required relationship', () => {
    render(RelationshipField, {
      props: {
        nodeId: 'task-1',
        group: assigneeGroup({ related: [sam], count: 1, required: true }),
        fieldId: 'f',
        onChanged
      }
    });
    expect(screen.queryByRole('button', { name: 'Clear assignee' })).toBeNull();
  });

  it('closes the list when an empty field loses focus', async () => {
    searchNodesByTitle.mockResolvedValue([personNode('person-sam', 'Sam Lee')]);
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    const input = screen.getByRole('combobox');
    await search(input, 'sam');
    await screen.findByRole('listbox');
    await fireEvent.blur(input);

    expect(screen.queryByRole('listbox')).toBeNull();
    expect((screen.getByRole('combobox') as HTMLInputElement).value).toBe('');
  });

  it('keeps focus in the input when the list itself is pressed', async () => {
    searchNodesByTitle.mockResolvedValue([personNode('person-sam', 'Sam Lee')]);
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    await search(screen.getByRole('combobox'), 'sam');
    const listbox = await screen.findByRole('listbox');
    // A mousedown the list doesn't cancel would blur the input and close it
    // under the pointer (a scrollbar drag, a status row).
    const press = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    listbox.dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
  });

  it('moves the highlight with arrow keys and exposes it as the active descendant', async () => {
    searchNodesByTitle.mockResolvedValue([
      personNode('person-sam', 'Sam Lee'),
      personNode('person-ana', 'Ana Ruiz')
    ]);
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    const input = screen.getByRole('combobox');
    await search(input, 'a');
    await screen.findByRole('option', { name: 'Ana Ruiz' });
    expect(input.getAttribute('aria-activedescendant')).toBe('f-option-0');

    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(input.getAttribute('aria-activedescendant')).toBe('f-option-1');
    expect(screen.getByRole('option', { name: 'Ana Ruiz' }).getAttribute('aria-selected')).toBe(
      'true'
    );

    await fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() =>
      expect(createRelationship).toHaveBeenCalledWith('person-ana', 'tasks', 'task-1', undefined)
    );
  });

  it('closes the list on Escape without writing', async () => {
    searchNodesByTitle.mockResolvedValue([personNode('person-sam', 'Sam Lee')]);
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    const input = screen.getByRole('combobox');
    await search(input, 'sam');
    await screen.findByRole('listbox');
    await fireEvent.keyDown(input, { key: 'Escape' });

    expect(screen.queryByRole('listbox')).toBeNull();
    expect(createRelationship).not.toHaveBeenCalled();
  });

  it('writes an outbound `one` edge from this node', async () => {
    searchNodesByTitle.mockResolvedValue([personNode('customer-1', 'Acme')]);
    const group = buildRelationshipsView({
      nodeId: 'invoice-1',
      nodeType: 'invoice',
      groups: [
        {
          relationshipName: 'billed_to',
          direction: 'out',
          targetType: 'customer',
          reverseName: 'invoices',
          sourceType: 'invoice',
          cardinality: 'one',
          required: null,
          edgeFields: null,
          description: null,
          related: [],
          count: 0
        }
      ]
    }).groups[0];
    render(RelationshipField, { props: { nodeId: 'invoice-1', group, fieldId: 'f', onChanged } });

    const input = screen.getByRole('combobox');
    await fireEvent.input(input, { target: { value: 'acme' } });
    await fireEvent.mouseDown(await screen.findByRole('option', { name: 'Acme' }));

    await waitFor(() =>
      expect(createRelationship).toHaveBeenCalledWith('invoice-1', 'billed_to', 'customer-1', undefined)
    );
  });

  it('ignores a second pick while the first write is in flight', async () => {
    let finish!: () => void;
    createRelationship.mockReturnValue(new Promise<void>((resolve) => (finish = resolve)));
    searchNodesByTitle.mockResolvedValue([
      personNode('person-sam', 'Sam Lee'),
      personNode('person-ana', 'Ana Ruiz')
    ]);
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    await search(screen.getByRole('combobox'), 'a');
    const sam = await screen.findByRole('option', { name: 'Sam Lee' });
    const ana = screen.getByRole('option', { name: 'Ana Ruiz' });
    await fireEvent.mouseDown(sam);
    await fireEvent.mouseDown(ana);
    finish();

    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(createRelationship).toHaveBeenCalledTimes(1);
  });

  it('reports a failed search rather than "No matches."', async () => {
    searchNodesByTitle.mockRejectedValue(new Error('daemon offline'));
    render(RelationshipField, {
      props: { nodeId: 'task-1', group: assigneeGroup(), fieldId: 'f', onChanged }
    });

    await search(screen.getByRole('combobox'), 'sam');
    await screen.findByText('Search failed.');
    expect(screen.queryByText('No matches.')).toBeNull();
  });

  it('names the set value for assistive tech, not just the field', () => {
    render(RelationshipField, {
      props: {
        nodeId: 'task-1',
        group: assigneeGroup({ related: [sam], count: 1 }),
        fieldId: 'f',
        onChanged
      }
    });
    expect(screen.getByRole('button', { name: 'Assignee: Sam Lee' })).toBeTruthy();
  });

  it('surfaces a failed write and keeps the field editable', async () => {
    deleteRelationship.mockRejectedValue(new Error('daemon offline'));
    render(RelationshipField, {
      props: {
        nodeId: 'task-1',
        group: assigneeGroup({ related: [sam], count: 1 }),
        fieldId: 'f',
        onChanged
      }
    });

    await fireEvent.click(screen.getByRole('button', { name: 'Clear assignee' }));
    await screen.findByText('daemon offline');
    expect(onChanged).not.toHaveBeenCalled();
  });
});
