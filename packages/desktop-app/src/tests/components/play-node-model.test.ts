/**
 * The play viewer's model helpers: the play's state, a trigger in words, the
 * raw content shown on demand, and which rules the switch-off warning names.
 */
import { describe, it, expect } from 'vitest';
import {
  actionForEach,
  describeTrigger,
  playState,
  rawAction,
  rulesWarnedOnDisable
} from '$lib/components/play/play-node-model';
import type { Action, RuleDefinition, Trigger } from '$lib/types/generated';

function rule(name: string, ruleClass: 'invariant' | 'reactive'): RuleDefinition {
  return {
    name,
    description: `${name} description`,
    class: ruleClass,
    trigger: { type: 'graph_event', on: 'node_created', select: { target_type: 'task' } }
  };
}

describe('playState', () => {
  it('is on for an enabled play with no suspension', () => {
    expect(playState({ enabled: true })).toBe('on');
  });

  it('is off when the switch is off, suspended or not', () => {
    expect(playState({ enabled: false })).toBe('off');
    expect(playState({ enabled: false, suspendedAt: '2026-10-01T09:00:00Z' })).toBe('off');
  });

  it('is suspended for an enabled play the engine stopped', () => {
    expect(playState({ enabled: true, suspendedAt: '2026-10-01T09:00:00Z' })).toBe('suspended');
  });
});

describe('describeTrigger', () => {
  const cases: Array<[string, Trigger, string]> = [
    [
      'node_created',
      { type: 'graph_event', on: 'node_created', select: { target_type: 'task' } },
      'When a task is created'
    ],
    [
      'property_changed with a property key',
      {
        type: 'graph_event',
        on: 'property_changed',
        select: { target_type: 'task' },
        property_key: 'task.status'
      },
      'When status changes on a task'
    ],
    [
      'property_changed with no property key',
      { type: 'graph_event', on: 'property_changed', select: { target_type: 'invoice' } },
      'When any property changes on an invoice'
    ],
    [
      'relationship_added',
      { type: 'graph_event', on: 'relationship_added', select: { target_type: 'project' } },
      'When a relationship is added from a project'
    ],
    [
      'relationship_removed',
      { type: 'graph_event', on: 'relationship_removed', select: { target_type: '*' } },
      'When a relationship is removed from a node'
    ],
    [
      'scheduled',
      { type: 'scheduled', cron: '0 9 * * 1', select: { target_type: 'task' } },
      'On the schedule 0 9 * * 1, for every task'
    ],
    [
      'an inline selector with filters',
      {
        type: 'graph_event',
        on: 'node_created',
        select: {
          target_type: 'task',
          filters: [{ type: 'property', operator: 'equals', property: 'status', value: 'open' }]
        }
      },
      'When a task matching 1 filter is created'
    ],
    [
      'a saved-query selector',
      { type: 'scheduled', cron: '@daily', select: { query_id: 'q-1' } },
      'On the schedule @daily, for every node the saved query selects'
    ]
  ];

  it.each(cases)('describes %s', (_name, trigger, expected) => {
    expect(describeTrigger(trigger)).toBe(expected);
  });
});

describe('actions', () => {
  const action: Action = {
    action_type: 'update_node',
    description: 'Mark each sub-task done',
    params: { node_id: '{item.id}', properties: { status: 'done' } },
    for_each: 'trigger.node.has_child'
  };

  it('names the collection a for_each action runs over', () => {
    expect(actionForEach(action)).toBe('trigger.node.has_child');
    expect(
      actionForEach({ action_type: 'reject', description: 'Refuse', params: { message: 'no' } })
    ).toBeUndefined();
  });

  it('shows the authored content without the description', () => {
    expect(JSON.parse(rawAction(action))).toEqual({
      action_type: 'update_node',
      params: { node_id: '{item.id}', properties: { status: 'done' } },
      for_each: 'trigger.node.has_child'
    });
  });
});

describe('rulesWarnedOnDisable', () => {
  const rules = [rule('guard', 'invariant'), rule('follow-up', 'reactive')];

  it('names the invariant rules of a seeded play', () => {
    expect(rulesWarnedOnDisable({ isSeeded: true, rules }).map((r) => r.name)).toEqual(['guard']);
  });

  it('is empty for a play that is not seeded, or carries no invariant rule', () => {
    expect(rulesWarnedOnDisable({ isSeeded: false, rules })).toEqual([]);
    expect(rulesWarnedOnDisable({ isSeeded: true, rules: [rule('r', 'reactive')] })).toEqual([]);
  });
});
