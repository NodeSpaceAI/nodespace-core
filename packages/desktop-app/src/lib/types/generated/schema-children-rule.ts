// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Which children a type's nodes may have, as a schema declares it (ADR-089).
 *
 * A named type covers its subtypes. A subtype inherits its base's rule and
 * may only tighten it: `any` declares nothing, and an `any_except` list adds
 * to the base's.
 */
export type SchemaChildrenRule =
  { rule: 'any' } | { rule: 'none' } | { rule: 'any_except'; types: Array<string> };
