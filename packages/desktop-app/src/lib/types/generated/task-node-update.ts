// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { Priority } from './priority';
import type { TaskStatus } from './task-status';

/**
 * Partial update for a task's core fields, received from the frontend.
 *
 * `status` has no clear path (the schema requires it); the other fields are
 * tri-state: absent leaves the field unchanged, `null` clears it, and a value
 * sets it. Dates accept `YYYY-MM-DD` or RFC 3339 and are stored as
 * `YYYY-MM-DD`.
 *
 * The update carries the task schema's fields and nothing else. `content` is
 * an envelope field and extension fields (`custom:…`) live in `properties`;
 * both are written through the generic node update.
 */
export type TaskNodeUpdate = {
  status?: TaskStatus;
  priority?: Priority | null;
  dueDate?: string | null;
  startedAt?: string | null;
  completedAt?: string | null;
};
