/**
 * Core node type registry helpers (ADR-086 §3).
 *
 * `CORE_NODE_TYPES` is generated from Rust's `CoreNodeType` registry
 * (`./generated`, `packages/nodespace-types/src/core_type.rs`), so the two
 * cannot list different types.
 *
 * This module is the one place that spells a core type id out as a literal in
 * a comparison. Everywhere else asks a question of it:
 * - `isA(nodeType, 'task')` — "does a task's rule apply to this node?". True for
 *   `task` and for every type that `extends` it, user-defined or not (ADR-086
 *   §5), so use it wherever a type's behaviour should reach its subtypes.
 * - `isExactly(nodeType, 'task')` — "is this node's stored type exactly `task`?".
 *   Right only where a subtype must not match: a wire shape, which a subtype
 *   never borrows from its base, and the `schema` meta-type, which nothing can
 *   extend.
 */

import { CORE_NODE_TYPES } from './generated';
import type { CoreNodeType, CoreTypeEntry } from './generated';

export { CORE_NODE_TYPES };
export type { CoreTypeEntry };

/** The stored `node_type` of a type NodeSpace itself ships. */
export type CoreNodeTypeId = CoreNodeType;

const CORE_ENTRIES: ReadonlyMap<string, CoreTypeEntry> = new Map(
  CORE_NODE_TYPES.map((t) => [t.id, t])
);

/** Look up a user-defined type's `extends` target, or `undefined` when it has none. */
type ExtendsResolver = (nodeType: string) => string | undefined;

let resolveExtends: ExtendsResolver = () => undefined;

/**
 * Set where user-defined types' `extends` targets come from: the loaded
 * schemas. Called once by the schema store, which holds them; the resolver
 * reads reactive state, so a derived value that asks `isA` re-evaluates when
 * the schemas load or change.
 */
export function setExtendsResolver(resolver: ExtendsResolver): void {
  resolveExtends = resolver;
}

/** Whether `id` is exactly a type NodeSpace itself ships. */
export function isCoreNodeType(id: string | null | undefined): id is CoreNodeTypeId {
  return id != null && CORE_ENTRIES.has(id);
}

/** The registry entry for a core type id, or `undefined` for any other type. */
export function coreTypeEntry(id: string | null | undefined): CoreTypeEntry | undefined {
  return id == null ? undefined : CORE_ENTRIES.get(id);
}

/** A type followed by its ancestors, nearest first. Core types resolve statically. */
export function typeChain(nodeType: string): string[] {
  const chain: string[] = [];
  let current: string | undefined = nodeType;
  while (current !== undefined && !chain.includes(current)) {
    chain.push(current);
    const core = CORE_ENTRIES.get(current);
    current = core ? (core.parent ?? undefined) : resolveExtends(current);
  }
  return chain;
}

/** The nearest core type in a type's `extends` chain: itself when it is core. */
export function nearestCoreType(nodeType: string | null | undefined): CoreNodeTypeId | undefined {
  if (nodeType == null) return undefined;
  return typeChain(nodeType).find(isCoreNodeType);
}

/**
 * Whether `nodeType` is `base` or descends from it through `extends`.
 *
 * The question to ask wherever a type's rule applies to its subtypes.
 */
export function isA(nodeType: string | null | undefined, base: string): boolean {
  if (nodeType == null) return false;
  return typeChain(nodeType).includes(base);
}

/**
 * Whether `nodeType` is exactly `id`, and not a subtype of it.
 *
 * Right only where a subtype must not match: a wire shape, and the `schema`
 * meta-type.
 */
export function isExactly(nodeType: string | null | undefined, id: string): boolean {
  return nodeType === id;
}
