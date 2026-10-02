/**
 * The dev-proxy relays the daemon's watch stream to the browser. A node the
 * daemon creates under a parent (a chat reply under its chat) arrives as two
 * events, the node and its `has_child` edge; the browser attaches the node to
 * its parent only from the second. These tests hold the relay to both.
 */

import { describe, it, expect } from 'vitest';
import { watchEventToSse } from '../../../../dev-tools/src/watch-event-mapping';

describe('watchEventToSse', () => {
  it('relays a node event as its id and type', () => {
    expect(watchEventToSse({ created: { id: 'n1', nodeType: 'ai-chat-message' } })).toEqual({
      type: 'nodeCreated',
      nodeId: 'n1',
      nodeType: 'ai-chat-message'
    });
    expect(watchEventToSse({ updated: { id: 'n1', nodeType: 'text' } })).toEqual({
      type: 'nodeUpdated',
      nodeId: 'n1'
    });
    expect(watchEventToSse({ deleted: { nodeId: 'n1', nodeType: 'text' } })).toEqual({
      type: 'nodeDeleted',
      nodeId: 'n1',
      nodeType: 'text'
    });
  });

  it('relays a created has_child edge with bare node ids and its decoded order', () => {
    expect(
      watchEventToSse({
        relationshipCreated: {
          id: 'relationship:chat:reply',
          // The daemon names endpoints by stored record id.
          fromId: 'node:chat',
          toId: 'node:reply',
          relationshipType: 'has_child',
          properties: '{"order":3}'
        }
      })
    ).toEqual({
      type: 'relationshipCreated',
      id: 'relationship:chat:reply',
      fromId: 'chat',
      toId: 'reply',
      relationshipType: 'has_child',
      properties: { order: 3 }
    });
  });

  it('relays an updated and a deleted edge', () => {
    expect(
      watchEventToSse({
        relationshipUpdated: {
          id: 'r1',
          fromId: 'a',
          toId: 'b',
          relationshipType: 'has_child',
          properties: '{"order":1.5}'
        }
      })
    ).toMatchObject({ type: 'relationshipUpdated', fromId: 'a', toId: 'b', properties: { order: 1.5 } });
    expect(
      watchEventToSse({
        relationshipDeleted: {
          id: 'r1',
          fromId: 'node:a',
          toId: 'node:b',
          relationshipType: 'mentions'
        }
      })
    ).toEqual({
      type: 'relationshipDeleted',
      id: 'r1',
      fromId: 'a',
      toId: 'b',
      relationshipType: 'mentions'
    });
  });

  it('reads properties that are absent or not a JSON object as none', () => {
    for (const properties of [undefined, '', 'not json', '[1]', 'null']) {
      expect(
        watchEventToSse({
          relationshipCreated: { id: 'r', fromId: 'a', toId: 'b', relationshipType: 'wrote', properties }
        })
      ).toMatchObject({ properties: {} });
    }
  });

  it('relays nothing for an event with no payload', () => {
    expect(watchEventToSse({})).toBeNull();
  });
});
