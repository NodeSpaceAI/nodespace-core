import { describe, it, expect } from 'vitest';
import { readTabDrop } from '$lib/utils/tab-drop';

const item = { tab: { id: 't1' }, paneId: 'pane-1' };

describe('readTabDrop', () => {
  it('reads a pointerup with no target as a click, not a failed drop', () => {
    // sveltednd starts a drag on every pointerdown and calls onDrop on pointerup;
    // a plain click on a tab never sets the target container.
    expect(readTabDrop({ draggedItem: item, sourceContainer: 'tab-2', targetContainer: null })).toEqual({
      kind: 'click'
    });
  });

  it('reads source and target indices from a real drop', () => {
    expect(readTabDrop({ draggedItem: item, sourceContainer: 'tab-0', targetContainer: 'tab-3' })).toEqual({
      kind: 'drop',
      sourceIndex: 0,
      targetIndex: 3
    });
  });

  it('reports missing state when there is no dragged item or no source', () => {
    expect(readTabDrop({ draggedItem: null, sourceContainer: 'tab-0', targetContainer: 'tab-1' })).toEqual({
      kind: 'invalid',
      reason: 'missing-state'
    });
    expect(readTabDrop({ draggedItem: item, sourceContainer: '', targetContainer: 'tab-1' })).toEqual({
      kind: 'invalid',
      reason: 'missing-state'
    });
  });

  it('reports a container that is not a tab index', () => {
    expect(readTabDrop({ draggedItem: item, sourceContainer: 'tab-0', targetContainer: 'pane' })).toEqual({
      kind: 'invalid',
      reason: 'unparseable-index'
    });
  });
});
