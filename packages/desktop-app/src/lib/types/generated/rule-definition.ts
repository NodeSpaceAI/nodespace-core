// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { Action } from './action';
import type { RuleClass } from './rule-class';
import type { RuleCondition } from './rule-condition';
import type { Trigger } from './trigger';

/**
 * One rule of a play: when it runs, what must hold, and what it does.
 *
 * Every condition must pass. `description` says what the rule does, in one
 * sentence.
 */
export type RuleDefinition = {
  name: string;
  description: string;
  class?: RuleClass;
  trigger: Trigger;
  conditions?: Array<RuleCondition>;
  actions?: Array<Action>;
};
