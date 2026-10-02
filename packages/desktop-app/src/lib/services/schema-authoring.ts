/**
 * Schema-instance authoring helper.
 *
 * Backs the "+ New" action in `QueryNodeViewer`: mint a fresh, empty node of a
 * given schema type so the caller can open it immediately for schema-driven
 * field entry. Unlike `collection-authoring`'s `createNodeInCollection`, there
 * is no membership edge — a schema-type query is a flat `nodeType` filter, not
 * a `member_of` grouping — so this helper only creates the node.
 */

import { isA, isExactly } from '$lib/types/core-node-types';
import { v4 as uuidv4 } from 'uuid';
import { backendAdapter } from '$lib/services/backend-adapter';
import { humanizeSchemaId } from '$lib/plugins/schema-plugin-loader';
import { getDefaultAiChatModelProperties, NATIVE_CHAT_AGENT } from '$lib/services/ai-chat-default-model';
import { UNTITLED_CHAT_TITLE } from '$lib/utils/ai-chat-title';
import type { Node } from '$lib/types';
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { resolveFieldValue } from '$lib/components/schema/schema-field-resolution';
import { isUserVisibleField } from '$lib/utils/schema-field-visibility';

/**
 * Core types whose `content` field IS the node's name, not body text — their
 * `NodeBehavior::validate()` rejects empty content. Creating one with `content: ''`
 * (the default for body-content types) always fails validation, so these seed a
 * type-identifying placeholder instead. Kept as an explicit list: there is no
 * frontend signal (schema field, `isCore`, field count) that distinguishes these
 * from primitive body-content types like `task`/`text`, so inferring the set would
 * silently drift as new Core types are added.
 *
 * The rule is the base type's, so it reaches every type that extends one of
 * these. `tool` is abstract: only its subtypes are created, and each has
 * required fields of its own (`tool-native` needs a `handler`), so this content
 * default alone does not make a tool creatable via "+New"; it keeps the content
 * rule uniform across the family.
 */
const NAME_AS_CONTENT_TYPES = ['project', 'skill', 'collection', 'agent-guidance', 'tool'] as const;

/**
 * The seed `content` a fresh instance of `typeId` starts with — shared by the
 * immediate create and the unsaved placeholder so both start identically.
 *
 * Every chat seeds the bare `"Untitled"`: that exact value is the sentinel the
 * daemon's background titler tests for before generating a title, so it has to
 * match `UNTITLED_CHAT_TITLE` byte for byte — `humanizeSchemaId` would yield
 * "Untitled Ai Chat Native" and the titler would read it as a user-chosen
 * title and leave it alone forever.
 */
function seedContent(typeId: string): string {
  if (isA(typeId, 'ai-chat')) return UNTITLED_CHAT_TITLE;
  return NAME_AS_CONTENT_TYPES.some((base) => isA(typeId, base))
    ? `Untitled ${humanizeSchemaId(typeId)}`
    : '';
}

/**
 * Fields a user must fill before the backend will accept an instance: required,
 * with no default to fall back on, and editable by the user. This mirrors the
 * backend validator, which rejects a node missing such a field. Keep it in step
 * with the required-field check in the backend's node validation: if the two
 * disagree, a create the frontend allows is rejected.
 */
export function requiredFieldsWithoutDefault(schema: SchemaNode | null): SchemaField[] {
  return (schema?.fields ?? []).filter(
    (f) => f.required === true && f.default === undefined && isUserVisibleField(f)
  );
}

function isFieldFilled(node: Node, field: SchemaField): boolean {
  const value = resolveFieldValue(node, field.name);
  if (value === null || value === undefined) return false;
  return typeof value !== 'string' || value.trim() !== '';
}

/** The names of `required` fields still missing a value on `node`. */
export function missingRequiredFields(node: Node, required: SchemaField[]): string[] {
  return required.filter((f) => !isFieldFilled(node, f)).map((f) => f.name);
}

/**
 * Whether "+ New" on this type must open an unsaved placeholder instead of
 * creating the node right away: the schema has required fields without a
 * default, so an empty instance would be rejected.
 */
export function needsUnsavedPlaceholder(schema: SchemaNode | null): boolean {
  return requiredFieldsWithoutDefault(schema).length > 0;
}

/**
 * Open a new instance of `schema`'s type that exists only in the store. It is
 * written to the backend by the store once every required field without a
 * default has a value (see `SharedNodeStore.createUnsavedPlaceholder`), and is
 * dropped silently if the tab showing it closes first.
 */
export function createInstancePlaceholder(schema: SchemaNode): Node {
  const required = requiredFieldsWithoutDefault(schema);
  const now = new Date().toISOString();
  const node: Node = {
    lifecycleStatus: 'active',
    id: uuidv4(),
    nodeType: schema.id,
    content: seedContent(schema.id),
    version: 1,
    createdAt: now,
    modifiedAt: now,
    properties: {},
    mentions: []
  };
  sharedNodeStore.createUnsavedPlaceholder(
    node,
    (candidate) => missingRequiredFields(candidate, required).length === 0
  );
  return node;
}

/**
 * Mint a fresh instance of the given schema type and return the created node.
 *
 * The node is created as a root (`parentId: null`) with no properties; the
 * schema-driven form UI fills in the fields once the node is opened. An
 * `ai-chat-native` is the exception: it is seeded with its required `agent`
 * and the user's default model, if any. `nodeType` is the schema's id — the
 * same key `QueryNodeViewer` queries on — so the new node matches that type's
 * result list. Body-content types start empty ("start typing"); name-as-content
 * Core types (see `NAME_AS_CONTENT_TYPES`) seed `"Untitled {Type}"` so they
 * pass their non-empty-content validation; chats seed `"Untitled"`.
 */
export async function createSchemaInstance(typeId: string): Promise<Node> {
  const newId = uuidv4();
  const content = seedContent(typeId);
  // A new native chat starts on the user's default model, written at creation
  // so the node records it from the first moment (no later write to race an echo).
  const properties: Record<string, unknown> = isExactly(typeId, 'ai-chat-native')
    ? { agent: NATIVE_CHAT_AGENT, ...getDefaultAiChatModelProperties() }
    : {};
  await backendAdapter.createNode({
    id: newId,
    nodeType: typeId,
    content,
    properties,
    mentions: [],
    parentId: null,
  });
  // Deliberate second round-trip: createNode doesn't return the node, but the caller
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
