/**
 * Reads the drag-and-drop library's state at a tab drop.
 *
 * `@thisux/sveltednd` starts a drag on every pointerdown on a tab and calls the
 * droppable's `onDrop` on pointerup, so a plain click on a tab arrives here as a
 * "drop" with a dragged item and a source but no target container (the target is
 * only set by a `pointerover`, which doesn't fire when the pointer was already on
 * the tab). That is a click, not a failed drop.
 */
export type TabDrop<T> =
  | { kind: 'drop'; draggedItem: T; sourceIndex: number; targetIndex: number }
  /** A pointerup with no target: a click on a tab. Nothing to do. */
  | { kind: 'click' }
  /** The library's state is incomplete or unparseable. */
  | { kind: 'invalid'; reason: 'missing-state' | 'unparseable-index' };

export function readTabDrop<T>(state: {
  draggedItem: T | null | undefined;
  sourceContainer: string | null | undefined;
  targetContainer: string | null | undefined;
}): TabDrop<T> {
  const { draggedItem, sourceContainer, targetContainer } = state;
  if (draggedItem && sourceContainer && !targetContainer) return { kind: 'click' };
  if (!draggedItem || !sourceContainer || !targetContainer) {
    return { kind: 'invalid', reason: 'missing-state' };
  }
  const sourceIndex = parseInt(sourceContainer.replace('tab-', ''));
  const targetIndex = parseInt(targetContainer.replace('tab-', ''));
  if (isNaN(sourceIndex) || isNaN(targetIndex)) {
    return { kind: 'invalid', reason: 'unparseable-index' };
  }
  return { kind: 'drop', draggedItem, sourceIndex, targetIndex };
}
