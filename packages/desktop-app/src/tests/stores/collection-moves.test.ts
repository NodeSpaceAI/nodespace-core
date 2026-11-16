/**
 * The rules and messages for putting one collection under another and for
 * renaming a collection (collection-moves.ts), as pure functions.
 */
import { describe, it, expect } from 'vitest';
import {
  ancestorIds,
  describeRefusal,
  descendantIds,
  hasMoveTarget,
  moveFailureMessage,
  moveTargets,
  refusalForMove,
  removeFailureMessage,
  renameFailureMessage,
  sameName
} from '$lib/stores/collection-moves';
import type { CollectionInfo } from '$lib/services/collection-service';

function collection(id: string, name: string, parentCollectionIds: string[] = []): CollectionInfo {
  return {
    lifecycleStatus: 'active',
    id,
    content: name,
    nodeType: 'collection',
    createdAt: '',
    modifiedAt: '',
    version: 1,
    properties: {},
    memberCount: 1,
    parentCollectionIds
  };
}

// company-policies sits in deutnet; hr sits in company-policies; design is on its own.
const COLLECTIONS = [
  collection('deutnet', 'DeutNet'),
  collection('company-policies', 'Company Policies', ['deutnet']),
  collection('hr', 'HR', ['company-policies']),
  collection('design', 'Design')
];

describe('ancestorIds and descendantIds', () => {
  it('walk the whole chain, not only the direct parent or child', () => {
    expect([...ancestorIds(COLLECTIONS, 'hr')].sort()).toEqual(['company-policies', 'deutnet']);
    expect([...descendantIds(COLLECTIONS, 'deutnet')].sort()).toEqual(['company-policies', 'hr']);
  });

  it('are empty for a collection on its own', () => {
    expect(ancestorIds(COLLECTIONS, 'design').size).toBe(0);
    expect(descendantIds(COLLECTIONS, 'design').size).toBe(0);
  });

  it('terminate on data that already holds a cycle', () => {
    const cyclic = [collection('a', 'A', ['b']), collection('b', 'B', ['a'])];
    expect([...ancestorIds(cyclic, 'a')].sort()).toEqual(['a', 'b']);
    expect([...descendantIds(cyclic, 'a')].sort()).toEqual(['a', 'b']);
  });

  it('follow every parent of a collection that sits in several', () => {
    const graph = [
      collection('x', 'X'),
      collection('y', 'Y'),
      collection('z', 'Z', ['x', 'y'])
    ];
    expect([...ancestorIds(graph, 'z')].sort()).toEqual(['x', 'y']);
  });
});

describe('refusalForMove', () => {
  it('allows a collection into an unrelated one, and into a second parent', () => {
    expect(refusalForMove(COLLECTIONS, 'design', 'deutnet')).toBeNull();
    // company-policies is already in deutnet: a second parent is fine.
    expect(refusalForMove(COLLECTIONS, 'company-policies', 'design')).toBeNull();
  });

  it('refuses a collection into itself', () => {
    expect(refusalForMove(COLLECTIONS, 'design', 'design')).toBe('self');
  });

  it('refuses a collection into the one it already sits in', () => {
    expect(refusalForMove(COLLECTIONS, 'company-policies', 'deutnet')).toBe('already-inside');
  });

  it('refuses DeutNet into Company Policies when Company Policies is already in DeutNet', () => {
    expect(refusalForMove(COLLECTIONS, 'deutnet', 'company-policies')).toBe('cycle');
  });

  it('refuses a cycle that closes through a longer chain', () => {
    expect(refusalForMove(COLLECTIONS, 'deutnet', 'hr')).toBe('cycle');
  });
});

describe('moveTargets', () => {
  it('leaves out the collection itself, its parents and everything inside it, sorted by name', () => {
    const targets = moveTargets(COLLECTIONS, 'company-policies').map((c) => c.id);
    // Not deutnet (its parent), not company-policies (itself), not hr (inside it).
    expect(targets).toEqual(['design']);
    expect(moveTargets(COLLECTIONS, 'design').map((c) => c.id)).toEqual([
      'company-policies',
      'deutnet',
      'hr'
    ]);
  });

  it('leaves out root collections and collections still being created', () => {
    const withExtras = [...COLLECTIONS, collection('ext-root', 'Extension root'), collection('pending-collection-1', 'New')];
    const ids = moveTargets(withExtras, 'design', new Set(['ext-root'])).map((c) => c.id);
    expect(ids).not.toContain('ext-root');
    expect(ids).not.toContain('pending-collection-1');
  });
});

describe('messages', () => {
  it('name both collections in a refused move', () => {
    expect(describeRefusal('cycle', 'DeutNet', 'Company Policies')).toBe(
      '“DeutNet” can\'t go into “Company Policies” because “Company Policies” is already inside “DeutNet”.'
    );
    expect(describeRefusal('already-inside', 'HR', 'Company Policies')).toBe(
      '“HR” is already in “Company Policies”.'
    );
    expect(describeRefusal('self', 'HR', 'HR')).toBe('“HR” can\'t be moved into itself.');
  });

  it("turn the backend's cycle refusal into the same cycle message", () => {
    const backend = {
      message:
        "Failed to add node to collection: collection_cycle: 'a' is already a descendant of 'b', so making it the parent would create a cycle",
      code: 'COLLECTION_ERROR'
    };
    expect(moveFailureMessage(backend, 'DeutNet', 'Company Policies')).toBe(
      describeRefusal('cycle', 'DeutNet', 'Company Policies')
    );
  });

  it("carry the backend's own reason for any other refused move, such as missing rights", () => {
    const refused = {
      message: 'Failed to add node to collection: You do not have access to “Company Policies”',
      code: 'COLLECTION_ERROR'
    };
    expect(moveFailureMessage(refused, 'HR', 'Company Policies')).toBe(
      'Couldn\'t move “HR” into “Company Policies”: You do not have access to “Company Policies”'
    );
    expect(moveFailureMessage(new Error('boom'), 'HR', 'Design')).toBe(
      'Couldn\'t move “HR” into “Design”: boom'
    );
    expect(moveFailureMessage(undefined, 'HR', 'Design')).toBe('Couldn\'t move “HR” into “Design”.');
  });

  it('say which parent an un-nest failed on', () => {
    expect(
      removeFailureMessage({ message: 'Failed to remove node from collection: nope' }, 'HR', 'DeutNet')
    ).toBe('Couldn\'t remove “HR” from “DeutNet”: nope');
  });

  it('say a name is taken when a rename is refused for that', () => {
    expect(renameFailureMessage({ message: 'x', code: 'COLLECTION_EXISTS' }, 'HR', 'People')).toBe(
      'A collection named “People” already exists.'
    );
  });

  it('say the collection changed when a rename is refused for a stale version', () => {
    const stale = 'The list is up to date now: try renaming it again.';
    // As the rename command reports it: an update error with the daemon's Aborted status.
    expect(
      renameFailureMessage(
        {
          message: 'Failed to rename collection: Version conflict on hr: expected 7, got 8',
          code: 'UPDATE_ERROR',
          details: 'Aborted'
        },
        'HR',
        'People'
      )
    ).toBe(`“HR” was changed somewhere else. ${stale}`);
    // By the status alone, and by the message alone.
    expect(
      renameFailureMessage({ message: 'x', code: 'UPDATE_ERROR', details: 'Aborted' }, 'HR', 'People')
    ).toBe(`“HR” was changed somewhere else. ${stale}`);
    expect(
      renameFailureMessage(new Error('Version conflict on hr: expected 7, got 8'), 'HR', 'People')
    ).toBe(`“HR” was changed somewhere else. ${stale}`);
  });

  it('carry the reason for any other failed rename', () => {
    expect(
      renameFailureMessage({ message: 'Failed to rename collection: disk full' }, 'HR', 'People')
    ).toBe('Couldn\'t rename “HR”: disk full');
    expect(renameFailureMessage(undefined, 'HR', 'People')).toBe('Couldn\'t rename “HR”.');
  });

  it('say whether a collection can go anywhere without building the list', () => {
    expect(hasMoveTarget(COLLECTIONS, 'design')).toBe(true);
    expect(hasMoveTarget([collection('only', 'Only')], 'only')).toBe(false);
    expect(hasMoveTarget(COLLECTIONS, 'design', new Set(['company-policies', 'deutnet', 'hr']))).toBe(false);
  });

  it('compare names without letter case or surrounding space', () => {
    expect(sameName('  People ', 'people')).toBe(true);
    expect(sameName('People', 'Peoples')).toBe(false);
  });
});
