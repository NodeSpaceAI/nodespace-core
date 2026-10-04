/**
 * Relationship change notifications.
 *
 * A typed relationship can be created or deleted by anything that talks to the
 * daemon (another pane, the CLI, an agent). The sync listeners report every
 * such edge here, and a surface showing a node's relationships subscribes to
 * re-read them when an edge touching its node changes.
 *
 * `has_child` edges are not reported: the structure tree owns the hierarchy,
 * and an import produces them by the thousand.
 */

/** Called with the bare ids of the changed edge's two ends. */
export type RelationshipChangeListener = (_fromId: string, _toId: string) => void;

const listeners = new Set<RelationshipChangeListener>();

/** Subscribe to relationship changes. Returns the unsubscribe function. */
export function onRelationshipChanged(listener: RelationshipChangeListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Report a created or deleted edge to every subscriber. */
export function notifyRelationshipChanged(fromId: string, toId: string): void {
  for (const listener of listeners) listener(fromId, toId);
}
