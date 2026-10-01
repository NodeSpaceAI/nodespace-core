// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A graph write completed during an assistant turn.
 *
 * Mirrors `nodespace_core::models::AiChatCompletedWrite`.
 */
export type AiChatCompletedWrite = {
  tool: string;
  nodeId?: string;
  summary?: string;
  /**
   * Edges the write evicted, rendered `"from -[type]-> to"`. Only a
   * cardinality-one `create_relationship` populates it.
   */
  replaced?: Array<string>;
  /**
   * The write's identity for the cross-turn duplicate guard: canonical JSON
   * verbatim, or `sha256:<hex>` of it when too large to store. Always
   * present — this is the struct that serialises to the frontend, so making
   * it optional here would contradict the TypeScript mirror.
   */
  canonicalArgs: string;
};
