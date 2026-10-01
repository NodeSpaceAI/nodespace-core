// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * A graph write completed during an assistant turn.
 *
 * Only successful, state-changing tool calls are recorded. This is the durable
 * evidence that a turn's write actually happened: the agent session is rebuilt
 * from scratch on every turn, so without it the next turn sees the user's
 * original instruction alongside a prose claim of completion and no proof the
 * write occurred — and may repeat it.
 *
 * Carries the tool name, the affected node, a short label, and the canonical
 * arguments the call was made with. Tool *results* are not persisted: the
 * purpose is to establish *that* the write happened and to recognise a later
 * call as the same write, not to replay its output. The one exception is the
 * edges a relationship write evicted (`replaced`) — a side effect the call's
 * arguments do not describe, and which a later turn needs to undo it.
 */
export type AiChatCompletedWrite = {
  /**
   * Name of the tool that performed the write (e.g. `"create_node"`).
   */
  tool: string;
  /**
   * ID of the node the write produced or affected, when the tool reported one.
   */
  nodeId?: string;
  /**
   * Short human-readable label for the written node, when available.
   */
  summary?: string;
  /**
   * Edges the write evicted as a side effect, each rendered the way
   * `summary` renders a relationship (`"from -[type]-> to"`). Only
   * `create_relationship` populates it: a cardinality-one end is honored by
   * replacing the prior edge, and this is the only place a later turn can
   * still find the previous holder once the reply prose is gone from
   * history. Empty for every other write.
   */
  replaced?: Array<string>;
  /**
   * The call's arguments, canonicalised (JSON key order normalised, parameter
   * aliases resolved). Together with `tool` this is the write's identity for
   * the cross-turn duplicate guard: a later call matching both is the same
   * write, not a new one.
   *
   * Two forms, both produced by `canonical_args_identity`: the canonical JSON
   * verbatim when it is small enough to store, or `sha256:<hex>` of that same
   * string when it is not (see `CANONICAL_ARGS_MAX_CHARS`). The digest keeps
   * large writes — an entire markdown import, say — guarded without copying
   * their content into this message history a second time. The forms cannot
   * be confused: canonical JSON always starts with `{`.
   *
   * Always present. An identity is what makes a recorded write enforceable,
   * so a write recorded without one would be indistinguishable from an
   * unguarded tool while still looking wired up.
   */
  canonicalArgs: string;
};
