// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { IssueStatus } from './issue-status';
import type { LinkValue } from './link-value';
import type { Priority } from './priority';

/**
 * Partial update for an issue's core fields, received from the frontend.
 *
 * The fields of a [`TaskNodeUpdate`], with the issue's status vocabulary,
 * and the issue's own: `pull_request`, and `commits`, which is replaced
 * whole. `status` has no clear path (the schema requires it); the other
 * fields are tri-state: absent leaves the field unchanged, `null` clears it,
 * and a value sets it.
 *
 * `content` is an envelope field and extension fields (`custom:…`) live in
 * `properties`; both are written through the generic node update.
 */
export type IssueNodeUpdate = {
  status?: IssueStatus;
  priority?: Priority | null;
  dueDate?: string | null;
  startedAt?: string | null;
  completedAt?: string | null;
  pullRequest?: LinkValue | null;
  commits?: Array<LinkValue> | null;
};
