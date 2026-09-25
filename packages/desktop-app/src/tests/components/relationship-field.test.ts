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
