// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * How an agent turn ended — the structural record ADR-038's "at most one
 * clarification per intent" contract is enforced from.
 *
 * The session is rebuilt from persisted messages every turn, so whether the
 * current intent already asked the user something has to be answerable from
 * the history alone. The reply's text cannot answer it: a turn that asked in
 * its own words carries no marker, and reading the model's prose for a
 * question is the unreliable channel ADR-038 built `route_clarify` to avoid.
 * What the turn *did* — change the graph, ask through the clarify composer,
 * or neither — is known exactly when it ends, so that is recorded.
 */
export type AiChatTurnOutcome = 'acted' | 'clarified' | 'replied';
