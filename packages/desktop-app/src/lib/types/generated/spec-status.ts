// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a spec stands (ADR-092 §1). A closed vocabulary: the seeded rules
 * compare against `approved` and `superseded` by name, so a value added
 * beside them would be one no rule understands.
 */
export type SpecStatus = 'draft' | 'approved' | 'superseded';
