/**
 * `CORE_NODE_TYPES` is the frontend mirror of Rust's `CoreNodeType` registry
 * (`packages/nodespace-types/src/core_type.rs`). The two are hand-synced across
 * the language boundary, so this reads the Rust source and asserts they list
 * the same ids, parents, abstract flags and mentionable flags.
 */

import { describe, it, expect, afterEach } from 'vitest';
import {
  CORE_NODE_TYPES,
  coreTypeEntry,
  isA,
  isCoreNodeType,
  isExactly,
  nearestCoreType,
  setExtendsResolver,
  typeChain
} from '$lib/types/core-node-types';
import { rustCoreTypes } from '../helpers/rust-core-type-registry';

describe('CORE_NODE_TYPES', () => {
  it('lists the core types Rust CoreNodeType::ALL lists, in the same order', () => {
    const rust = rustCoreTypes();
    expect(rust.length).toBeGreaterThan(0);
    expect(CORE_NODE_TYPES.map((t) => t.id)).toEqual(rust.map((t) => t.id));
  });

  it('matches the Rust registry on parent, abstract and mentionable, per type', () => {
    const ts = CORE_NODE_TYPES.map(({ id, parent, abstract, mentionable }) => ({
      id,
      parent,
      abstract,
      mentionable
    }));
    const rust = rustCoreTypes().map(({ id, parent, abstract, mentionable }) => ({
      id,
      parent,
      abstract,
      mentionable
    }));
    expect(ts).toEqual(rust);
  });

  it('has unique ids and only parents that are themselves core types', () => {
    const ids = CORE_NODE_TYPES.map((t) => t.id as string);
    expect(new Set(ids).size).toBe(ids.length);
    for (const t of CORE_NODE_TYPES) {
      if (t.parent !== null) expect(ids).toContain(t.parent);
    }
  });
});

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
