/**
 * Writes to a native chat's messages (ADR-088 §3): each message is an
 * `ai-chat-message` child of the chat, in conversation order.
 */

import { v4 as uuidv4 } from 'uuid';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { InsertPosition } from '$lib/services/backend-adapter';

/**
 * Create the user's message as the last child of `chatId`.
 *
 * It goes through the store's normal create path, so it shows immediately and
 * persists with the chat as its parent. The fields are written as properties
 * under their canonical snake_case storage names (the type has no typed update
 * command) and promoted to the top level so the optimistic node reads like the
 * one the daemon later confirms.
 *
 * Returns the new message's id.
 */
export function appendUserMessage(chatId: string, content: string, viewerId: string): string {
  const id = uuidv4();
  const now = new Date().toISOString();
  const siblings = structureTree.getChildrenWithOrder(chatId);
  const order = siblings.length > 0 ? siblings[siblings.length - 1].order + 1 : 1;

  const insertPosition: InsertPosition = { type: 'end' };
  const message = {
    id,
    nodeType: 'ai-chat-message',
    content,
    lifecycleStatus: 'active',
    version: 1,
    createdAt: now,
    modifiedAt: now,
    properties: { role: 'user', timestamp: now },
    mentions: [],
    role: 'user',
    timestamp: now,
    insertPosition
  };

  structureTree.addInMemoryRelationship(chatId, id, order);
  sharedNodeStore.setNode(message, { type: 'viewer', viewerId });
  return id;
}

/**
 * Take back a message {@link appendUserMessage} showed but could not store, so
 * the conversation holds only what was sent. Nothing is deleted in the
 * backend: the message never reached it.
 */
export function discardUnsentMessage(messageId: string, viewerId: string): void {
  structureTree.removeNode(messageId);
  sharedNodeStore.deleteNode(messageId, { type: 'viewer', viewerId }, true);
}
