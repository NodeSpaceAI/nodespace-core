// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { LinkValue } from './link-value';
import type { NodeReference } from './node-reference';
import type { Priority } from './priority';
import type { ProjectStatus } from './project-status';

/**
 * Wire shape for software-project nodes sent to the frontend: a project's
 * fields and the two that say where its source is (ADR-098).
 *
 * Produced by `node_to_typed_value` for a node whose type is exactly
 * `software-project`. Fields map directly to the TypeScript
 * `SoftwareProjectNode` interface. The status is a project's: a software
 * project adds no status of its own.
 */
export type SoftwareProjectNode = {
  status: ProjectStatus;
  priority?: Priority;
  startDate?: string;
  endDate?: string;
  /**
   * The project's source repository. A client binds a checkout to the
   * project by comparing the checkout's remote with it.
   */
  repository?: LinkValue;
  /**
   * The folder on this machine that holds the project's checkout. It is
   * machine-bound: absent on a machine that has not set its own.
   */
  checkoutPath?: string;
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
