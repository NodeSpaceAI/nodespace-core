// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * One successful write tool call, as its message's `wrote` edge records it.
 *
 * The agent session is rebuilt from the stored conversation on every turn, so
 * this is the durable evidence that a write happened. Tool results are not
 * kept: the record establishes that the write happened and lets a later call
 * be recognised as the same write. The one exception is the edges a
 * relationship write evicted (`replaced`), a side effect the call's
 * arguments do not describe and a later turn needs to undo it.
 */
export type AiChatWrite = {
  /**
   * The call's position among the message's writes, across all of its
   * `wrote` edges.
   */
  seq: number;
  /**
   * Name of the tool that performed the write (`create_node`, ...).
   */
  tool: string;
  /**
   * Short human-readable label for what was written, when available.
   */
  summary?: string;
  /**
   * The call's arguments, canonicalised. With `tool` this is the write's
   * identity for the cross-turn duplicate guard: a later call matching both
   * is the same write. Either the canonical JSON verbatim or, when that is
   * too large to store, `sha256:<hex>` of it; canonical JSON always starts
   * with `{`, so the two forms cannot be confused.
   */
  canonical_args: string;
  /**
   * Edges the write evicted, each rendered `"from -[type]-> to"`. Only a
   * relationship write that replaced a cardinality-one edge has any.
   */
  replaced?: Array<string>;
};
