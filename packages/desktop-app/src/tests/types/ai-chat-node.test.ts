import { describe, it, expect } from 'vitest';
import { isAiChatNode, nodeToAiChatNode, type AiChatMessage } from '$lib/types/ai-chat-node';
import type { Node } from '$lib/types/node';

function makeNode(overrides: Record<string, unknown> = {}): Node {
  return {
    id: 'n1',
    nodeType: 'text',
    content: 'hello',
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    properties: {},
    ...overrides
  } as unknown as Node;
}

describe('isAiChatNode', () => {
  it('returns true for a node with nodeType "ai-chat"', () => {
    const node = makeNode({ nodeType: 'ai-chat' });
    expect(isAiChatNode(node)).toBe(true);
  });

  it('returns false for a node with a different nodeType', () => {
    const node = makeNode({ nodeType: 'text' });
    expect(isAiChatNode(node)).toBe(false);
  });

  it('returns false for a task node', () => {
    const node = makeNode({ nodeType: 'task' });
    expect(isAiChatNode(node)).toBe(false);
  });
});

describe('nodeToAiChatNode', () => {
  it('passes through id, content, version, createdAt, modifiedAt verbatim', () => {
    const node = makeNode({
      id: 'chat-1',
      content: 'some content',
      version: 7,
      createdAt: '2026-01-01T00:00:00Z',
      modifiedAt: '2026-01-02T00:00:00Z'
    });

    const chat = nodeToAiChatNode(node);

    expect(chat.id).toBe('chat-1');
    expect(chat.content).toBe('some content');
    expect(chat.version).toBe(7);
    expect(chat.createdAt).toBe('2026-01-01T00:00:00Z');
    expect(chat.modifiedAt).toBe('2026-01-02T00:00:00Z');
    expect(chat.nodeType).toBe('ai-chat');
  });

  it('defaults turnStatus to "idle" and sessionStatus to "active" when absent', () => {
    const node = makeNode();
    const chat = nodeToAiChatNode(node);
    expect(chat.turnStatus).toBe('idle');
    expect(chat.sessionStatus).toBe('active');
  });

  it('preserves explicit turnStatus and sessionStatus independently', () => {
    const node = makeNode({ turnStatus: 'processing', sessionStatus: 'archived' });
    const chat = nodeToAiChatNode(node);
    expect(chat.turnStatus).toBe('processing');
    expect(chat.sessionStatus).toBe('archived');
  });

  it('defaults messages to [] when missing', () => {
    const node = makeNode();
    const chat = nodeToAiChatNode(node);
    expect(chat.messages).toEqual([]);
  });

  it('defaults messages to [] when present but not an array', () => {
    const node = makeNode({ messages: 'not-an-array' });
    const chat = nodeToAiChatNode(node);
    expect(chat.messages).toEqual([]);
  });

  it('preserves a valid messages array', () => {
    const messages: AiChatMessage[] = [
      { role: 'user', content: 'hi' },
      { role: 'assistant', content: 'hello there' }
    ];
    const node = makeNode({ messages });
    const chat = nodeToAiChatNode(node);
    expect(chat.messages).toEqual(messages);
  });

  it('carries through optional lifecycleStatus/provider/model when present', () => {
    const node = makeNode({
      lifecycleStatus: 'archived',
      provider: 'openai-compat',
      model: 'gpt-4o'
    });
    const chat = nodeToAiChatNode(node);
    expect(chat.lifecycleStatus).toBe('archived');
    expect(chat.provider).toBe('openai-compat');
    expect(chat.model).toBe('gpt-4o');
  });

  it('leaves lifecycleStatus/provider/model undefined when absent', () => {
    const node = makeNode();
    const chat = nodeToAiChatNode(node);
    expect(chat.lifecycleStatus).toBeUndefined();
    expect(chat.provider).toBeUndefined();
    expect(chat.model).toBeUndefined();
  });
});
