import { describe, it, expect, afterEach } from 'vitest';
import {
  coreTypeEntry,
  isA,
  isCoreNodeType,
  isExactly,
  nearestCoreType,
  setExtendsResolver,
  typeChain
} from '$lib/types/core-node-types';

describe('type helpers', () => {
  afterEach(() => setExtendsResolver(() => undefined));

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
    setExtendsResolver((id) => parents[id]);

    expect(typeChain('bug')).toEqual(['bug', 'issue', 'task']);
    expect(isA('bug', 'task')).toBe(true);
    expect(isA('issue', 'task')).toBe(true);
    expect(isA('issue', 'bug')).toBe(false);
    expect(isA('issue', 'text')).toBe(false);
    expect(nearestCoreType('bug')).toBe('task');
  });

  it('does not loop on a cyclic extends chain', () => {
    const parents: Record<string, string> = { a: 'b', b: 'a' };
    setExtendsResolver((id) => parents[id]);
    expect(typeChain('a')).toEqual(['a', 'b']);
    expect(isA('a', 'task')).toBe(false);
  });

  it('leaves a type with no known parent as its own chain', () => {
    expect(typeChain('invoice')).toEqual(['invoice']);
    expect(nearestCoreType('invoice')).toBeUndefined();
  });

  it('keeps isExactly strict for a subtype', () => {
    setExtendsResolver((id) => (id === 'issue' ? 'task' : undefined));
    expect(isExactly('task', 'task')).toBe(true);
    expect(isExactly('issue', 'task')).toBe(false);
    expect(isExactly(undefined, 'task')).toBe(false);
  });
});
