/**
 * RelationshipViewerModal — authoring a many-to-many edge from its inbound end.
 *
 * A task's `Blocked By` is the same stored edge as another task's `Blocks`, so
 * it is added and removed here with the endpoints transposed: the edge is
 * written `other —blocks→ this` under the declared forward name. An inbound
 * group that declares edge fields stays read-only — its values are authored
 * where the relationship is declared.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor, within } from '@testing-library/svelte';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import RelationshipViewerModal from '$lib/components/relationships/relationship-viewer-modal.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { RawNodeRelationships, RawRelationshipGroup } from '$lib/services/relationship-grouping';
import type { Node } from '$lib/types';

function group(overrides: Partial<RawRelationshipGroup>): RawRelationshipGroup {
  return {
    relationshipName: 'blocks',
    direction: 'out',
    targetType: 'task',
    reverseName: 'blocked_by',
    sourceType: 'task',
    cardinality: 'many',
    required: null,
    edgeFields: null,
    description: null,
    related: [],
    count: 0,
    ...overrides
  };
}

function related(id: string, title: string) {
  return { id, nodeType: 'task', title, contentPreview: '', edgeProperties: {} };
}

function taskNode(id: string, title: string): Node {
  return { id, nodeType: 'task', title, content: '', properties: {} } as unknown as Node;
}

function payload(groups: RawRelationshipGroup[]): RawNodeRelationships {
  return { nodeId: 'task-a', nodeType: 'task', groups };
}

let getNodeRelationships: ReturnType<typeof vi.fn>;
let createRelationship: ReturnType<typeof vi.fn>;
let deleteRelationship: ReturnType<typeof vi.fn>;
let searchNodesByTitle: ReturnType<typeof vi.fn>;

beforeEach(() => {
  getNodeRelationships = vi.fn();
  createRelationship = vi.fn().mockResolvedValue(undefined);
  deleteRelationship = vi.fn().mockResolvedValue(undefined);
  searchNodesByTitle = vi.fn().mockResolvedValue([]);
  vi.spyOn(backendAdapter, 'getNodeRelationships').mockImplementation(
    getNodeRelationships as unknown as typeof backendAdapter.getNodeRelationships
  );
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

describe('RelationshipViewerModal: inbound many-to-many', () => {
  it('offers an empty inbound group under + Add and writes the edge transposed', async () => {
    getNodeRelationships.mockResolvedValue(
      payload([group({}), group({ direction: 'in' })])
    );
    searchNodesByTitle.mockResolvedValue([taskNode('task-b', 'Ship API')]);
    render(RelationshipViewerModal, { props: { open: true, nodeId: 'task-a' } });

    await fireEvent.click(await screen.findByText('Add'));
    // Both ends of the many-to-many are offered.
    expect(await screen.findByRole('button', { name: /Blocks/ })).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: /Blocked By/ }));

    const input = await screen.findByPlaceholderText('Search task…');
    await fireEvent.input(input, { target: { value: 'ship' } });
    await waitFor(() => expect(searchNodesByTitle).toHaveBeenCalledWith('task', 'ship', 10));
    await fireEvent.click(await screen.findByText('Ship API'));

    // "task-a is blocked by task-b" is stored as `task-b —blocks→ task-a`.
    await waitFor(() =>
      expect(createRelationship).toHaveBeenCalledWith('task-b', 'blocks', 'task-a', undefined)
    );
  });

  it('removes a bare inbound edge with transposed arguments', async () => {
    getNodeRelationships.mockResolvedValue(
      payload([group({ direction: 'in', related: [related('task-b', 'Ship API')], count: 1 })])
    );
    render(RelationshipViewerModal, { props: { open: true, nodeId: 'task-a' } });

    // A bare inbound group is editable here, so it is not under the read-only heading.
    await screen.findByText('Ship API');
    expect(screen.queryByText('Incoming · read-only')).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Remove Ship API' }));
    await waitFor(() =>
      expect(deleteRelationship).toHaveBeenCalledWith('task-b', 'blocks', 'task-a')
    );
  });

  it('keeps an inbound group with edge fields read-only: no remove, no add', async () => {
    getNodeRelationships.mockResolvedValue(
      payload([
        group({
          relationshipName: 'has_access_to',
          direction: 'in',
          targetType: 'person',
          sourceType: 'person',
          reverseName: 'members',
          edgeFields: [{ name: 'access', type: 'string' }],
          related: [{ ...related('person-sam', 'Sam Lee'), nodeType: 'person' }],
          count: 1
        })
      ])
    );
    render(RelationshipViewerModal, { props: { open: true, nodeId: 'task-a' } });

    const heading = await screen.findByText('Incoming · read-only');
    expect(within(heading.parentElement as HTMLElement).getByText('Members')).toBeTruthy();
    expect(await screen.findByText('Sam Lee')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Remove Sam Lee' })).toBeNull();
    expect(screen.queryByText('Add')).toBeNull();
  });
});
