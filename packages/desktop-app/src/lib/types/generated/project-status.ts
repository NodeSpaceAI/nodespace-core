// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a project stands.
 *
 * The four core statuses are named; any other string is a status a user
 * added to the project schema. A project with no stored status is
 * `planning`, the schema's declared default.
 */
export type ProjectStatus = 'planning' | 'active' | 'completed' | 'cancelled' | string;
