// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { SpecStatus } from './spec-status';

/**
 * Partial update for a spec's core fields, received from the frontend.
 *
 * `spec_status` can be set but not cleared (the schema requires it); the
 * text fields are tri-state: absent leaves the field unchanged, `null`
 * clears it, and a string sets it.
 */
export type SpecNodeUpdate = {
  objective?: string | null;
  boundaries?: string | null;
  specStatus?: SpecStatus;
};
