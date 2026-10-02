// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * One hop of a [`RelationshipPath`].
 *
 * `name` is a relationship as the author names it: a built-in (`has_child`,
 * `member_of`, `mentions`, `has_role`), a schema-declared relationship
 * (`blocks`, `tasks`), or the reverse name of either (`child_of`,
 * `blocked_by`, `project`). The direction of travel is implied by which name
 * was used.
 *
 * An open-ended hop follows the relationship repeatedly: `child_of` reaches
 * the parent, an open-ended `child_of` reaches every ancestor.
 *
 * On the wire a fixed hop is its bare name and an open-ended hop is
 * `{ "name": "child_of", "open_ended": true }`.
 */
export type RelationshipHop = string | { name: string; open_ended: boolean };
