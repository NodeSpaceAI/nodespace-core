/**
 * Relationship viewer — pure grouping/normalization.
 *
 * Turns the `get_node_relationships` command payload (see
 * `rel_ops::NodeRelationshipsOutput` on the Rust side) into the view model the
 * modal renders. Kept free of any Tauri/DOM imports so it is unit-testable in
 * isolation (see `src/tests/unit/relationship-grouping.test.ts`).
 *
 * Responsibilities:
 * - Keep outbound and inbound as SEPARATE groups (never flatten different
 *   directions / target types together).
 * - Derive each group's display label — outbound uses the relationship name,
 *   inbound uses the schema-declared `reverseName`.
 * - Classify each group as a `table` (carries edge attributes) or `chips`
 *   (bare edge with no edge data) layout.
 * - Compute the ordered edge-attribute column set for the table layout.
 * - Decide where each group surfaces: a field on the property form, a populated
 *   modal section, an entry in the modal's Add chooser, or nothing at all
 *   (`partitionGroups`), and whether its edges can be edited from this node
 *   (`groupSupportsEdgeEditing`).
 */

import type { EnumValue } from '$lib/types/schema-node';

export type RelationshipDirection = 'out' | 'in';
export type RelationshipCardinality = 'one' | 'many';

/** An edge field definition as declared on the schema relationship. */
export interface RawEdgeField {
  name: string;
  type: string;
  /**
   * The closed value set of an `enum` edge field. Present only when
   * `type === 'enum'`; the backend rejects a declaration that pairs one
   * without the other. Edge enums have no user-extensible half, so there is
   * no `userValues` counterpart here.
   */
  coreValues?: EnumValue[];
  indexed?: boolean;
  required?: boolean;
  default?: unknown;
  targetType?: string;
  description?: string;
}

/** A related node plus the connecting edge's stored properties. */
export interface RawRelatedNode {
  id: string;
  nodeType: string;
  title: string | null;
  contentPreview: string;
  edgeProperties: Record<string, unknown>;
}

/** One relationship group as returned by the command (per name + direction). */
export interface RawRelationshipGroup {
  relationshipName: string;
  direction: RelationshipDirection;
  targetType: string | null;
  reverseName: string;
  sourceType: string;
  cardinality: RelationshipCardinality;
  required: boolean | null;
  edgeFields: RawEdgeField[] | null;
  description: string | null;
  related: RawRelatedNode[];
  count: number;
}

/** The full command payload. */
export interface RawNodeRelationships {
  nodeId: string;
  nodeType: string;
  groups: RawRelationshipGroup[];
}

/** A single row (target + edge attribute values) in a group. */
export interface RelationshipRowView {
  id: string;
  nodeType: string;
  /** Display label: current title, else content preview, else the id. */
  label: string;
  /** Edge attribute values keyed by field name. */
  edgeValues: Record<string, unknown>;
}

/** A relationship group ready to render. */
export interface RelationshipGroupView {
  /** Stable, unique key across name + direction + target type. */
  key: string;
  /**
   * The raw schema relationship name (e.g. `assigned_to`). Needed to drive the
   * create/delete/update mutation calls — distinct from the humanized `label`.
   */
  relationshipName: string;
  /** Human-readable heading for the group. */
  label: string;
  direction: RelationshipDirection;
  targetType: string | null;
  cardinality: RelationshipCardinality | null;
  /** Whether the relationship requires at least one edge (blocks last-edge removal). */
  required: boolean;
  description: string | null;
  /**
   * Ordered edge-property names: the union of declared `edgeFields` and any keys
   * found on stored edges. Drives which rows an expanded edge shows.
   */
  edgeColumns: string[];
  /** Declared edge field definitions, used to render edge-attribute editors. */
  edgeFields: RawEdgeField[];
  rows: RelationshipRowView[];
  count: number;
}

/** The view model consumed by the modal. */
export interface NodeRelationshipsView {
  nodeId: string;
  nodeType: string;
  groups: RelationshipGroupView[];
  /** True when the node has no typed relationships to show. */
  isEmpty: boolean;
}

/**
 * Turn a snake/kebab identifier into a Title Case label
 * (`assigned_to` → `Assigned To`).
 */
export function humanizeName(name: string): string {
  return name
    .replace(/[_-]+/g, ' ')
    .trim()
    .split(/\s+/)
    .filter((word) => word.length > 0)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}

/**
 * Display label for a group.
 * - Outbound: the humanized relationship name.
 * - Inbound: the humanized `reverseName`.
 *
 * There is no synthesized `"{SourceType} ({Relationship Name})"` fallback: a
 * schema relationship must declare `reverseName`, so every inbound group
 * carries a name its author chose. The fallback was what produced
 * "Invoice (Customer)" where the modeled answer was "Invoices", and keeping it
 * as a defensive branch would only hide a schema that failed validation.
 */
export function groupDisplayLabel(group: RawRelationshipGroup): string {
  return humanizeName(
    group.direction === 'out' ? group.relationshipName : group.reverseName
  );
}

/**
 * Ordered edge-property names for a group: declared `edgeFields` first (in
 * declared order), then any additional keys present on stored edge properties
 * that were never declared, so ad-hoc edge data still surfaces.
 *
 * Naturally empty for a group with neither — which is most of them, since most
 * relationships declare no edge fields at all.
 */
export function groupEdgeColumns(group: RawRelationshipGroup): string[] {
  const columns: string[] = [];
  const seen = new Set<string>();
  for (const field of group.edgeFields ?? []) {
    if (!seen.has(field.name)) {
      seen.add(field.name);
      columns.push(field.name);
    }
  }
  for (const node of group.related) {
    for (const key of Object.keys(node.edgeProperties ?? {})) {
      if (!seen.has(key)) {
        seen.add(key);
        columns.push(key);
      }
    }
  }
  return columns;
}

function rowLabel(node: RawRelatedNode): string {
  if (node.title && node.title.trim().length > 0) {
    return node.title;
  }
  if (node.contentPreview && node.contentPreview.trim().length > 0) {
    return node.contentPreview;
  }
  return node.id;
}

function buildGroupView(group: RawRelationshipGroup): RelationshipGroupView {
  const edgeColumns = groupEdgeColumns(group);
  const rows: RelationshipRowView[] = group.related.map((node) => ({
    id: node.id,
    nodeType: node.nodeType,
    label: rowLabel(node),
    edgeValues: node.edgeProperties ?? {}
  }));
  return {
    key: `${group.direction}:${group.relationshipName}:${group.targetType ?? '*'}`,
    relationshipName: group.relationshipName,
    label: groupDisplayLabel(group),
    direction: group.direction,
    targetType: group.targetType,
    cardinality: group.cardinality,
    required: group.required ?? false,
    description: group.description,
    edgeColumns,
    edgeFields: group.edgeFields ?? [],
    rows,
    count: group.count
  };
}

/** The stored source/target endpoints of a typed relationship edge. */
export interface EdgeEndpoints {
  sourceId: string;
  targetId: string;
}

/**
 * Resolve the stored `source`/`target` of a typed relationship edge, given the
 * node the modal is centered on (`nodeId`), the group's `direction`, and the
 * related node on the other end (`otherId`).
 *
 * Orientation is the crux of correct mutations: an edge is always stored
 * source→target, but a group shown on `nodeId` can face either way.
 * - `direction: 'out'` — the edge points FROM this node: source=nodeId, target=otherId.
 * - `direction: 'in'`  — the edge points TO this node:   source=otherId, target=nodeId.
 *
 * Pure and Tauri-free so it can be unit-tested in isolation.
 */
export function resolveEdgeEndpoints(
  nodeId: string,
  direction: RelationshipDirection,
  otherId: string
): EdgeEndpoints {
  return direction === 'out'
    ? { sourceId: nodeId, targetId: otherId }
    : { sourceId: otherId, targetId: nodeId };
}

/**
 * A relationship group's edge properties can only be edited when the schema
 * declares fields to edit. Inbound groups are never editable: the edge is owned
 * by the other node's schema.
 *
 * A group carrying only AD-HOC edge keys — present on stored edges but never
 * declared in `edgeFields` — is deliberately not editable either. The values
 * remain VISIBLE: `groupEdgeColumns` includes those keys, and the panel expands
 * a row over that union, so an undeclared key still renders read-only. An
 * undeclared key has no `type`, so an editor could only ever guess a free-text
 * input, and the update path replaces edge properties wholesale — which would
 * coerce a stored number or boolean into a string on the first save. Showing the
 * value and declining to edit it is the honest option; declaring the field on
 * the schema is how it becomes editable.
 */
export function groupSupportsEdgeEditing(group: RelationshipGroupView): boolean {
  return group.direction === 'out' && group.edgeFields.length > 0;
}

/**
 * The ids of nodes already linked into a group — the single source of truth
 * for "already linked" checks in the target picker, so a target search never
 * offers a node the group already has an edge to.
 *
 * Lowercased so the comparison is case-insensitive: callers may compare
 * against an id from a source that doesn't guarantee the same casing as the
 * backend's canonical (lowercase) form.
 */
export function linkedTargetIds(group: RelationshipGroupView): Set<string> {
  return new Set(group.rows.map((row) => row.id.toLowerCase()));
}

/** Whether a node id is already linked into this group, case-insensitively. */
export function isTargetLinked(group: RelationshipGroupView, id: string): boolean {
  return linkedTargetIds(group).has(id.toLowerCase());
}

/** Drop nodes already linked into the group from a set of candidate targets. */
export function filterUnlinkedTargets<T extends { id: string }>(
  group: RelationshipGroupView,
  nodes: T[]
): T[] {
  const existing = linkedTargetIds(group);
  return nodes.filter((node) => !existing.has(node.id.toLowerCase()));
}

/**
 * Whether a group is edited as a field on the property form rather than in the
 * Relationships modal.
 *
 * A relationship whose LOCAL end is `one` reads as a property of the node —
 * "this task's assignee" — not as a graph edge, so it is promoted to a
 * single-select field next to the node's scalar properties. `cardinality` is
 * already the local end's (the backend reports `reverse_cardinality` for an
 * inbound group), so direction plays no part here; it only decides which way
 * round the edge is written (`resolveEdgeEndpoints`).
 *
 * A `one` group that declares edge fields stays in the modal: a form field
 * holds only the related node, so promoting it would strand the edge's own
 * values with no editor.
 */
export function isFormPromoted(group: RelationshipGroupView): boolean {
  return group.cardinality === 'one' && group.edgeFields.length === 0;
}

/** The groups a node's relationship surfaces render, partitioned by what each needs. */
export interface PartitionedGroups {
  /**
   * Single-valued groups edited as fields on the property form (see
   * `isFormPromoted`), whether or not they have an edge yet. Never part of
   * `populated` or `addable`: each edge is edited in exactly one place.
   */
  promoted: RelationshipGroupView[];
  /** Modal groups with at least one edge — rendered as full sections, either direction. */
  populated: RelationshipGroupView[];
  /**
   * Outbound modal groups with no edges yet. They get no section of their own;
   * they are the entries of the single "Add relationship" chooser.
   */
  addable: RelationshipGroupView[];
}

/**
 * Split groups between the property form and the Relationships modal, and
 * within the modal between what it renders as sections versus what it offers
 * behind the single Add control.
 *
 * The form takes every promoted group, so the two surfaces partition rather
 * than duplicate — one edge, one control, no question of which is
 * authoritative.
 *
 * The modal's size must track the node's DATA, not the schema's declared
 * relationship count — a type declaring six relationships with no edges yet is
 * six empty sections' worth of scaffolding carrying zero information. So an
 * empty group never gets a section:
 *  - empty OUTBOUND groups collapse into `addable`, keeping the first edge one
 *    interaction away;
 *  - empty INBOUND groups are dropped entirely. An inbound group is the same
 *    physical edge seen from the other end, owned by the other node's schema, so
 *    it has no Add of its own to justify standing open and empty.
 */
export function partitionGroups(groups: RelationshipGroupView[]): PartitionedGroups {
  const modal = groups.filter((group) => !isFormPromoted(group));
  return {
    promoted: groups.filter(isFormPromoted),
    populated: modal.filter((group) => group.rows.length > 0),
    addable: modal.filter((group) => group.direction === 'out' && group.rows.length === 0)
  };
}

/**
 * Whether the Relationships modal has anything to show once promoted groups
 * have moved to the form — the gate for its entry point. A node whose only
 * relationships are promoted fields, or empty inbound groups the modal drops,
 * would otherwise open a modal with nothing in it.
 */
export function hasModalContent(partitioned: PartitionedGroups): boolean {
  return partitioned.populated.length > 0 || partitioned.addable.length > 0;
}

/** A group plus one of its rows, resolved together against the current view. */
export interface ResolvedRow {
  group: RelationshipGroupView;
  row: RelationshipRowView;
}

/**
 * Look a group up in the CURRENT view by its stable key.
 *
 * Every mutation reloads the view into a wholly new object graph, so UI state
 * that outlives a mutation (an open editor, an open target picker) must hold a
 * key and resolve it each time rather than capture the group object it started
 * with — otherwise it goes on rendering, and writing from, a pre-reload
 * snapshot. Returns `null` once the group is gone, which callers treat as
 * "close the thing that was open".
 */
export function findGroupByKey(
  groups: RelationshipGroupView[],
  key: string | null
): RelationshipGroupView | null {
  if (!key) return null;
  return groups.find((group) => group.key === key) ?? null;
}

/**
 * Resolve a `(group key, row id)` pair against the current view, for UI state
 * scoped to a single edge. Returns `null` when either half has gone — the group
 * removed from the schema, or the row's edge deleted — so an editor left open
 * over a vanished edge is hidden on the next RELOAD.
 *
 * That is the whole of the guarantee: it is not a substitute for the daemon
 * rejecting a write against a deleted edge. Nothing here re-checks the view at
 * the moment of a save, so a save racing an out-of-band delete still reaches the
 * daemon and still needs its error surfaced — do not read this as making that
 * path unreachable.
 */
export function findRowByKey(
  groups: RelationshipGroupView[],
  groupKey: string | null,
  rowId: string | null
): ResolvedRow | null {
  const group = findGroupByKey(groups, groupKey);
  if (!group || !rowId) return null;
  const row = group.rows.find((candidate) => candidate.id === rowId);
  return row ? { group, row } : null;
}

/**
 * Build the modal's view model from the command payload. Every group returned by
 * the command is retained — including declared groups with no related nodes yet —
 * and `partitionGroups` decides how each is surfaced (a form field, a populated
 * modal section, or an entry in the Add chooser). `isEmpty` is true when no group has any rows, i.e.
 * there is genuinely nothing populated to show.
 */
export function buildRelationshipsView(raw: RawNodeRelationships): NodeRelationshipsView {
  const groups = raw.groups.map(buildGroupView);
  return {
    nodeId: raw.nodeId,
    nodeType: raw.nodeType,
    groups,
    isEmpty: groups.every((group) => group.rows.length === 0)
  };
}
