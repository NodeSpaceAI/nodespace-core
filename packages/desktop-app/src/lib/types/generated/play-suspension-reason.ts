// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Why the engine took a play out of service on this device (ADR-087 §5).
 */
export type PlaySuspensionReason =
  'validation_failed' | 'action_failed' | 'cycle_limit' | 'schema_drift';
