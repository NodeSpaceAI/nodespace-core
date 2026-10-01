// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { Action } from './action';
import type { RuleClass } from './rule-class';
import type { Trigger } from './trigger';

/**
 * One rule of a play: when it runs, what must hold, and what it does.
 *
 * `conditions` are CEL expressions over the triggering node; all must pass.
 */
export type RuleDefinition = {
  name: string;
  class?: RuleClass;
  trigger: Trigger;
  conditions?: Array<string>;
  actions?: Array<Action>;
};
