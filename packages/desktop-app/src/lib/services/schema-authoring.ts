/**
 * Schema-instance authoring helper.
 *
 * Backs the "+ New" action in `QueryNodeViewer`: mint a fresh, empty node of a
 * given schema type so the caller can open it immediately for schema-driven
 * field entry. Unlike `collection-authoring`'s `createNodeInCollection`, there
 * is no membership edge — a schema-type query is a flat `nodeType` filter, not
 * a `member_of` grouping — so this helper only creates the node.
 */

import { v4 as uuidv4 } from 'uuid';
import { backendAdapter } from '$lib/services/backend-adapter';
import { humanizeSchemaId } from '$lib/plugins/schema-plugin-loader';
import { UNTITLED_CHAT_TITLE } from '$lib/utils/ai-chat-title';
import type { Node } from '$lib/types';

/**
 * Core types whose `content` field IS the node's name, not body text — their
 * `NodeBehavior::validate()` rejects empty content. Creating one with `content: ''`
 * (the default for body-content types) always fails validation, so these seed a
 * type-identifying placeholder instead. Kept as an explicit list: there is no
 * frontend signal (schema field, `isCore`, field count) that distinguishes these
 * from primitive body-content types like `task`/`text`, so inferring the set would
 * silently drift as new Core types are added.
 *
 * `tool` is listed for constraint-consistency, but note its validation also
 * requires a non-empty `handler` property — this content default alone does not
 * make a bare `tool` creatable via "+New"; it just keeps the content rule uniform
 * for the day a tool-creation path (with a handler) exists.
 */
const NAME_AS_CONTENT_TYPES = new Set(['project', 'skill', 'collection', 'agent-guidance', 'tool']);

/**
 * Types whose seed content is a fixed string rather than the
 * `"Untitled {Type}"` form `NAME_AS_CONTENT_TYPES` derives.
 *
 * `ai-chat` seeds the bare `"Untitled"`: that exact value is the sentinel the
 * daemon's background titler tests for before generating a title, so it has to
 * match `UNTITLED_CHAT_TITLE` byte for byte — `humanizeSchemaId` would yield
 * "Untitled Ai Chat" and the titler would read it as a user-chosen title and
 * leave it alone forever.
 */
const SEED_CONTENT_OVERRIDES = new Map<string, string>([['ai-chat', UNTITLED_CHAT_TITLE]]);

/**
 * Mint a fresh instance of the given schema type and return the created node.
 *
 * The node is created as a root (`parentId: null`) with no properties; the
 * schema-driven form UI fills in the fields once the node is opened. `nodeType`
 * is the schema's id — the same key `QueryNodeViewer` queries on — so the new
 * node matches that type's result list. Body-content types start empty ("start
 * typing"); name-as-content Core types (see `NAME_AS_CONTENT_TYPES`) seed
 * `"Untitled {Type}"` so they pass their non-empty-content validation; types in
 * `SEED_CONTENT_OVERRIDES` seed a fixed string instead.
 */
export async function createSchemaInstance(typeId: string): Promise<Node> {
  const newId = uuidv4();
  const content =
    SEED_CONTENT_OVERRIDES.get(typeId) ??
    (NAME_AS_CONTENT_TYPES.has(typeId) ? `Untitled ${humanizeSchemaId(typeId)}` : '');
  await backendAdapter.createNode({
    id: newId,
    nodeType: typeId,
    content,
    properties: {},
    mentions: [],
    parentId: null,
  });
  // Deliberate second round-trip: createNode returns only the id, but the caller
  // needs the full hydrated Node to seed the shared store — so load it back.
  const created = await backendAdapter.getNode(newId);
  if (!created) {
    throw new Error(`Newly created node ${newId} could not be loaded`);
  }
  return created;
}

/**
 * Decide whether a just-created instance should still be integrated into a
 * viewer's current results.
 *
 * Returns `false` when the active query generation (`loadId`) or database epoch
 * changed while the create was in flight — the node is still persisted, it just
 * must not be injected into a now-stale or switched-away view (ADR-053, the same
 * discipline the viewer's load path applies to its own writes).
 */
export function shouldIntegrateInstance(
  captured: { loadId: number; epoch: number },
  current: { loadId: number; epoch: number }
): boolean {
  return captured.loadId === current.loadId && captured.epoch === current.epoch;
}
