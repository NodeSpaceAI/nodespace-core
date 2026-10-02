// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * Partial update for the settings node's core fields, received from the
 * frontend.
 *
 * `required_extensions` is tri-state: absent leaves it unchanged, `null`
 * clears it (it then reads as empty), and a list replaces it whole.
 */
export type DatabaseSettingsNodeUpdate = { requiredExtensions?: Array<string> | null };
