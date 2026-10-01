import { isA } from '$lib/types/core-node-types';

/**
 * The node type a tab's viewer is chosen by.
 *
 * A tab records the type it opened with, and that normally decides the viewer.
 * A chat is the exception: it can be retyped in place (a native chat becomes a
 * terminal chat when a harness is picked), and a chat tab may even have opened
 * under the abstract `ai-chat` base. For the chat family the node's current
 * type wins, so the pane swaps viewers when the node's type changes; every
 * other tab keeps the type it opened with (a schema id opened as `query` is not
 * a node of that type).
 */
export function resolveViewerNodeType(
  tabNodeType: string,
  currentNodeType: string | undefined
): string {
  if (!isA(tabNodeType, 'ai-chat')) return tabNodeType;
  return isA(currentNodeType, 'ai-chat') ? (currentNodeType as string) : tabNodeType;
}
