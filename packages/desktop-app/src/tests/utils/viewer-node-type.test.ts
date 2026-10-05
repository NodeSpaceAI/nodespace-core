import { afterEach, describe, it, expect } from 'vitest';
import { setTypeResolver } from '$lib/types/core-node-types';
import { resolveViewerFallback, resolveViewerNodeType } from '$lib/utils/viewer-node-type';

describe('resolveViewerNodeType', () => {
  it('swaps a chat tab to the viewer of its node\'s new subtype when the node is retyped', () => {
    // The tab opened on a native chat; picking a harness retypes the node.
    expect(resolveViewerNodeType('ai-chat-native', 'ai-chat-native')).toBe('ai-chat-native');
    expect(resolveViewerNodeType('ai-chat-native', 'ai-chat-pty')).toBe('ai-chat-pty');
  });

  it('resolves a tab opened under the abstract base through the node', () => {
    expect(resolveViewerNodeType('ai-chat', 'ai-chat-pty')).toBe('ai-chat-pty');
  });

  it('keeps the tab\'s own type until the node has hydrated', () => {
    expect(resolveViewerNodeType('ai-chat-native', undefined)).toBe('ai-chat-native');
  });

  it('keeps the tab\'s own type when the node is no longer a chat', () => {
    expect(resolveViewerNodeType('ai-chat-native', 'text')).toBe('ai-chat-native');
  });

  it('never overrides a non-chat tab, whose type is a routing choice (a schema opened as query)', () => {
    expect(resolveViewerNodeType('query', 'schema')).toBe('query');
    expect(resolveViewerNodeType('text', 'ai-chat-pty')).toBe('text');
  });
});

describe('resolveViewerFallback', () => {
  // `board` extends `collection`; `wall` extends `board`.
  const parents: Record<string, string> = { board: 'collection', wall: 'board' };
  const hasViewerIn = (types: string[]) => (t: string) => types.includes(t);

  afterEach(() => setTypeResolver(() => undefined));

  function declareSubtypes(): void {
    setTypeResolver((id) => (parents[id] ? ({ extends: parents[id] } as never) : undefined));
  }

  it('resolves a subtype with no viewer of its own to its parent\'s viewer', () => {
    declareSubtypes();
    expect(resolveViewerFallback('board', hasViewerIn(['collection']))).toBe('collection');
  });

  it('takes the nearest ancestor that has a viewer', () => {
    declareSubtypes();
    expect(resolveViewerFallback('wall', hasViewerIn(['collection', 'board']))).toBe('board');
    expect(resolveViewerFallback('wall', hasViewerIn(['collection']))).toBe('collection');
  });

  it('lets a subtype\'s own viewer win over its parent\'s', () => {
    declareSubtypes();
    expect(resolveViewerFallback('board', hasViewerIn(['collection', 'board']))).toBe('board');
  });

  it('keeps the type itself when nothing in its chain has a viewer', () => {
    declareSubtypes();
    expect(resolveViewerFallback('board', hasViewerIn([]))).toBe('board');
    expect(resolveViewerFallback('unrelated', hasViewerIn(['collection']))).toBe('unrelated');
  });
});
