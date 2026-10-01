import { describe, it, expect } from 'vitest';
import { resolveViewerNodeType } from '$lib/utils/viewer-node-type';

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
