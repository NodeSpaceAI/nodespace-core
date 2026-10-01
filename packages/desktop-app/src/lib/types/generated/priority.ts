// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The relative-urgency scale of the core `task` and `project` types.
 *
 * Core priorities are strongly typed; a priority a user added to the schema
 * (`user_values`, e.g. "critical") is `User(String)`. There is no default:
 * a node without a priority has none.
 */
export type Priority = 'highest' | 'high' | 'medium' | 'low' | 'lowest' | string;
