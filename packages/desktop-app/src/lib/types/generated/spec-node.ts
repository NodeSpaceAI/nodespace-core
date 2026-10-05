// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { SpecStatus } from './spec-status';

/**
 * Wire shape for spec nodes sent to the frontend.
 *
 * A spec says what is being built and why. Its `content` is its title and
 * its criteria are its direct `checkbox` children, so neither is a field.
 */
export type SpecNode = {
  /**
   * What is being built, why, and for whom.
   */
  objective?: string;
  /**
   * What may always be done, what needs sign-off, and what must never be
   * done.
   */
  boundaries?: string;
  specStatus: SpecStatus;
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
