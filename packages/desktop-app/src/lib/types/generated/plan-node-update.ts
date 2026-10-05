// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { PlanStatus } from './plan-status';

/**
 * Partial update for a plan's core fields, received from the frontend.
 *
 * `plan_status` can be set but not cleared (the schema requires it); the
 * text fields are tri-state: absent leaves the field unchanged, `null`
 * clears it, and a string sets it.
 */
export type PlanNodeUpdate = {
  approach?: string | null;
  risks?: string | null;
  planStatus?: PlanStatus;
};
