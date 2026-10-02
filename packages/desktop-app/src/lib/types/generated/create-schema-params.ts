// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { SchemaChildrenRule } from './schema-children-rule';
import type { SchemaField } from './schema-field';
import type { SchemaParentRule } from './schema-parent-rule';
import type { SchemaRelationship } from './schema-relationship';

/**
 * Parameters of `create_schema`.
 */
export type CreateSchemaParams = {
  /**
   * Schema name (e.g., "Invoice", "Customer"). The schema id is derived
   * from it.
   */
  name: string;
  /**
   * Brief prose summary of what this entity type represents. Stored as the
   * schema node's child subtree for semantic discovery; not parsed into
   * fields.
   */
  description?: string;
  /**
   * Explicit field definitions. Required: `[]` declares a type with no
   * fields, an absent key is refused.
   */
  fields?: Array<SchemaField>;
  /**
   * Schema id of a parent type this schema specializes (ADR-078).
   *
   * Structural vocabulary, on the same footing as `fields`, not a
   * relationship the caller authors: `extends` and `extended_by` are
   * refused in `relationships`. Declaring it composes this schema's
   * effective field set as its own fields plus the parent's (additive
   * only, single parent, no override), and instances created under this
   * schema carry *this* schema's id as their `node_type`.
   */
  extends?: string;
  /**
   * Declare the type abstract (ADR-086 §6): it can be extended and
   * queried, but no node is created with it as its `node_type` or retyped
   * into it. Only its subtypes are instantiated.
   */
  abstract?: boolean;
  /**
   * Which children this type's nodes may have (ADR-089): `{"rule": "any"}`
   * (the default), `{"rule": "none"}`, or
   * `{"rule": "any_except", "types": [...]}`. A named type covers its
   * subtypes. A subtype inherits its base's rule and may only tighten it.
   */
  children?: SchemaChildrenRule;
  /**
   * Where this type's nodes may sit in the tree (ADR-089):
   * `{"rule": "any"}` (the default), `{"rule": "must_be_root"}`, or
   * `{"rule": "must_have_parent_of", "types": [...]}`.
   */
  parent?: SchemaParentRule;
  /**
   * Relationship definitions to other schemas.
   */
  relationships?: Array<SchemaRelationship>;
  /**
   * Template for computing a node's title from its field values, with
   * `{field_name}` tokens that reference fields defined in `fields`.
   * Example: `"{first_name} {last_name}"` for a customer schema.
   */
  title_template?: string;
  /**
   * Template for the property summary shown under a node's title, in the
   * same `{field_name}` syntax. Evaluated by the client.
   * Example: `"{status} · {company}"` → `"Active · Acme Corp"`.
   */
  properties_header_summary_template?: string;
};
