// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { GraphEventType } from './graph-event-type';
import type { Selector } from './selector';

/**
 * What makes a rule run. Tagged on `type`; a field that does not belong to
 * the chosen variant is a decode error.
 */
export type Trigger =
  | {
      type: 'graph_event';
      on: GraphEventType;
      select: Selector;
      /**
       * `property_changed` only: the one property to watch, namespaced as
       * `<type>.<field>`. Omitted, any property change fires the rule.
       */
      property_key?: string;
    }
  | { type: 'scheduled'; cron: string; select: Selector };
