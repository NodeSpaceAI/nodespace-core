// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { CoreNodeType } from './core-node-type';

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
   * Offered by the `@` mention picker (the effective rule: it narrows down the chain).
   */
  mentionable: boolean;
};
