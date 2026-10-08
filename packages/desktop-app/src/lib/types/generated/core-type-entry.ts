// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CoreNodeType } from './core-node-type';
import type { StructuralRules } from './structural-rules';
import type { TypeCategory } from './type-category';

/**
 * A core type's registry entry, as the frontend reads it.
 */
export type CoreTypeEntry = {
  /**
   * The stored `node_type`.
   */
  id: CoreNodeType;
  /**
   * The core type this one `extends`.
   */
  parent: CoreNodeType | null;
  /**
   * Whether nodes of exactly this type may exist.
   */
  abstract: boolean;
  /**
   * What "strongly typed" amounts to for the type: a primitive declares no
   * fields and keeps its data as `content`.
   */
  category: TypeCategory;
  /**
   * Offered by the `@` mention picker (the effective rule: it narrows down the chain).
   */
  mentionable: boolean;
  /**
   * Left out of default queries, counts and lists, a page outline among
   * them (the effective rule: it widens down the chain).
   */
  excludedFromDefaultQueries: boolean;
  /**
   * Whether the type's typed fields are written through a typed update
   * command. A type without one writes them as a `properties` patch keyed
   * by storage name.
   */
  typedUpdate: boolean;
  /**
   * The structural rules the type itself declares, before inheritance.
   */
  structure: StructuralRules;
};
