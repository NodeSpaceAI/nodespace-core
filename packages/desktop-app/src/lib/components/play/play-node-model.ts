/**
 * What `PlayNodeViewer` shows, derived from a play's wire fields (ADR-090 §2).
 *
 * Every function here is pure: the viewer keeps no copy of the play, so each
 * render derives its text from the node the store holds.
 */

import type {
  Action,
  PlayNode,
  RuleDefinition,
  Selector,
  Trigger
} from '$lib/types/generated';

/** A play's state as its header shows it. */
export type PlayState = 'on' | 'off' | 'suspended';

/**
 * `off` when the user switched the play off, `suspended` when the engine
 * stopped it on this device, `on` otherwise. The user's switch wins: a play
 * that is off is not waiting on its suspension.
 */
export function playState(play: Pick<PlayNode, 'enabled' | 'suspendedAt'>): PlayState {
  if (!play.enabled) return 'off';
  return play.suspendedAt ? 'suspended' : 'on';
}

/** The noun a selector's nodes are named by: `task`, `node the saved query selects`. */
function selectorNoun(select: Selector): string {
  if ('query_id' in select) return 'node the saved query selects';
  const noun = select.target_type === '*' ? 'node' : select.target_type;
  const count = select.filters?.length ?? 0;
  if (count === 0) return noun;
  return `${noun} matching ${count} ${count === 1 ? 'filter' : 'filters'}`;
}

function withArticle(noun: string): string {
  return `${/^[aeiou]/i.test(noun) ? 'an' : 'a'} ${noun}`;
}

/** The field a `<type>.<field>` property key names. */
function fieldName(propertyKey: string): string {
  return propertyKey.slice(propertyKey.lastIndexOf('.') + 1);
}

/**
 * A trigger in words. A trigger carries no authored description (ADR-090 §1):
 * its fields describe it exactly, so this is the only text it has.
 */
export function describeTrigger(trigger: Trigger): string {
  const noun = selectorNoun(trigger.select);
  if (trigger.type === 'scheduled') {
    return `On the schedule ${trigger.cron}, for every ${noun}`;
  }
  const target = withArticle(noun);
  switch (trigger.on) {
    case 'node_created':
      return `When ${target} is created`;
    case 'property_changed':
      return trigger.property_key
        ? `When ${fieldName(trigger.property_key)} changes on ${target}`
        : `When any property changes on ${target}`;
    case 'relationship_added':
      return `When a relationship is added from ${target}`;
    case 'relationship_removed':
      return `When a relationship is removed from ${target}`;
  }
}

/** The collection an action runs over, when it has one. */
export function actionForEach(action: Action): string | undefined {
  return 'for_each' in action ? action.for_each : undefined;
}

/** A trigger as authored, for the on-demand raw view. */
export function rawTrigger(trigger: Trigger): string {
  return JSON.stringify(trigger, null, 2);
}

/** An action's content as authored (everything but its description). */
export function rawAction(action: Action): string {
  const { description: _description, ...content } = action;
  return JSON.stringify(content, null, 2);
}

/** The rules a play runs inside the triggering write, fail-closed (ADR-060). */
export function invariantRules(play: Pick<PlayNode, 'rules'>): RuleDefinition[] {
  return play.rules.filter((rule) => rule.class === 'invariant');
}

/**
 * The invariant rules whose effect the ADR-060 §8 warning names before a
 * seeded play is switched off. Empty when switching the play off needs no
 * warning.
 */
export function rulesWarnedOnDisable(play: Pick<PlayNode, 'isSeeded' | 'rules'>): RuleDefinition[] {
  return play.isSeeded ? invariantRules(play) : [];
}
