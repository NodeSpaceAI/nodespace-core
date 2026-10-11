// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Where an issue stands (ADR-098).
 *
 * An issue has every status a task has, and two of its own for review:
 * `changes_requested` and `ready_to_merge`. Any other string is a status a
 * user added. Read at `task` scope, each of the two reads as the task status
 * [`Self::task_status`] names.
 */
export type IssueStatus =
  | 'open'
  | 'in_progress'
  | 'in_review'
  | 'changes_requested'
  | 'ready_to_merge'
  | 'done'
  | 'cancelled'
  | string;
