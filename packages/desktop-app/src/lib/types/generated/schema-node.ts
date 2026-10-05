// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { RelationshipPath } from './relationship-path';
import type { SchemaChildrenRule } from './schema-children-rule';
import type { SchemaField } from './schema-field';
import type { SchemaParentRule } from './schema-parent-rule';
import type { SchemaRelationship } from './schema-relationship';

/**
 * A schema: the definition of a node type (ADR-086 §1).
 *
 * The one wire shape of a schema, on every surface. The store fills it: the
 * fields, the structural rules and the templates come from the schema node's
 * row, and `relationships` and `extends` from the schema's declaration edges
 * in the `relationship` table (ADR-070, ADR-078). Every schema read returns
 * one the store built from both, so a `SchemaNode` a reader receives always
 * carries its relationships and its parent.
 *
 * A schema's description is not a field: it is the schema node's child
 * subtree.
 */
export type SchemaNode = {
  isCore: boolean;
  /**
   * An abstract type is never instantiated: no node is created with it as
   * its `node_type` or retyped into it. It stays a valid `extends` target
   * and query scope (ADR-086 §6).
   */
  abstract?: boolean;
  /**
   * The schema id of the type this one extends (ADR-078). Stored as the
   * schema's `extends` edge, never as an entry in `relationships`.
   */
  extends?: string;
  /**
   * Which children this type's nodes may have: the rule this type itself
   * declares, on top of what it inherits (ADR-089).
   */
  children?: SchemaChildrenRule;
  /**
   * Where this type's nodes may sit in the tree: the rule this type itself
   * declares, on top of what it inherits (ADR-089).
   */
  parent?: SchemaParentRule;
  schemaVersion: number;
  /**
   * The fields this schema itself declares. A read of one schema's
   * definition reports the effective set instead: these, then the ones
   * inherited through `extends`.
   */
  fields: Array<SchemaField>;
  /**
   * The relationships this schema declares to other types, stored as
   * declaration edges between schema nodes (ADR-070). A read of one
   * schema's definition adds the inherited ones, like `fields`.
   */
  relationships: Array<SchemaRelationship>;
  /**
   * Template for a node's indexed title, with `{field_name}` tokens, e.g.
   * `"{first_name} {last_name}"`. When set, the title is interpolated from
   * the node's fields rather than taken from its content.
   */
  titleTemplate?: string;
  /**
   * Template for the property summary shown under a node's title, in the
   * same `{field_name}` syntax. Evaluated by the client and never stored
   * on a node.
   */
  propertiesHeaderSummaryTemplate?: string;
  /**
   * The paths from a node of this type to the nodes that govern it: what
   * a context read follows when it is given no paths (ADR-094 §2). The
   * ones this schema itself declares; a type's context paths are its
   * ancestors' and then its own, and a read of one schema's definition
   * reports that set.
   */
  contextPaths?: Array<RelationshipPath>;
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties: Record<string, unknown>;
  mentions?: Array<string>;
  mentionedIn?: Array<NodeReference>;
  title?: string | null;
  lifecycleStatus: string;
};
