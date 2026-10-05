// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where a task stands.
 *
 * The five core statuses are named, in workflow order; any other string is a
 * status a user added to the task schema. `in_review` is work that is
 * finished and waiting on review, so a task in review has been started.
 */
export type TaskStatus = 'open' | 'in_progress' | 'in_review' | 'done' | 'cancelled' | string;
