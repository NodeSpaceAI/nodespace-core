// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';

/**
 * The fields every node carries, whatever its type (ADR-086 §2).
 *
 * The generic [`Node`] is the envelope itself, and every typed node struct
 * embeds it, so no typed shape can omit a universal field. On a typed node,
 * `properties` holds only extension fields: each field the type's schema
 * chain declares has one home, its typed top-level field.
 *
 * `lifecycle_status` is always serialized. It is governance state (ADR-087),
 * and a reader that had to infer `active` from an absent key could not tell
 * that from a shape that never carried the field.
 */
export type NodeEnvelope = {
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
