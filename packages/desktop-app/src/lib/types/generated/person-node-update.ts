// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Partial update for a person's core fields, received from the frontend.
 *
 * Each field is tri-state: absent leaves it unchanged, `null` clears it, and
 * a string sets it.
 */
export type PersonNodeUpdate = {
  firstName?: string | null;
  lastName?: string | null;
  email?: string | null;
};
