import { describe, it, expect } from 'vitest';
import {
  isAiChatNode,
  isAiChatNativeNode,
  isAiChatPtyNode,
  nodeToAiChatNativeNode,
  nodeToAiChatPtyNode,
  type AiChatMessage
} from '$lib/types/ai-chat-node';
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

describe('chat node guards', () => {
  it('isAiChatNode is true for both subtypes and false for anything else', () => {
    expect(isAiChatNode(makeNode({ nodeType: 'ai-chat-native' }))).toBe(true);
    expect(isAiChatNode(makeNode({ nodeType: 'ai-chat-pty' }))).toBe(true);
    expect(isAiChatNode(makeNode({ nodeType: 'text' }))).toBe(false);
    expect(isAiChatNode(makeNode({ nodeType: 'task' }))).toBe(false);
  });

  it('the subtype guards are false for the abstract base, which no node ever is', () => {
    expect(isAiChatNativeNode(makeNode({ nodeType: 'ai-chat' }))).toBe(false);
    expect(isAiChatPtyNode(makeNode({ nodeType: 'ai-chat' }))).toBe(false);
  });

  it('the subtype guards each match only their own type', () => {
    const native = makeNode({ nodeType: 'ai-chat-native' });
    const pty = makeNode({ nodeType: 'ai-chat-pty' });
    expect(isAiChatNativeNode(native)).toBe(true);
    expect(isAiChatNativeNode(pty)).toBe(false);
    expect(isAiChatPtyNode(pty)).toBe(true);
    expect(isAiChatPtyNode(native)).toBe(false);
  });
});

describe('nodeToAiChatNativeNode', () => {
  it('passes through id, content, version, createdAt, modifiedAt verbatim', () => {
    const chat = nodeToAiChatNativeNode(
      makeNode({
        id: 'chat-1',
        content: 'some content',
        version: 7,
        createdAt: '2026-01-01T00:00:00Z',
        modifiedAt: '2026-01-02T00:00:00Z',
        nodeType: 'ai-chat-native'
      })
    );

    expect(chat.id).toBe('chat-1');
    expect(chat.content).toBe('some content');
    expect(chat.version).toBe(7);
    expect(chat.createdAt).toBe('2026-01-01T00:00:00Z');
    expect(chat.modifiedAt).toBe('2026-01-02T00:00:00Z');
    expect(chat.nodeType).toBe('ai-chat-native');
  });

  it('fills the schema defaults when the wire shape omits a field', () => {
    const chat = nodeToAiChatNativeNode(makeNode());
    expect(chat.agent).toBe('');
    expect(chat.provider).toBe('native');
    expect(chat.turnStatus).toBe('idle');
    expect(chat.contextTokens).toBe(0);
    expect(chat.messages).toEqual([]);
  });

  it('defaults messages to [] when present but not an array', () => {
    expect(nodeToAiChatNativeNode(makeNode({ messages: 'not-an-array' })).messages).toEqual([]);
  });

  it('preserves a valid messages array', () => {
    const messages: AiChatMessage[] = [
      { role: 'user', content: 'hi' },
      { role: 'assistant', content: 'hello there' }
    ];
    expect(nodeToAiChatNativeNode(makeNode({ messages })).messages).toEqual(messages);
  });

  it('carries through the declared fields when present', () => {
    const chat = nodeToAiChatNativeNode(
      makeNode({
        lifecycleStatus: 'archived',
        agent: 'nodespace',
        provider: 'openai-compat',
        model: 'gpt-4o',
        turnStatus: 'processing',
        contextTokens: 120,
        summary: 'about gpt',
        lastActive: '2026-01-03T00:00:00Z'
      })
    );
    expect(chat.lifecycleStatus).toBe('archived');
    expect(chat.agent).toBe('nodespace');
    expect(chat.provider).toBe('openai-compat');
    expect(chat.model).toBe('gpt-4o');
    expect(chat.turnStatus).toBe('processing');
    expect(chat.contextTokens).toBe(120);
    expect(chat.summary).toBe('about gpt');
    expect(chat.lastActive).toBe('2026-01-03T00:00:00Z');
  });

  it('leaves the optional fields undefined when absent', () => {
    const chat = nodeToAiChatNativeNode(makeNode());
    expect(chat.lifecycleStatus).toBeUndefined();
    expect(chat.model).toBeUndefined();
    expect(chat.summary).toBeUndefined();
    expect(chat.lastActive).toBeUndefined();
  });
});

describe('nodeToAiChatPtyNode', () => {
  it('reads the typed session fields', () => {
    const chat = nodeToAiChatPtyNode(
      makeNode({
        nodeType: 'ai-chat-pty',
        agent: 'claude-code',
        sessionStatus: 'ended',
        sessionId: 's-1',
        transcript: 'hi',
        summary: 'did a thing',
        exitCode: 0
      })
    );
    expect(chat.nodeType).toBe('ai-chat-pty');
    expect(chat.agent).toBe('claude-code');
    expect(chat.sessionStatus).toBe('ended');
    expect(chat.sessionId).toBe('s-1');
    expect(chat.transcript).toBe('hi');
    expect(chat.summary).toBe('did a thing');
    expect(chat.exitCode).toBe(0);
  });

  it('defaults sessionStatus to "active" and leaves unset session fields undefined', () => {
    const chat = nodeToAiChatPtyNode(makeNode());
    expect(chat.sessionStatus).toBe('active');
    expect(chat.sessionId).toBeUndefined();
    expect(chat.transcript).toBeUndefined();
    expect(chat.exitCode).toBeUndefined();
  });

  it('carries no native fields', () => {
    const chat = nodeToAiChatPtyNode(makeNode({ messages: [{ role: 'user', content: 'x' }] }));
    expect('messages' in chat).toBe(false);
    expect('turnStatus' in chat).toBe(false);
  });
});
