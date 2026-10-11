// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { Priority } from './priority';
import type { ProjectStatus } from './project-status';

/**
 * Partial update for a project's core fields, received from the frontend.
 *
 * `status` has no clear path (the schema requires it); the other fields are
 * tri-state: absent leaves the field unchanged, `null` clears it, and a value
 * sets it. Dates accept `YYYY-MM-DD` or RFC 3339 and are stored as
 * `YYYY-MM-DD`.
 */
export type ProjectNodeUpdate = {
  status?: ProjectStatus;
  priority?: Priority | null;
  startDate?: string | null;
  endDate?: string | null;
};
