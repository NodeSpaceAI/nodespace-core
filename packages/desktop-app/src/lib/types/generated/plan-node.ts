// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { PlanStatus } from './plan-status';

/**
 * Wire shape for plan nodes sent to the frontend.
 *
 * A plan says how one spec will be built. Its `content` is its title; the
 * spec it implements and the tasks that carry it out are relationships.
 */
export type PlanNode = {
  /**
   * Components, dependencies and sequencing.
   */
  approach?: string;
  /**
   * What could go wrong with this approach.
   */
  risks?: string;
  planStatus: PlanStatus;
  id: string;
  nodeType: string;
  content: string;
  version: number;
  createdAt: string;
  modifiedAt: string;
  properties: Record<string, unknown>;
  mentions?: Array<string>;
  mentionedIn?: Array<NodeReference>;
  title?: string | null;
  lifecycleStatus: string;
};
