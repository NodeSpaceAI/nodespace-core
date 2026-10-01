/**
 * Core node type registry helpers (ADR-086 §3).
 *
 * `CORE_NODE_TYPES` is generated from Rust's `CoreNodeType` registry
 * (`./generated`, `packages/nodespace-types/src/core_type.rs`), so the two
 * cannot list different types.
 *
 * Each entry carries the type's structural rules (ADR-089): which children its
 * nodes may have, and where they may sit in the tree. The database enforces
 * them on every write; the editor asks `canHaveChild` / `canBeRoot` so it does
 * not offer an indent, outdent or type change the database would refuse.
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
import type {
  CoreNodeType,
  CoreTypeEntry,
  SchemaChildrenRule,
  SchemaParentRule,
  StructuralRules
} from './generated';

export { CORE_NODE_TYPES };
export type { CoreTypeEntry, StructuralRules };

/** The stored `node_type` of a type NodeSpace itself ships. */
export type CoreNodeTypeId = CoreNodeType;

const CORE_ENTRIES: ReadonlyMap<string, CoreTypeEntry> = new Map(
  CORE_NODE_TYPES.map((t) => [t.id, t])
);

/** What a user-defined type's schema declares about its place in the type system. */
export interface TypeDeclaration {
  /** The type it `extends`, when it is a subtype. */
  extends?: string;
  /** Its own `children` rule; absent is `any`. */
  children?: SchemaChildrenRule;
  /** Its own `parent` rule; absent is `any`. */
  parent?: SchemaParentRule;
}

/** Look up a user-defined type's declaration, or `undefined` for a type with no schema. */
type TypeResolver = (nodeType: string) => TypeDeclaration | undefined;

let resolveType: TypeResolver = () => undefined;

/**
 * Set where user-defined types' declarations come from: the loaded schemas.
 * Called once by the schema store, which holds them; the resolver reads
 * reactive state, so a derived value that asks `isA` or `canHaveChild`
 * re-evaluates when the schemas load or change.
 */
export function setTypeResolver(resolver: TypeResolver): void {
  resolveType = resolver;
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
    current = core ? (core.parent ?? undefined) : resolveType(current)?.extends;
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

const ANY: StructuralRules = { children: { rule: 'any' }, parent: { rule: 'any' } };

/** The structural rules a type itself declares: the registry's for a core type. */
function declaredStructure(nodeType: string): StructuralRules {
  const core = CORE_ENTRIES.get(nodeType);
  if (core) return core.structure;
  const declared = resolveType(nodeType);
  return {
    children: declared?.children ?? ANY.children,
    parent: declared?.parent ?? ANY.parent
  };
}

/**
 * The structural rules in force for a type: its own on top of everything its
 * `extends` chain declares. A subtype only tightens, so `none` wins over a
 * list, the `any_except` lists of the chain add up, and the nearest `parent`
 * declaration is the one in force.
 */
export function structuralRules(nodeType: string): StructuralRules {
  let children: SchemaChildrenRule = ANY.children;
  let parent: SchemaParentRule = ANY.parent;
  // Furthest ancestor first, so each nearer declaration lands on top.
  for (const type of typeChain(nodeType).reverse()) {
    const own = declaredStructure(type);
    if (own.children.rule === 'none' || children.rule === 'none') {
      children = { rule: 'none' };
    } else if (own.children.rule === 'any_except') {
      const inherited = children.rule === 'any_except' ? children.types : [];
      children = { rule: 'any_except', types: [...new Set([...inherited, ...own.children.types])] };
    }
    if (own.parent.rule !== 'any') parent = own.parent;
  }
  return { children, parent };
}

/**
 * Whether both structural rules allow a node of `childType` under a node of
 * `parentType`: the child's `parent` rule and the parent's `children` rule.
 */
export function canHaveChild(parentType: string, childType: string): boolean {
  const { parent } = structuralRules(childType);
  if (parent.rule === 'must_be_root') return false;
  if (parent.rule === 'must_have_parent_of' && !parent.types.some((t) => isA(parentType, t))) {
    return false;
  }
  const { children } = structuralRules(parentType);
  if (children.rule === 'none') return false;
  return children.rule !== 'any_except' || !children.types.some((t) => isA(childType, t));
}

/** Whether a node of `nodeType` may sit at the root: its type needs no parent. */
export function canBeRoot(nodeType: string): boolean {
  return structuralRules(nodeType).parent.rule !== 'must_have_parent_of';
}
