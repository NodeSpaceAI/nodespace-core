// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The value of a `link` field: a title and the URL it opens.
 *
 * A nested stored value, so its keys are snake_case and an unknown key is
 * refused (ADR-086 §7).
 */
export type LinkValue = {
  title: string;
  /**
   * An absolute URL: one with a scheme and a host. Any scheme is stored;
   * a client decides which schemes it opens.
   */
  url: string;
};
