// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * How skill search ranks a skill (ADR-038). A closed vocabulary: search
 * compares against `procedure` by name.
 */
export type SkillRole = 'tool' | 'procedure';
