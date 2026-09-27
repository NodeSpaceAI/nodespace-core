/**
 * Merge copy for the Conflicts view: one refusal message per tree
 * invariant, each naming the step that unblocks the merge, and the
 * position sentence for the confirmation.
 */
import { describe, it, expect } from 'vitest';
import {
  describeMergeRefusal,
  describeSurvivorPosition
} from '$lib/components/conflicts/merge-messages';

const LABELS: Record<string, string> = {
  s: 'Survivor',
  l: 'Loser',
  c1: 'Work',
  c2: 'Home',
  p1: 'People',
  p2: 'Contacts'
};
const labelOf = (id: string) => LABELS[id] ?? id;

describe('describeMergeRefusal', () => {
  it('names every collection to leave for member_of_not_root', () => {
    const text = describeMergeRefusal(
      { rule: 'member_of_not_root', node_id: 's', related_ids: ['c1', 'c2'], detail: '' },
      's',
      'l',
      labelOf
    );
    expect(text).toContain('"Survivor" would end up under a parent while it belongs to "Work" and "Home"');
    expect(text).toContain('Remove it from "Work" and "Home" first, then merge.');
  });

  it('points at the subtree to leave for a cycle', () => {
    const text = describeMergeRefusal(
      { rule: 'cycle', node_id: 's', related_ids: ['l'], detail: '' },
      's',
      'l',
      labelOf
    );
    expect(text).toContain('Move "Survivor" out of "Loser"\'s subtree first, or keep "Loser" instead.');
  });

  it('offers the other side for collection_not_root', () => {
    const text = describeMergeRefusal(
      { rule: 'collection_not_root', node_id: 's', related_ids: [], detail: '' },
      's',
      'l',
      labelOf
    );
    expect(text).toContain('"Survivor" is a collection');
    expect(text).toContain('Move "Loser" to the top level first, or keep "Loser" instead.');
  });
});

describe('describeSurvivorPosition', () => {
  it('is silent when both sides share a parent', () => {
    expect(
      describeSurvivorPosition(
        { survivorParentId: 'p1', loserParentId: 'p1', resultingParentId: 'p1' },
        's',
        'l',
        labelOf
      )
    ).toBeNull();
  });

  it('names the parent the survivor takes from the loser', () => {
    expect(
      describeSurvivorPosition(
        { survivorParentId: null, loserParentId: 'p2', resultingParentId: 'p2' },
        's',
        'l',
        labelOf
      )
    ).toBe('The merged node "Survivor" will sit under "Contacts".');
  });

  it('says the survivor stays top-level and the loser leaves its parent', () => {
    expect(
      describeSurvivorPosition(
        { survivorParentId: null, loserParentId: 'p2', resultingParentId: null },
        's',
        'l',
        labelOf
      )
    ).toBe('The merged node "Survivor" stays a top-level page. "Loser" is removed from under "Contacts".');
  });

  it('says the survivor keeps its parent when the loser was top-level', () => {
    expect(
      describeSurvivorPosition(
        { survivorParentId: 'p1', loserParentId: null, resultingParentId: 'p1' },
        's',
        'l',
        labelOf
      )
    ).toBe('The merged node "Survivor" will sit under "People".');
  });
});
