// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { DecisionStatus } from './decision-status';

/**
 * Partial update for a decision's core field, received from the frontend.
 *
 * `decision_status` can be set but not cleared (the schema requires it).
 */
export type DecisionNodeUpdate = { decisionStatus?: DecisionStatus };
