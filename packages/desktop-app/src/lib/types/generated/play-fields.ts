// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { PlaySuspensionReason } from './play-suspension-reason';
import type { RuleDefinition } from './rule-definition';

/**
 * The play schema's fields, decoded from a play node's properties.
 *
 * The only reader of a stored play: storage keys are the schema's field
 * names, hoisted by the store under `properties.play.*`.
 * [`Self::from_properties`] reads that bucket, or the flat shape a node built
 * in memory or a create payload carries.
 */
export type PlayFields = {
  rules: Array<RuleDefinition>;
  /**
   * What the play automates, in one line.
   */
  description?: string;
  /**
   * The user's switch. The engine never changes it.
   */
  enabled: boolean;
  /**
   * Why the engine suspended the play on this device, when it has.
   */
  suspendedReason?: PlaySuspensionReason;
  /**
   * The diagnostic the suspension was logged with.
   */
  suspendedMessage?: string;
  /**
   * When the engine suspended the play (RFC 3339).
   */
  suspendedAt?: string;
};
