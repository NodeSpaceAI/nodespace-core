// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { FieldRename } from './field-rename';
import type { FieldValueAddition } from './field-value-addition';
import type { RelationshipPath } from './relationship-path';
import type { SchemaChildrenRule } from './schema-children-rule';
import type { SchemaField } from './schema-field';
import type { SchemaParentRule } from './schema-parent-rule';
import type { SchemaRelationship } from './schema-relationship';

/**
 * Parameters of `update_schema`: a batch of changes to one schema.
 */
export type UpdateSchemaParams = {
  /**
   * Schema id to update.
   */
  schema_id: string;
  /**
   * Fields to add.
   */
  add_fields?: Array<SchemaField>;
  /**
   * Field names to remove.
   */
  remove_fields?: Array<string>;
  /**
   * Append values to an existing field's `userValues` (ADR-076). Gated on
   * that field being `extensible`. Append-only: never touches
   * `coreValues`, never removes or renames existing user values.
   */
  add_field_values?: Array<FieldValueAddition>;
  /**
   * Field renames. An identity rename rekeys the property data of every
   * existing node of this type together with the schema definition.
   */
  rename_fields?: Array<FieldRename>;
  /**
   * Set or change this schema's parent type (ADR-078). Absent leaves the
   * current parent untouched; there is no way to clear one.
   *
   * Re-targeting is validated exactly as creation is: the new parent must
   * exist, must not introduce a cycle, and must not collide with a field
   * this schema (or a remaining ancestor) already declares.
   */
  extends?: string;
  /**
   * Relationships to add.
   */
  add_relationships?: Array<SchemaRelationship>;
  /**
   * Relationship names to remove. `extends` and `extended_by` are refused
   * here: the parent can only be re-targeted, through `extends` above.
   */
  remove_relationships?: Array<string>;
  /**
   * New description.
   */
  description?: string;
  /**
   * Make the type abstract (`true`) or concrete (`false`); absent leaves it
   * unchanged. A type that already has nodes of its own cannot become
   * abstract: no node may have an abstract type (ADR-086 §6).
   */
  abstract?: boolean;
  /**
   * Replace the type's `children` rule (ADR-089); absent leaves it
   * unchanged. Refused when it relaxes the base type's rule, or when a
   * node of the type already breaks the new one.
   */
  children?: SchemaChildrenRule;
  /**
   * Replace the type's `parent` rule (ADR-089); absent leaves it
   * unchanged. Refused like `children`.
   */
  parent?: SchemaParentRule;
  /**
   * Set the title template; absent leaves it unchanged. `{field_name}`
   * tokens reference fields defined in the schema.
   */
  title_template?: string;
  /**
   * Set the properties header summary template; absent leaves it
   * unchanged. Same `{field_name}` syntax, evaluated by the client.
   */
  properties_header_summary_template?: string;
  /**
   * Context paths to add (ADR-094 §2): the relationship paths from a node
   * of this type to the nodes that govern it, which a context read follows
   * by default. Each is a list of hops, `["spec", "decisions"]`, and is
   * checked against the schemas as they stand: a name the type does not
   * declare is refused. Allowed on a core schema.
   */
  add_context_paths?: Array<RelationshipPath>;
  /**
   * Context paths to remove, each written as it was added. Only a path
   * this schema itself declares can be removed here: an inherited one is
   * removed on the schema that declares it.
   */
  remove_context_paths?: Array<RelationshipPath>;
  /**
   * Proceed even if active plays would be affected. When false (the
   * default), such an update is refused with the affected plays listed.
   */
  force?: boolean;
};
