import { describe, it, expect, afterEach } from 'vitest';
import {
  CORE_NODE_TYPES,
  canBeRoot,
  canHaveChild,
  coreTypeEntry,
  isA,
  isCoreNodeType,
  isExactly,
  isOwnedByParentViewer,
  nearestCoreType,
  setTypeResolver,
  structuralRules,
  typeChain,
  type TypeDeclaration
} from '$lib/types/core-node-types';

/** A user-defined type that only declares the type it extends. */
function declared(parent: string | undefined): TypeDeclaration | undefined {
  return parent === undefined ? undefined : { extends: parent };
}

describe('type helpers', () => {
  afterEach(() => setTypeResolver(() => undefined));

  it('recognizes exactly the shipped types as core', () => {
    expect(isCoreNodeType('task')).toBe(true);
    expect(isCoreNodeType('ai-chat')).toBe(true);
    expect(isCoreNodeType('issue')).toBe(false);
    expect(isCoreNodeType(undefined)).toBe(false);
    expect(coreTypeEntry('collection')?.mentionable).toBe(false);
    expect(coreTypeEntry('issue')).toBeUndefined();
  });

  it('answers isA for a core type through the static registry', () => {
    expect(isA('task', 'task')).toBe(true);
    expect(isA('task', 'text')).toBe(false);
    expect(isA(undefined, 'task')).toBe(false);
    expect(isA(null, 'task')).toBe(false);
  });

  it('resolves a user-defined subtype through the loaded schemas', () => {
    const parents: Record<string, string> = { issue: 'task', bug: 'issue' };
    setTypeResolver((id) => declared(parents[id]));

    expect(typeChain('bug')).toEqual(['bug', 'issue', 'task']);
    expect(isA('bug', 'task')).toBe(true);
    expect(isA('issue', 'task')).toBe(true);
    expect(isA('issue', 'bug')).toBe(false);
    expect(isA('issue', 'text')).toBe(false);
    expect(nearestCoreType('bug')).toBe('task');
  });

  it('does not loop on a cyclic extends chain', () => {
    const parents: Record<string, string> = { a: 'b', b: 'a' };
    setTypeResolver((id) => declared(parents[id]));
    expect(typeChain('a')).toEqual(['a', 'b']);
    expect(isA('a', 'task')).toBe(false);
  });

  it('leaves a type with no known parent as its own chain', () => {
    expect(typeChain('invoice')).toEqual(['invoice']);
    expect(nearestCoreType('invoice')).toBeUndefined();
  });

  it('keeps isExactly strict for a subtype', () => {
    setTypeResolver((id) => (id === 'issue' ? { extends: 'task' } : undefined));
    expect(isExactly('task', 'task')).toBe(true);
    expect(isExactly('issue', 'task')).toBe(false);
    expect(isExactly(undefined, 'task')).toBe(false);
  });
});

describe('structural rules', () => {
  afterEach(() => setTypeResolver(() => undefined));

  const ROOT_ONLY = ['collection', 'schema', 'date'];
  const LEAVES = [
    'code-block',
    'ordered-list',
    'horizontal-line',
    'table',
    'query',
    'tool',
    // A leaf through the rule it inherits from `tool`.
    'tool-native',
    'database-settings'
  ];
  // Only ever a leaf under a native chat; covered by the chat message tests below.
  const MESSAGES = ['ai-chat-message'];

  it('keeps collection, schema and date at the root', () => {
    for (const type of ROOT_ONLY) {
      expect(structuralRules(type).parent, type).toEqual({ rule: 'must_be_root' });
      expect(canHaveChild('text', type), type).toBe(false);
      // Each still takes children of its own.
      expect(canHaveChild(type, 'text'), type).toBe(true);
      expect(canBeRoot(type), type).toBe(true);
    }
  });

  it('gives the leaf types no children', () => {
    for (const type of LEAVES) {
      expect(structuralRules(type).children, type).toEqual({ rule: 'none' });
      expect(canHaveChild(type, 'text'), type).toBe(false);
      // A leaf may itself be a child.
      expect(canHaveChild('text', type), type).toBe(true);
    }
  });

  it('leaves every other core type open, a chat included', () => {
    const open = CORE_NODE_TYPES.map((t) => t.id as string).filter(
      (id) => !ROOT_ONLY.includes(id) && !LEAVES.includes(id) && !MESSAGES.includes(id)
    );
    expect(open).toContain('ai-chat');
    for (const type of open) {
      expect(structuralRules(type), type).toEqual({
        children: { rule: 'any' },
        parent: { rule: 'any' }
      });
      expect(canHaveChild(type, 'text'), type).toBe(true);
      expect(canHaveChild('text', type), type).toBe(true);
    }
  });

  it('applies a base type rule to a user-defined subtype', () => {
    const schemas: Record<string, TypeDeclaration> = {
      team: { extends: 'collection' },
      'saved-search': { extends: 'query' }
    };
    setTypeResolver((id) => schemas[id]);

    expect(canHaveChild('text', 'team')).toBe(false);
    expect(canHaveChild('team', 'text')).toBe(true);
    expect(canHaveChild('saved-search', 'text')).toBe(false);
  });

  it('reads the rules a user-defined type declares', () => {
    const schemas: Record<string, TypeDeclaration> = {
      thread: {},
      'support-thread': { extends: 'thread' },
      reply: {
        children: { rule: 'none' },
        parent: { rule: 'must_have_parent_of', types: ['thread'] }
      },
      journal: { children: { rule: 'any_except', types: ['task'] } },
      issue: { extends: 'task' }
    };
    setTypeResolver((id) => schemas[id]);

    // A named type covers its subtypes, on both rules.
    expect(canHaveChild('thread', 'reply')).toBe(true);
    expect(canHaveChild('support-thread', 'reply')).toBe(true);
    expect(canHaveChild('text', 'reply')).toBe(false);
    expect(canBeRoot('reply')).toBe(false);
    expect(canHaveChild('reply', 'text')).toBe(false);

    expect(canHaveChild('journal', 'text')).toBe(true);
    expect(canHaveChild('journal', 'task')).toBe(false);
    expect(canHaveChild('journal', 'issue')).toBe(false);
  });

  it('composes a subtype rule on top of its base, tightening only', () => {
    const schemas: Record<string, TypeDeclaration> = {
      journal: { children: { rule: 'any_except', types: ['task'] } },
      'private-journal': {
        extends: 'journal',
        children: { rule: 'any_except', types: ['person'] }
      },
      'sealed-journal': { extends: 'private-journal', children: { rule: 'none' } },
      // A subtype's `any` declares nothing: the base's rule stays in force.
      'plain-search': { extends: 'query', children: { rule: 'any' } },
      thread: {},
      'support-thread': { extends: 'thread' },
      reply: { parent: { rule: 'must_have_parent_of', types: ['thread'] } },
      'support-reply': {
        extends: 'reply',
        parent: { rule: 'must_have_parent_of', types: ['support-thread'] }
      }
    };
    setTypeResolver((id) => schemas[id]);

    expect(structuralRules('private-journal').children).toEqual({
      rule: 'any_except',
      types: ['task', 'person']
    });
    expect(structuralRules('sealed-journal').children).toEqual({ rule: 'none' });
    expect(structuralRules('plain-search').children).toEqual({ rule: 'none' });
    // The nearest parent declaration is the one in force.
    expect(canHaveChild('thread', 'support-reply')).toBe(false);
    expect(canHaveChild('support-thread', 'support-reply')).toBe(true);
  });
});

describe('chat message structure (ADR-088 §3)', () => {
  afterEach(() => setTypeResolver(() => undefined));

  it('lives only directly under a native chat', () => {
    expect(canHaveChild('ai-chat-native', 'ai-chat-message')).toBe(true);
    expect(canHaveChild('ai-chat-pty', 'ai-chat-message')).toBe(false);
    expect(canHaveChild('text', 'ai-chat-message')).toBe(false);
  });

  it('is refused every position an indent, outdent or drag could move it to', () => {
    // Indent under a sibling message: a message holds no children.
    expect(canHaveChild('ai-chat-message', 'ai-chat-message')).toBe(false);
    // Outdent to the chat's parent (a page) or to the root.
    expect(canHaveChild('text', 'ai-chat-message')).toBe(false);
    expect(canBeRoot('ai-chat-message')).toBe(false);
  });

  it('is owned by its parent viewer, unlike an outline row', () => {
    expect(isOwnedByParentViewer('ai-chat-message')).toBe(true);
    expect(isOwnedByParentViewer('text')).toBe(false);
    expect(isOwnedByParentViewer('ai-chat-native')).toBe(false);
    expect(isOwnedByParentViewer('task')).toBe(false);
  });

  it('is inherited by a subtype of an excluded core type', () => {
    setTypeResolver((id) => (id === 'pinned-message' ? { extends: 'ai-chat-message' } : undefined));
    expect(isOwnedByParentViewer('pinned-message')).toBe(true);
  });

  it('is not claimed by a user type that only declares a parent rule', () => {
    setTypeResolver((id) =>
      id === 'reply'
        ? { extends: 'text', parent: { rule: 'must_have_parent_of', types: ['thread'] } }
        : undefined
    );
    expect(canBeRoot('reply')).toBe(false);
    expect(isOwnedByParentViewer('reply')).toBe(false);
    expect(isOwnedByParentViewer('unknown-type')).toBe(false);
  });
});
