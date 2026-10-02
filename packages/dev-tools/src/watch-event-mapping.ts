/**
 * Maps one event of the daemon's `WatchNodes` stream to the server-sent event
 * the browser's sync service reads.
 *
 * Node events carry only the id (and type): the browser fetches the node when
 * it needs it. Relationship events are relayed whole, so a hierarchy change
 * made by another writer (the CLI, an agent, the daemon appending a chat
 * reply) reaches the browser's structure tree: without the `has_child` event
 * a node created elsewhere arrives but never attaches to its parent.
 */

export interface ProtoNodeEventData {
  id: string;
  nodeType: string;
}

export interface ProtoRelationshipPayload {
  id: string;
  fromId: string;
  toId: string;
  relationshipType: string;
  /** JSON-encoded edge properties. */
  properties?: string;
}

export interface ProtoRelationshipDeletedPayload {
  id: string;
  fromId: string;
  toId: string;
  relationshipType: string;
}

export interface ProtoWatchEvent {
  created?: ProtoNodeEventData;
  updated?: ProtoNodeEventData;
  deleted?: { nodeId: string; nodeType: string };
  relationshipCreated?: ProtoRelationshipPayload;
  relationshipUpdated?: ProtoRelationshipPayload;
  relationshipDeleted?: ProtoRelationshipDeletedPayload;
}

/** An edge's properties, decoded. A payload that is not a JSON object reads as none. */
function edgeProperties(encoded: string | undefined): Record<string, unknown> {
  if (!encoded) return {};
  try {
    const parsed: unknown = JSON.parse(encoded);
    return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : {};
  } catch {
    return {};
  }
}

/**
 * A relationship event names its endpoints by stored record id, with the
 * `node:` table prefix; the browser keys nodes by bare id.
 */
function bareNodeId(id: string): string {
  return id.startsWith('node:') ? id.slice('node:'.length) : id;
}

/** The server-sent event for `event`, or `null` when it carries none the browser reads. */
export function watchEventToSse(event: ProtoWatchEvent): Record<string, unknown> | null {
  if (event.created) {
    return { type: 'nodeCreated', nodeId: event.created.id, nodeType: event.created.nodeType };
  }
  if (event.updated) {
    return { type: 'nodeUpdated', nodeId: event.updated.id };
  }
  if (event.deleted) {
    return {
      type: 'nodeDeleted',
      nodeId: event.deleted.nodeId,
      nodeType: event.deleted.nodeType
    };
  }
  const changed = event.relationshipCreated ?? event.relationshipUpdated;
  if (changed) {
    return {
      type: event.relationshipCreated ? 'relationshipCreated' : 'relationshipUpdated',
      id: changed.id,
      fromId: bareNodeId(changed.fromId),
      toId: bareNodeId(changed.toId),
      relationshipType: changed.relationshipType,
      properties: edgeProperties(changed.properties)
    };
  }
  if (event.relationshipDeleted) {
    const { id, fromId, toId, relationshipType } = event.relationshipDeleted;
    return {
      type: 'relationshipDeleted',
      id,
      fromId: bareNodeId(fromId),
      toId: bareNodeId(toId),
      relationshipType
    };
  }
  return null;
}
