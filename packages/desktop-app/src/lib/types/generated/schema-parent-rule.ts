// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a type's nodes may sit in the `has_child` tree, as a schema declares
 * it (ADR-089). A named type covers its subtypes.
 */
export type SchemaParentRule =
  | { rule: 'any' }
  | { rule: 'must_be_root' }
  | { rule: 'must_have_parent_of'; types: Array<string> };
