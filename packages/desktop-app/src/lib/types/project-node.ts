/**
 * Type-Safe Project Node Interface
 *
 * Flat structure matching the Rust `ProjectNode` wire shape
 * (`packages/nodespace-types/src/project.rs`): the project schema's core
 * fields travel at the top level. `properties` carries only extension fields
 * (`custom:…`), never `status`/`priority`/`start_date`/`end_date`.
 */

import type { Node } from './node';

/** Project status — the schema's core values plus any user-added value. */
export type ProjectStatus = 'planning' | 'active' | 'completed' | 'archived' | 'cancelled' | string;

export interface ProjectNode {
  id: string;
  nodeType: 'project';
  content: string;
  title?: string | null;
  version: number;
  createdAt: string;
  modifiedAt: string;
  /** Extension fields only — core fields are the typed fields below. */
  properties?: Record<string, unknown>;

  status: ProjectStatus;
  priority?: string;
  startDate?: string;
  endDate?: string;
}

/**
 * Partial update for a project's core fields. Mirrors the Rust
 * `ProjectNodeUpdate`: absent = no change, `null` = clear, a value = set.
 * `status` cannot be cleared (the schema requires it).
 */
export interface ProjectNodeUpdate {
  status?: ProjectStatus;
  priority?: string | null;
  startDate?: string | null;
  endDate?: string | null;
}

export function isProjectNode(node: Node | ProjectNode): node is ProjectNode {
  return node.nodeType === 'project';
}

/**
 * Convert a node received over any transport to a `ProjectNode`. The backend
 * (`node_to_typed_value`) already promotes the core fields to the top level
 * for every transport, so this only narrows the type and fills the status
 * default.
 */
export function nodeToProjectNode(node: Node): ProjectNode {
  const project = node as unknown as ProjectNode;
  return {
    ...project,
    nodeType: 'project',
    properties: node.properties ?? {},
    status: project.status ?? 'planning'
  };
}
