// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { LinkValue } from './link-value';
import type { Priority } from './priority';
import type { ProjectStatus } from './project-status';

/**
 * Partial update for a software project's core fields, received from the
 * frontend.
 *
 * The fields of a [`ProjectNodeUpdate`] and the software project's own:
 * `repository` and `checkout_path`. `status` has no clear path (the schema
 * requires it); the other fields are tri-state: absent leaves the field
 * unchanged, `null` clears it, and a value sets it.
 */
export type SoftwareProjectNodeUpdate = {
  status?: ProjectStatus;
  priority?: Priority | null;
  startDate?: string | null;
  endDate?: string | null;
  repository?: LinkValue | null;
  checkoutPath?: string | null;
};
