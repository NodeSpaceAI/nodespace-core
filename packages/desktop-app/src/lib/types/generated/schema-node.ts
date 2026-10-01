// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { NodeReference } from './node-reference';
import type { SchemaField } from './schema-field';
import type { SchemaRelationship } from './schema-relationship';

export type SchemaNode = {
  isCore: boolean;
  /**
   * An abstract type is never instantiated: no node is created with it as
   * its `node_type` or retyped into it. It stays a valid `extends` target
   * and query scope (ADR-086 §6).
   */
  abstract?: boolean;
  /**
   * The schema id of the type this one extends (ADR-078), so a client can
   * resolve a user-defined subtype to the type whose rules it takes.
   */
  extends?: string;
  schemaVersion: number;
  description: string;
  fields: Array<SchemaField>;
  relationships: Array<SchemaRelationship>;
  titleTemplate?: string;
  propertiesHeaderSummaryTemplate?: string;
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
