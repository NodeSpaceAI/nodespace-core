// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Partial update for a collection's core fields, received from the frontend.
 *
 * `description` is tri-state: absent leaves it unchanged, `null` clears it,
 * and a string sets it. The collection's name is `content`, an envelope
 * field, and is written through the rename operation.
 */
export type CollectionNodeUpdate = { description?: string | null };
