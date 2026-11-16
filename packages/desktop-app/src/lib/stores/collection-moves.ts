/**
 * Rules and messages for moving a collection under another one and for renaming
 * a collection from the sidebar.
 *
 * Collections nest through `member_of` and form a graph, not a tree: a
 * collection may sit under several parents, and may never sit inside itself
 * or inside one of its own descendants. The backend enforces the same rules
 * (a cyclic membership is refused at the store), so the checks here exist to
 * answer instantly, with the names of the two collections in the message,
 * before any call is made.
 */

import type { CollectionInfo } from '$lib/services/collection-service';
import { isCommandError } from '$lib/types/errors';

/** The temporary id prefix of a collection whose create has not resolved yet. */
export const PENDING_COLLECTION_PREFIX = 'pending-collection-';

/** The outcome of a move, un-nest or rename: done, or the message to show. */
export type CollectionActionResult = { ok: true } | { ok: false; message: string };

export type MoveRefusal = 'self' | 'already-inside' | 'cycle';

/** The ids of every collection `id` sits in, directly or through other collections. */
export function ancestorIds(collections: readonly CollectionInfo[], id: string): Set<string> {
  const parentsById = new Map(collections.map((c) => [c.id, c.parentCollectionIds ?? []]));
  const seen = new Set<string>();
  const stack = [...(parentsById.get(id) ?? [])];
  while (stack.length > 0) {
    const next = stack.pop()!;
    if (seen.has(next)) continue;
    seen.add(next);
    stack.push(...(parentsById.get(next) ?? []));
  }
  return seen;
}

/** The ids of every collection that sits in `id`, directly or through other collections. */
export function descendantIds(collections: readonly CollectionInfo[], id: string): Set<string> {
  const childrenById = new Map<string, string[]>();
  for (const c of collections) {
    for (const parent of c.parentCollectionIds ?? []) {
      const children = childrenById.get(parent) ?? [];
      children.push(c.id);
      childrenById.set(parent, children);
    }
  }
  const seen = new Set<string>();
  const stack = [...(childrenById.get(id) ?? [])];
  while (stack.length > 0) {
    const next = stack.pop()!;
    if (seen.has(next)) continue;
    seen.add(next);
    stack.push(...(childrenById.get(next) ?? []));
  }
  return seen;
}

/**
 * Why `childId` cannot be put directly under `parentId`, or null when it can:
 * it is the same collection, it already sits directly in it, or the parent is
 * already inside the child (so the child would end up inside itself).
 */
export function refusalForMove(
  collections: readonly CollectionInfo[],
  childId: string,
  parentId: string
): MoveRefusal | null {
  if (childId === parentId) return 'self';
  const child = collections.find((c) => c.id === childId);
  if ((child?.parentCollectionIds ?? []).includes(parentId)) return 'already-inside';
  if (ancestorIds(collections, parentId).has(childId)) return 'cycle';
  return null;
}

/**
 * The collections `childId` can be put under: every collection except itself,
 * the ones it already sits directly in, those inside it, and the root
 * collections an extension keeps out of the tree. Sorted by name.
 */
export function moveTargets(
  collections: readonly CollectionInfo[],
  childId: string,
  rootCollectionIds: ReadonlySet<string> = new Set()
): CollectionInfo[] {
  return collections
    .filter(
      (c) =>
        !rootCollectionIds.has(c.id) &&
        !c.id.startsWith(PENDING_COLLECTION_PREFIX) &&
        refusalForMove(collections, childId, c.id) === null
    )
    .sort((a, b) => a.content.localeCompare(b.content));
}

/** The message for a refused move, naming both collections. */
export function describeRefusal(
  refusal: MoveRefusal,
  childName: string,
  parentName: string
): string {
  switch (refusal) {
    case 'self':
      return `“${childName}” can't be moved into itself.`;
    case 'already-inside':
      return `“${childName}” is already in “${parentName}”.`;
    case 'cycle':
      return `“${childName}” can't go into “${parentName}” because “${parentName}” is already inside “${childName}”.`;
  }
}

/** The text a failed call carries, whether it is an `Error`, a command error or a string. */
export function failureText(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (isCommandError(err)) return err.message;
  if (typeof err === 'string') return err;
  return '';
}

const BACKEND_PREFIXES = [
  'Failed to add node to collection: ',
  'Failed to remove node from collection: ',
  'Failed to rename collection: '
];

function withoutBackendPrefix(text: string): string {
  for (const prefix of BACKEND_PREFIXES) {
    if (text.startsWith(prefix)) return text.slice(prefix.length);
  }
  return text;
}

/** The message for a move the backend refused or could not complete. */
export function moveFailureMessage(err: unknown, childName: string, parentName: string): string {
  const text = failureText(err);
  if (text.includes('collection_cycle')) return describeRefusal('cycle', childName, parentName);
  const detail = withoutBackendPrefix(text);
  return detail
    ? `Couldn't move “${childName}” into “${parentName}”: ${detail}`
    : `Couldn't move “${childName}” into “${parentName}”.`;
}

/** The message for an un-nest the backend refused or could not complete. */
export function removeFailureMessage(err: unknown, childName: string, parentName: string): string {
  const detail = withoutBackendPrefix(failureText(err));
  return detail
    ? `Couldn't remove “${childName}” from “${parentName}”: ${detail}`
    : `Couldn't remove “${childName}” from “${parentName}”.`;
}

/** The message for a rename the backend refused or could not complete. */
export function renameFailureMessage(err: unknown, name: string, newName: string): string {
  const code = isCommandError(err) ? err.code : undefined;
  if (code === 'COLLECTION_EXISTS') return `A collection named “${newName}” already exists.`;
  if (code === 'VERSION_CONFLICT') {
    return `“${name}” was changed somewhere else. Try renaming it again.`;
  }
  const detail = withoutBackendPrefix(failureText(err));
  return detail ? `Couldn't rename “${name}”: ${detail}` : `Couldn't rename “${name}”.`;
}

/** A name compared the way a person would: surrounding space and letter case ignored. */
export function sameName(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}
