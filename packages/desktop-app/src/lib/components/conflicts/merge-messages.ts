/**
 * User-facing text for the Conflicts view's merge action: why a merge was
 * refused and what to do about it, and where the merged node will live.
 * Pure functions over already-resolved labels so the pane stays thin.
 */
import type { TreeInvariantViolationData } from '$lib/types/errors';
import type { MergePreview } from '$lib/stores/conflicts.svelte';

/** Resolves a node id to the label shown to the user. */
export type LabelOf = (nodeId: string) => string;

function quoted(label: string): string {
  return `"${label}"`;
}

function joinLabels(labels: string[]): string {
  const q = labels.map(quoted);
  if (q.length <= 1) return q.join('');
  return `${q.slice(0, -1).join(', ')} and ${q[q.length - 1]}`;
}

/**
 * Explain a merge refused by a tree invariant, naming the step the user has
 * to take before the merge can go through.
 */
export function describeMergeRefusal(
  violation: TreeInvariantViolationData,
  survivorId: string,
  loserId: string,
  labelOf: LabelOf
): string {
  const survivor = quoted(labelOf(survivorId));
  const loser = quoted(labelOf(loserId));
  switch (violation.rule) {
    case 'member_of_not_root': {
      // `node_id` is whichever side holds the membership, not necessarily
      // the survivor.
      const holder = quoted(labelOf(violation.node_id ?? survivorId));
      const collections = joinLabels(violation.related_ids.map(labelOf));
      const where = collections || 'its collections';
      return (
        `Can't merge: the merged node would sit under a parent while ${holder} belongs to ${where}, ` +
        `and only top-level pages can belong to a collection. Remove ${holder} from ${where} first, then merge.`
      );
    }
    case 'cycle': {
      const other = quoted(labelOf(violation.related_ids[0] ?? loserId));
      return (
        `Can't merge: ${survivor} sits inside ${other}'s subtree, so it would become its own ancestor. ` +
        `Move ${survivor} out of ${other}'s subtree first, or keep ${other} instead.`
      );
    }
    case 'collection_not_root':
      // Keeping the loser instead would fold a collection into a plain node,
      // so the only advice is to lift the loser out of its parent.
      return (
        `Can't merge: ${survivor} is a collection, and collections can't sit under a parent. ` +
        `Move ${loser} to the top level first, then merge.`
      );
  }
}

/**
 * The sentence the merge confirmation adds about position, or `null` when
 * both nodes already sit in the same place and nothing moves.
 */
export function describeSurvivorPosition(
  preview: MergePreview,
  survivorId: string,
  loserId: string,
  labelOf: LabelOf
): string | null {
  if (preview.survivorParentId === preview.loserParentId) return null;

  const survivor = quoted(labelOf(survivorId));
  const loser = quoted(labelOf(loserId));
  const keeps = preview.resultingParentId
    ? `The merged node ${survivor} will sit under ${quoted(labelOf(preview.resultingParentId))}.`
    : `The merged node ${survivor} stays a top-level page.`;
  const dropped =
    preview.loserParentId && preview.loserParentId !== preview.resultingParentId
      ? ` ${loser} is removed from under ${quoted(labelOf(preview.loserParentId))}.`
      : '';
  return keeps + dropped;
}
