// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { SchemaChildrenRule } from './schema-children-rule';
import type { SchemaParentRule } from './schema-parent-rule';

/**
 * A type's two structural rules (ADR-089).
 */
export type StructuralRules = {
  /**
   * Which children the type's nodes may have.
   */
  children: SchemaChildrenRule;
  /**
   * Where the type's nodes may sit in the `has_child` tree.
   */
  parent: SchemaParentRule;
};
