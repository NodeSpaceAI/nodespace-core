// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a project stands.
 *
 * The core statuses are strongly typed; a status a user added to the schema
 * (`user_values`) is `User(String)`. The default, [`Self::Planning`], is the
 * project schema's declared default: what a project with no stored status
 * has.
 */
export type ProjectStatus = 'planning' | 'active' | 'completed' | 'cancelled' | string;
