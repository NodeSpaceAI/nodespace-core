<!--
  RelationshipViewerModal — browse and edit a node's typed relationships.

  ## Two panes, not stacked tables

  LEFT is a rail of the relationships this node actually HAS, one row each with
  its edge count. RIGHT is every edge of the selected relationship. Before this,
  each populated relationship rendered its own full-width table stacked
  vertically, so a node with six of them was six tables and a scrollbar: the
  panel's height tracked the node's connectivity. The rail stays the same size
  whatever that connectivity is.

  Two panes rather than three because expansion does the third pane's job in
  place: a row expands to its own edge properties, so properties stay one click
  away and a single-target relationship needs no special case — it simply shows
  its one edge.

  ## Only established relationships appear

  The rail lists what EXISTS. A relationship the schema declares but the node
  has no edge for does not appear, not even as a `0` row — placeholder rows in a
  rail are the same disease as empty sections, rotated ninety degrees. Declared
  relationships surface only when the user asks, via `+ Add`.

  ## Single-valued relationships are not here at all

  A relationship whose local end is `one` — a task's assignee or project — is
  edited as a field on the property form (`isFormPromoted`), and
  `partitionGroups` keeps it out of both the rail and `+ Add`. The two surfaces
  partition the node's relationships, so no edge has two controls.

  ## `+ Add` marks the authoring boundary

  Rail order is: relationships this node can AUTHOR, then `+ Add`, then a
  divider, then INCOMING · READ-ONLY. The control's position tells the user what
  they can create, so nothing has to carry that rule as a badge — and the absent
  controls in the detail pane are what actually hold it.

  ## Incoming relationships are the same edge from the other end

  An inbound relationship is not a second relationship. It is the SAME physical
  row in the `relationship` table, whose source is the other node — where it is
  declared. Direction decides which way round an edge is written, not whether it
  can be (`groupAcceptsEdgesHere`):

  - A BARE inbound edge carries nothing but the link, so it is the same fact
    from either end. A task's `Blocked By` is authored here like its `Blocks`
    twin: add and remove write the declared edge with the endpoints transposed.
  - An inbound edge WITH edge fields is read-only here: it shows the SAME values,
    and editing means opening the declaring node, which the target link does.
    This is the authority model, not a UI convenience: a `person` viewing
    `has_access_to → Design Docs` sees `access: Owner` and must not change it,
    because access is granted from the collection's panel.

  ## Nothing is "saved"

  Edits persist on change, as node edits do everywhere else in the app. There is
  no Save button; adding one would reintroduce a pattern the codebase has moved
  past.
-->
<script lang="ts">
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import LoaderIcon from '@lucide/svelte/icons/loader-circle';
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
  import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
  import XIcon from '@lucide/svelte/icons/x';
  import PlusIcon from '@lucide/svelte/icons/plus';
  import * as Popover from '$lib/components/ui/popover';
  import * as Select from '$lib/components/ui/select';
  import { createLogger } from '$lib/utils/logger';
  import { toError } from '$lib/types/errors';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { getEnumValues, enumValueLabel } from '$lib/utils/schema-enum-values';
  import {
    loadNodeRelationshipsView,
    addEdge,
    removeEdge,
    updateEdgeProperties,
    searchTargets
  } from '$lib/services/relationship-viewer-service';
  import {
    filterUnlinkedTargets,
    findGroupByKey,
    findRowByKey,
    groupAcceptsEdgesHere,
    groupSupportsEdgeEditing,
    partitionGroups,
    type NodeRelationshipsView,
    type RawEdgeField,
    type RelationshipGroupView,
    type RelationshipRowView
  } from '$lib/services/relationship-grouping';
  import {
    coerceNumber,
    edgeInputKind,
    edgeInputType,
    edgeInputValue,
    formatEdgeFieldLabel,
    toInputString
  } from '$lib/services/edge-field-input';
  import type { Node } from '$lib/types';

  const log = createLogger('RelationshipViewerModal');

  interface Props {
    open: boolean;
    nodeId: string;
  }

  let { open = $bindable(false), nodeId }: Props = $props();

  type Phase = 'idle' | 'loading' | 'loaded' | 'error';

  /**
   * One row of an expanded edge's properties: the property name, plus its
   * schema declaration when it has one. `field` is undefined for a key stored on
   * the edge but never declared — see `expandableFields`.
   */
  type EdgeRow = { name: string; field: RawEdgeField | undefined };

  let phase = $state<Phase>('idle');
  let view = $state<NodeRelationshipsView | null>(null);
  let errorMessage = $state<string | null>(null);
  // Tracks which node's data is currently loaded/loading, so re-renders don't
  // refetch and a stale response for a previous node is discarded.
  let loadedKey: string | null = null;

  // --- Edit state -----------------------------------------------------------
  let busy = $state(false);
  let mutationError = $state<string | null>(null);

  // Per-row edge-attribute drafts, keyed by `${group.key}::${rowId}`. Only the
  // fields the user has touched are stored; the merge on save layers them over
  // the row's stored edge values.
  let edgeDrafts = $state<Record<string, Record<string, unknown>>>({});

  // --- Rail selection -------------------------------------------------------
  // Which relationship the detail pane is showing, held as a KEY rather than the
  // group object: every mutation reloads `view` into a wholly new object graph,
  // so a captured group would leave the pane rendering a pre-reload snapshot.
  let selectedKey = $state<string | null>(null);

  // Which rows are expanded to their edge properties, as a set of
  // `${group.key}::${rowId}` keys. Independent toggles, NOT an accordion: most
  // rows never expand, expanded content is short, and the access-control case
  // means several edges are relevant at once — comparing access across people is
  // exactly what an accordion prevents. Not persisted across reopening.
  let expandedRows = $state<Record<string, boolean>>({});

  // Whether the "+ Add" relationship chooser is open.
  let addChooserOpen = $state(false);

  // Target picker (one group open at a time), keyed for the same reason as
  // `selectedKey`.
  let addGroupKey = $state<string | null>(null);
  let addQuery = $state('');
  let addResults = $state<Node[]>([]);
  let addSearching = $state(false);
  // True once a search returned at least one raw match but every one of them
  // was already linked, so the empty-results message can say that rather than
  // the ambiguous "No matches." — reserved for a search that genuinely found
  // nothing.
  let addAllLinked = $state(false);
  // A picked target awaiting edge-attribute entry (only for groups with declared
  // edge fields); null means the picker is still in search mode.
  let addStaged = $state<{ id: string; label: string } | null>(null);
  let addEdgeDraft = $state<Record<string, unknown>>({});
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  // The rail lists only relationships that actually have edges; `addable` is the
  // set of empty relationships this node can author, which surface solely as
  // `+ Add` entries.
  const partitioned = $derived(partitionGroups(view?.groups ?? []));
  const populatedGroups = $derived(partitioned.populated);
  const addableGroups = $derived(partitioned.addable);

  // Rail order encodes the authoring boundary: relationships editable here
  // (outbound, and bare inbound ones), then the `+ Add` control, then read-only
  // incoming ones below the divider.
  const editableGroups = $derived(populatedGroups.filter(groupAcceptsEdgesHere));
  const readOnlyGroups = $derived(
    populatedGroups.filter((group) => !groupAcceptsEdgesHere(group))
  );

  // Resolves against the CURRENT view every render, so the detail pane always
  // reflects the latest reload rather than a pre-mutation snapshot. Whether the
  // selection is still one the RAIL offers is a separate question, handled by the
  // effect below — a group can resolve here while having no rows left.
  const selectedGroup = $derived(findGroupByKey(view?.groups ?? [], selectedKey));
  const addGroup = $derived(findGroupByKey(view?.groups ?? [], addGroupKey));

  /**
   * Keep a relationship selected whenever the rail has one to select.
   *
   * Two cases land on the same fallback: nothing is selected yet (the panel just
   * loaded), or the selection has left the RAIL — its last edge was removed, or a
   * schema change dropped the relationship. An empty right pane beside a
   * populated rail reads as a bug, so fall back to the first rail entry.
   *
   * The check is against `populatedGroups`, NOT against whether the key still
   * resolves in `view.groups`. Removing a relationship's last edge does not
   * remove the group — it stays as a declared-but-empty group, which is exactly
   * what `+ Add` offers — so the key goes on resolving while the rail entry it
   * pointed at is gone, leaving the pane showing a header with no rows.
   *
   * Terminates: it only assigns when the current selection is absent from the
   * rail, and the value assigned is taken from the rail, which is present in it.
   */
  $effect(() => {
    if (populatedGroups.length === 0) {
      if (selectedKey !== null) selectedKey = null;
      return;
    }
    const stillInRail = populatedGroups.some((group) => group.key === selectedKey);
    if (!stillInRail) selectedKey = populatedGroups[0].key;
  });

  /**
   * Drop expansion state for rows that no longer resolve against the current
   * view, so a reload closes what its edge no longer justifies keeping open.
   *
   * This has to be an active prune rather than only resolving on read, because
   * neither half of the key is unique enough to leave lying around: a group key
   * is `direction:name:targetType` and a row id is the TARGET NODE's id, not the
   * edge's. So removing an edge and re-adding the same target to the same
   * relationship rebuilds a row that matches the dangling key — and the row would
   * spring open already expanded, which `findRowByKey` alone cannot prevent
   * because by then the key resolves again.
   *
   * Terminates: it writes only when a stale key exists, and the write removes
   * exactly those keys.
   */
  $effect(() => {
    const groups = view?.groups ?? [];
    const stale = Object.keys(expandedRows).filter((key) => {
      const [groupKey, rowId] = splitRowKey(key);
      return findRowByKey(groups, groupKey, rowId) === null;
    });
    if (stale.length === 0) return;
    const next = { ...expandedRows };
    for (const key of stale) delete next[key];
    expandedRows = next;
  });

  // A group key outliving its group would reopen the picker if a schema change
  // brought that relationship back.
  $effect(() => {
    if (addGroupKey && !addGroup) closeAdd();
  });

  $effect(() => {
    if (!open) {
      loadedKey = null;
      resetTransient();
      return;
    }
    if (!nodeId || loadedKey === nodeId) return;
    loadedKey = nodeId;
    void load(nodeId);
  });

  function resetTransient() {
    edgeDrafts = {};
    selectedKey = null;
    expandedRows = {};
    addChooserOpen = false;
    addGroupKey = null;
    addQuery = '';
    addResults = [];
    addSearching = false;
    addStaged = null;
    addEdgeDraft = {};
    mutationError = null;
  }

  async function load(id: string) {
    phase = 'loading';
    view = null;
    errorMessage = null;
    resetTransient();
    try {
      const result = await loadNodeRelationshipsView(id);
      // Discard a stale response if the modal was reopened for another node.
      if (loadedKey !== id) return;
      view = result;
      phase = 'loaded';
    } catch (error) {
      if (loadedKey !== id) return;
      log.error('Failed to load relationships', error);
      errorMessage = toError(error).message;
      phase = 'error';
    }
  }

  /** Re-fetch after a mutation WITHOUT blanking the view or losing selection. */
  async function reloadAfterMutation() {
    try {
      const result = await loadNodeRelationshipsView(nodeId);
      if (loadedKey !== nodeId) return;
      view = result;
    } catch (error) {
      log.error('Failed to reload relationships', error);
      mutationError = toError(error).message;
    }
  }

  /**
   * Run a relationship mutation, then reload so both directions reflect the
   * change. Returns `true` on success. Transient cleanup is left to the caller so
   * a completed action only clears its OWN state (e.g. the saved row's draft, or
   * the open picker) and never wipes an unrelated row's unsaved edit. On failure
   * nothing is cleared so the user can retry, and the error is surfaced.
   */
  async function runMutation(fn: () => Promise<void>): Promise<boolean> {
    busy = true;
    mutationError = null;
    try {
      await fn();
      await reloadAfterMutation();
      return true;
    } catch (error) {
      log.error('Relationship mutation failed', error);
      mutationError = toError(error).message;
      return false;
    } finally {
      busy = false;
    }
  }

  // --- Rail -----------------------------------------------------------------

  function selectGroup(group: RelationshipGroupView) {
    selectedKey = group.key;
    mutationError = null;
  }

  // --- Row expansion --------------------------------------------------------

  function isExpanded(group: RelationshipGroupView, row: RelationshipRowView): boolean {
    return expandedRows[rowKey(group, row)] === true;
  }

  function toggleExpanded(group: RelationshipGroupView, row: RelationshipRowView) {
    const key = rowKey(group, row);
    expandedRows = { ...expandedRows, [key]: !expandedRows[key] };
  }

  // --- Edge-attribute editing ----------------------------------------------

  /** The `edgeDrafts` key for one row, from the same parts an expansion holds. */
  function draftKey(groupKey: string, rowId: string): string {
    return `${groupKey}::${rowId}`;
  }

  function rowKey(group: RelationshipGroupView, row: RelationshipRowView): string {
    return draftKey(group.key, row.id);
  }

  /**
   * Split a composite key back into its parts, for resolving a stored key
   * against the current view. A group key can itself contain `:` (it is
   * `direction:name:targetType`), so the separator is the `::` that `draftKey`
   * joins with, and only its LAST occurrence is a separator — a row id is a node
   * id and never contains one.
   */
  function splitRowKey(key: string): [string, string] {
    const at = key.lastIndexOf('::');
    return at === -1 ? [key, ''] : [key.slice(0, at), key.slice(at + 2)];
  }

  function currentEdgeValue(
    group: RelationshipGroupView,
    row: RelationshipRowView,
    fieldName: string
  ): unknown {
    const draft = edgeDrafts[rowKey(group, row)];
    if (draft && fieldName in draft) return draft[fieldName];
    return row.edgeValues[fieldName];
  }

  /**
   * Record an edge-field change and persist it immediately.
   *
   * Edits save as they are made, like node edits everywhere else in the app —
   * there is no Save button. The draft is still written first so the input stays
   * controlled by what the user typed while the write is in flight, and so a
   * failed write leaves their value on screen to retry rather than reverting it.
   */
  function setEdgeDraft(
    group: RelationshipGroupView,
    row: RelationshipRowView,
    fieldName: string,
    value: unknown
  ) {
    const key = rowKey(group, row);
    edgeDrafts = { ...edgeDrafts, [key]: { ...(edgeDrafts[key] ?? {}), [fieldName]: value } };
  }

  /** Drop a single row's draft without disturbing any other row's unsaved edits. */
  function clearRowDraft(group: RelationshipGroupView, row: RelationshipRowView) {
    clearDraftKey(rowKey(group, row));
  }

  /**
   * Drop a draft by its composite key, for the case where the row object is no
   * longer reachable — its edge has gone from the view — but its draft is still
   * held under the key that row had.
   */
  function clearDraftKey(key: string) {
    if (!(key in edgeDrafts)) return;
    const next = { ...edgeDrafts };
    delete next[key];
    edgeDrafts = next;
  }

  /**
   * Persist one edge's properties. Called on change/blur rather than from a Save
   * button — see `setEdgeDraft`.
   *
   * `update` replaces edge properties wholesale, so the full merged bag goes over
   * the wire, not just the touched field.
   */
  async function commitEdge(group: RelationshipGroupView, row: RelationshipRowView) {
    const draft = edgeDrafts[rowKey(group, row)];
    if (!draft || Object.keys(draft).length === 0) return;
    const properties = { ...row.edgeValues, ...draft };
    const ok = await runMutation(() => updateEdgeProperties(nodeId, group, row.id, properties));
    // Only this row's draft is now stale (its values are persisted); leave any
    // other row's in-progress edit untouched. On failure the draft stays so the
    // surfaced error can be acted on without retyping.
    if (ok) clearRowDraft(group, row);
  }

  // --- Removal --------------------------------------------------------------

  async function removeRow(group: RelationshipGroupView, row: RelationshipRowView) {
    if (group.required && group.rows.length === 1) {
      const confirmed = window.confirm(
        `Remove the last "${group.label}" relationship? This relationship is required.`
      );
      if (!confirmed) return;
    }
    const ok = await runMutation(() => removeEdge(nodeId, group, row.id));
    // Drop any draft that belonged to the now-removed row so it can't linger.
    if (ok) clearRowDraft(group, row);
  }

  // --- Navigation -----------------------------------------------------------

  /**
   * Open the node at the other end of an edge, reusing its tab if it already has
   * one rather than stacking a duplicate.
   *
   * For a read-only INBOUND row — one whose relationship declares edge fields —
   * this is also the route to editing: that node's own panel declares the
   * relationship outbound, which is the only place such an edge can be changed.
   * The modal closes so it doesn't sit over the destination.
   */
  function openTarget(row: RelationshipRowView) {
    open = false;
    getNavigationService().focusOrOpenNode(row.id, {
      nodeType: row.nodeType,
      // The row already resolved this label, so the tab can carry it straight
      // away instead of showing "Loading..." until the viewer mounts.
      title: row.label
    });
  }

  // --- Target picker --------------------------------------------------------

  /** Choose a relationship type from the `+ Add` chooser, then pick its target. */
  function chooseAddType(group: RelationshipGroupView) {
    addChooserOpen = false;
    openAdd(group);
  }

  function openAdd(group: RelationshipGroupView) {
    addGroupKey = group.key;
    addQuery = '';
    addResults = [];
    addSearching = false;
    addAllLinked = false;
    addStaged = null;
    addEdgeDraft = {};
    mutationError = null;
  }

  function closeAdd() {
    addGroupKey = null;
    addQuery = '';
    addResults = [];
    addSearching = false;
    addAllLinked = false;
    addStaged = null;
    addEdgeDraft = {};
  }

  function onAddQueryInput(value: string) {
    addQuery = value;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void runSearch(), 200);
  }

  async function runSearch() {
    const group = addGroup;
    if (!group) return;
    const requestedKey = group.key;
    const q = addQuery.trim();
    if (!q) {
      addResults = [];
      addAllLinked = false;
      return;
    }
    addSearching = true;
    try {
      const results = await searchTargets(group.targetType, q);
      // Discard a response for a picker that has since moved on.
      if (addGroupKey !== requestedKey) return;
      // Re-read the group AFTER the await: a mutation during the search would
      // have reloaded the view, and filtering on the pre-search rows could
      // offer a target that is already linked.
      const current = addGroup;
      if (!current) return;
      const filtered = filterUnlinkedTargets(current, results);
      addResults = filtered;
      // Distinguishes "nothing left to add" from "no matches": only true when
      // the search itself found something, but every match was already
      // linked — as opposed to a search that genuinely found nothing.
      addAllLinked = results.length > 0 && filtered.length === 0;
    } catch (error) {
      log.error('Target search failed', error);
      // Only blank the results if this response still belongs to the open
      // picker — a superseded request must not clear what replaced it.
      if (addGroupKey === requestedKey) {
        addResults = [];
        addAllLinked = false;
      }
    } finally {
      if (addGroupKey === requestedKey) addSearching = false;
    }
  }

  function nodeLabel(node: Node): string {
    return node.title?.trim() || node.content?.trim() || node.id;
  }

  /** Pick a target: create immediately when the group has no edge fields, else
   *  stage it and prompt for the declared edge-attribute values. */
  function pickTarget(group: RelationshipGroupView, targetId: string, label: string) {
    if (group.edgeFields.length === 0) {
      void confirmAdd(group, targetId, {});
      return;
    }
    addStaged = { id: targetId, label };
    addEdgeDraft = {};
  }

  async function confirmAdd(
    group: RelationshipGroupView,
    targetId: string,
    edgeData: Record<string, unknown>
  ) {
    const hasEdgeData = Object.keys(edgeData).length > 0;
    const ok = await runMutation(() =>
      addEdge(nodeId, group, targetId, hasEdgeData ? edgeData : undefined)
    );
    // Select the relationship the edge just landed in, so the new row is visible
    // rather than hidden behind whatever was selected before.
    if (ok) {
      selectedKey = group.key;
      closeAdd();
    }
  }

  // --- Read-only formatting -------------------------------------------------

  function formatValue(value: unknown): string {
    if (value === null || value === undefined || value === '') return '—';
    if (typeof value === 'string') return value;
    if (typeof value === 'number' || typeof value === 'boolean') return String(value);
    return JSON.stringify(value);
  }

  /**
   * Read-only rendering of an edge value. An enum shows its declared label
   * rather than the stored key, so the viewer and the picker agree on how a
   * value reads. Everything else — including an undeclared edge key, where
   * `field` is undefined — formats as before.
   */
  function formatEdgeValue(field: RawEdgeField | undefined, value: unknown): string {
    if (field && edgeInputKind(field) === 'enum' && typeof value === 'string' && value !== '') {
      return enumValueLabel(field, value) ?? value;
    }
    return formatValue(value);
  }

  /**
   * Cardinality as the panel says it: `single` or nothing.
   *
   * `cardinality` constrains only THIS end — one owner per ADR, while that person
   * may own many ADRs; the other end's constraint is `reverseCardinality`, which
   * governs the inbound view. Each group therefore arrives with one cardinality
   * already resolved for its own side, so the label never reasons about pairings
   * and never says "one-to-one" or "many-to-one".
   */
  function cardinalityLabel(group: RelationshipGroupView): string | null {
    return group.cardinality === 'one' ? 'single' : null;
  }

  /**
   * The edge-property rows a group shows when expanded, keyed by name.
   *
   * `edgeColumns` rather than `edgeFields`: it is the union of the DECLARED
   * fields and any keys actually present on stored edges. A group whose edges
   * carry only undeclared ("ad-hoc") properties has no `edgeFields` at all, and
   * keying off those alone would make such a row unexpandable — its values
   * invisible with no way to reach them.
   *
   * That union is GROUP-WIDE, not per row: every row of a group offers the same
   * property names, and a row whose own edge lacks one shows it as empty. This is
   * deliberate. Per-row columns would make the expander appear and disappear down
   * a list of otherwise identical rows, and would hide the difference between
   * "this edge has no note" and "no edge here has a note" — in a group where
   * three of four edges carry `note`, the fourth's missing one is information.
   * Declared fields already behave this way; this keeps ad-hoc keys consistent
   * with them.
   *
   * The declaration is still what governs EDITING: `field` is undefined for an
   * ad-hoc key, and `groupSupportsEdgeEditing` is false for a group with no
   * declared fields, so those values render read-only. That asymmetry is
   * deliberate — an undeclared key has no `type`, and the update path replaces
   * edge properties wholesale, so an editor could only guess a free-text input
   * and would coerce a stored number or boolean to a string on first save.
   * Showing the value and declining to edit it is the honest option.
   */
  function expandableFields(group: RelationshipGroupView): EdgeRow[] {
    const declared = new Map(group.edgeFields.map((field) => [field.name, field]));
    return group.edgeColumns.map((name) => ({ name, field: declared.get(name) }));
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-4xl">
    <Dialog.Header>
      <Dialog.Title>Relationships</Dialog.Title>
      <Dialog.Description>
        Typed relationships connecting this node to others, in both directions.
      </Dialog.Description>
    </Dialog.Header>

    {#if mutationError}
      <div
        role="alert"
        class="border-destructive/30 bg-destructive/10 text-destructive flex items-start gap-2 rounded-md border p-3 text-sm"
      >
        <CircleAlertIcon class="mt-0.5 size-4 shrink-0" />
        <span>{mutationError}</span>
      </div>
    {/if}

    {#if phase === 'loading'}
      <div class="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <LoaderIcon class="size-4 animate-spin" />
        <span>Loading relationships…</span>
      </div>
    {:else if phase === 'error'}
      <div
        role="alert"
        class="border-destructive/30 bg-destructive/10 text-destructive flex items-start gap-2 rounded-md border p-3 text-sm"
      >
        <CircleAlertIcon class="mt-0.5 size-4 shrink-0" />
        <span>{errorMessage ?? 'Failed to load relationships.'}</span>
      </div>
    {:else if phase === 'loaded' && view && populatedGroups.length === 0 && addableGroups.length === 0}
      <div class="text-muted-foreground py-6 text-center text-sm">
        This node has no typed relationships.
      </div>
    {:else if phase === 'loaded' && view}
      <!-- One pane at a time below `sm`, two side by side above it. -->
      <div class="grid gap-4 sm:grid-cols-[minmax(0,13rem)_minmax(0,1fr)]">
        <!-- LEFT: the relationships this node actually has. -->
        <nav
          class="sm:border-border max-h-[55vh] overflow-y-auto sm:border-r sm:pr-3"
          aria-label="Relationships"
        >
          {#if editableGroups.length > 0}
            <div class="text-muted-foreground px-1 pb-1 text-xs font-medium uppercase">
              On this node
            </div>
            <ul class="grid gap-0.5">
              {#each editableGroups as group (group.key)}
                <li>
                  <button
                    type="button"
                    class="rail-item {group.key === selectedKey ? 'rail-item--active' : ''}"
                    aria-current={group.key === selectedKey ? 'true' : undefined}
                    onclick={() => selectGroup(group)}
                  >
                    <span class="min-w-0 flex-1 truncate">{group.label}</span>
                    <span class="text-muted-foreground shrink-0 text-xs">{group.count}</span>
                  </button>
                </li>
              {/each}
            </ul>
          {/if}

          <!-- `+ Add` sits between owned and incoming: its position is what tells
               the user which relationships they can create from this node. -->
          {#if addableGroups.length > 0}
            <Popover.Root bind:open={addChooserOpen}>
              <Popover.Trigger
                class="rail-item text-muted-foreground focus-visible:bg-accent focus-visible:text-accent-foreground mt-1 focus-visible:outline-none"
              >
                <PlusIcon class="size-3.5 shrink-0" />
                <span>Add</span>
              </Popover.Trigger>
              <Popover.Content class="w-64 p-1" align="start">
                <div class="grid gap-0.5">
                  {#each addableGroups as group (group.key)}
                    <button
                      type="button"
                      class="rail-item"
                      onclick={() => chooseAddType(group)}
                    >
                      <span class="min-w-0 flex-1 truncate">{group.label}</span>
                      {#if group.targetType}
                        <span class="text-muted-foreground shrink-0 text-xs"
                          >{group.targetType}</span
                        >
                      {/if}
                    </button>
                  {/each}
                </div>
              </Popover.Content>
            </Popover.Root>
          {/if}

          {#if readOnlyGroups.length > 0}
            <div class="border-border mt-2 border-t border-dashed pt-2">
              <div class="text-muted-foreground px-1 pb-1 text-xs font-medium uppercase">
                Incoming · read-only
              </div>
              <ul class="grid gap-0.5">
                {#each readOnlyGroups as group (group.key)}
                  <li>
                    <button
                      type="button"
                      class="rail-item {group.key === selectedKey ? 'rail-item--active' : ''}"
                      aria-current={group.key === selectedKey ? 'true' : undefined}
                      onclick={() => selectGroup(group)}
                    >
                      <span class="min-w-0 flex-1 truncate">{group.label}</span>
                      <span class="text-muted-foreground shrink-0 text-xs">{group.count}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
        </nav>

        <!-- RIGHT: every edge of the selected relationship. -->
        <div class="max-h-[55vh] min-w-0 overflow-y-auto">
          {#if selectedGroup}
            {@const group = selectedGroup}
            {@const editable = groupSupportsEdgeEditing(group)}
            {@const authorable = groupAcceptsEdgesHere(group)}
            <!-- No `read-only` badge here: the rail heading above the selection
                 already says it, and the pane offers no controls to edit with. A
                 third signal is noise. -->
            <header class="flex flex-wrap items-baseline gap-x-2 gap-y-1 pb-2">
              <span class="text-sm font-medium">{group.label}</span>
              {#if group.targetType}
                <span class="text-muted-foreground text-xs">→ {group.targetType}</span>
              {/if}
              {#if cardinalityLabel(group)}
                <span class="text-muted-foreground text-xs">· {cardinalityLabel(group)}</span>
              {/if}
              {#if group.required}
                <span class="text-muted-foreground text-xs">· required</span>
              {/if}
            </header>

            {#if group.description}
              <p class="text-muted-foreground pb-2 text-xs">{group.description}</p>
            {/if}

            <ul class="grid gap-1">
              {#each group.rows as row (row.id)}
                {@const fields = expandableFields(group)}
                {@const expandable = fields.length > 0}
                {@const rowOpen = expandable && isExpanded(group, row)}
                <li class="border-border rounded-md border">
                  <div class="flex items-center gap-1 px-2 py-1.5">
                    {#if expandable}
                      <button
                        type="button"
                        class="text-muted-foreground hover:text-foreground focus-visible:bg-accent focus-visible:text-accent-foreground inline-flex size-5 shrink-0 items-center justify-center rounded focus-visible:outline-none"
                        aria-expanded={rowOpen}
                        aria-controls={`edge-props-${rowKey(group, row)}`}
                        aria-label={rowOpen ? `Collapse ${row.label}` : `Expand ${row.label}`}
                        onclick={() => toggleExpanded(group, row)}
                      >
                        {#if rowOpen}
                          <ChevronDownIcon class="size-4" />
                        {:else}
                          <ChevronRightIcon class="size-4" />
                        {/if}
                      </button>
                    {:else}
                      <!-- Keeps labels aligned with expandable rows. Most
                           relationships declare no edge fields, so this is the
                           common case, not the exception. -->
                      <span class="size-5 shrink-0" aria-hidden="true"></span>
                    {/if}

                    <span class="min-w-0 flex-1 truncate text-sm">{row.label}</span>

                    <button
                      type="button"
                      class="text-muted-foreground hover:text-foreground focus-visible:bg-accent focus-visible:text-accent-foreground inline-flex size-6 shrink-0 items-center justify-center rounded focus-visible:outline-none"
                      title="Open {row.label}"
                      aria-label="Open {row.label}"
                      onclick={() => openTarget(row)}
                    >
                      <ExternalLinkIcon class="size-3.5" />
                    </button>

                    {#if authorable}
                      <button
                        type="button"
                        class="text-muted-foreground hover:text-destructive focus-visible:bg-accent focus-visible:text-accent-foreground inline-flex size-6 shrink-0 items-center justify-center rounded focus-visible:outline-none disabled:opacity-50"
                        title="Remove {row.label}"
                        aria-label="Remove {row.label}"
                        disabled={busy}
                        onclick={() => removeRow(group, row)}
                      >
                        <XIcon class="size-3.5" />
                      </button>
                    {/if}
                  </div>

                  {#if rowOpen}
                    <div
                      id={`edge-props-${rowKey(group, row)}`}
                      class="border-border grid gap-2 border-t px-2 py-2 pl-9"
                    >
                      {#each fields as entry (entry.name)}
                        {@const field = entry.field}
                        {@const value = currentEdgeValue(group, row, entry.name)}
                        {@const kind = field ? edgeInputKind(field) : null}
                        <div class="grid gap-1 sm:grid-cols-[minmax(0,8rem)_minmax(0,1fr)]">
                          <span
                            class="text-muted-foreground text-xs font-medium uppercase sm:pt-2"
                          >
                            {formatEdgeFieldLabel(entry.name)}
                          </span>
                          <!-- No `field` means a key stored on the edge that the
                               schema never declared: show its value, but offer no
                               editor — there is no type to render one from. -->
                          {#if !editable || !field || !kind}
                            <span class="text-sm sm:py-1.5">{formatEdgeValue(field, value)}</span>
                          {:else if kind === 'enum'}
                            <Select.Root
                              type="single"
                              disabled={busy}
                              value={typeof value === 'string' ? value : ''}
                              onValueChange={(next) => {
                                setEdgeDraft(group, row, entry.name, next);
                                void commitEdge(group, row);
                              }}
                            >
                              <Select.Trigger class="h-8 text-sm">
                                {enumValueLabel(field, typeof value === 'string' ? value : '') ??
                                  'Select…'}
                              </Select.Trigger>
                              <Select.Content>
                                {#each getEnumValues(field) as option (option.value)}
                                  <Select.Item value={option.value}>{option.label}</Select.Item>
                                {/each}
                              </Select.Content>
                            </Select.Root>
                          {:else if kind === 'boolean'}
                            <input
                              type="checkbox"
                              class="border-input size-4 rounded sm:mt-2"
                              checked={value === true}
                              disabled={busy}
                              onchange={(event) => {
                                setEdgeDraft(
                                  group,
                                  row,
                                  entry.name,
                                  event.currentTarget.checked
                                );
                                void commitEdge(group, row);
                              }}
                            />
                          {:else}
                            <Input
                              class="h-8 text-sm"
                              type={edgeInputType(kind)}
                              value={edgeInputValue(kind, value)}
                              disabled={busy}
                              oninput={(event) =>
                                setEdgeDraft(
                                  group,
                                  row,
                                  entry.name,
                                  kind === 'number'
                                    ? coerceNumber(event.currentTarget.value)
                                    : event.currentTarget.value
                                )}
                              onblur={() => void commitEdge(group, row)}
                            />
                          {/if}
                        </div>
                      {/each}
                      <!-- Two different reasons a row is read-only, and they need
                           different answers: an inbound edge with declared fields
                           is authored at its declaring end, so the fix is to open
                           that node; an undeclared key has no type to build an
                           editor from, so the fix is to declare it on the schema.
                           Saying "open the owning node" for the second case would
                           send the user somewhere that cannot help — neither end
                           can edit an undeclared key. -->
                      <p class="text-muted-foreground text-xs">
                        {#if editable}
                          Changes save as you make them.
                        {:else if !authorable}
                          Edit these from the node that owns this relationship.
                        {:else}
                          These properties aren't declared on the schema, so they're shown as
                          stored. Declare them as edge fields to make them editable.
                        {/if}
                      </p>
                    </div>
                  {/if}
                </li>
              {/each}
            </ul>
          {:else}
            <div class="text-muted-foreground py-6 text-center text-sm">
              Select a relationship to see its details.
            </div>
          {/if}
        </div>
      </div>

      <!-- Target picker for the relationship chosen from `+ Add`. -->
      {#if addGroup}
        {@const group = addGroup}
        <div class="border-border grid gap-2 rounded-md border p-3">
          <div class="flex items-center gap-2">
            <span class="text-sm font-medium">Add {group.label}</span>
            {#if group.targetType}
              <span class="text-muted-foreground text-xs">→ {group.targetType}</span>
            {/if}
            <button
              type="button"
              class="text-muted-foreground hover:text-foreground focus-visible:bg-accent focus-visible:text-accent-foreground ml-auto inline-flex size-6 items-center justify-center rounded focus-visible:outline-none"
              aria-label="Cancel add"
              onclick={closeAdd}
            >
              <XIcon class="size-3.5" />
            </button>
          </div>

          {#if addStaged}
            {@const staged = addStaged}
            <div class="grid gap-2">
              <span class="text-sm">{staged.label}</span>
              {#each group.edgeFields as field (field.name)}
                {@const value = addEdgeDraft[field.name]}
                {@const kind = edgeInputKind(field)}
                <div class="grid gap-1 sm:grid-cols-[minmax(0,8rem)_minmax(0,1fr)]">
                  <span class="text-muted-foreground text-xs font-medium uppercase sm:pt-2">
                    {formatEdgeFieldLabel(field.name)}
                  </span>
                  {#if kind === 'enum'}
                    <Select.Root
                      type="single"
                      value={typeof value === 'string' ? value : ''}
                      onValueChange={(next) => (addEdgeDraft = { ...addEdgeDraft, [field.name]: next })}
                    >
                      <Select.Trigger class="h-8 text-sm">
                        {enumValueLabel(field, typeof value === 'string' ? value : '') ??
                          'Select…'}
                      </Select.Trigger>
                      <Select.Content>
                        {#each getEnumValues(field) as option (option.value)}
                          <Select.Item value={option.value}>{option.label}</Select.Item>
                        {/each}
                      </Select.Content>
                    </Select.Root>
                  {:else if kind === 'boolean'}
                    <input
                      type="checkbox"
                      class="border-input size-4 rounded sm:mt-2"
                      checked={value === true}
                      onchange={(event) =>
                        (addEdgeDraft = {
                          ...addEdgeDraft,
                          [field.name]: event.currentTarget.checked
                        })}
                    />
                  {:else}
                    <Input
                      class="h-8 text-sm"
                      type={edgeInputType(kind)}
                      value={toInputString(value)}
                      oninput={(event) =>
                        (addEdgeDraft = {
                          ...addEdgeDraft,
                          [field.name]:
                            kind === 'number'
                              ? coerceNumber(event.currentTarget.value)
                              : event.currentTarget.value
                        })}
                    />
                  {/if}
                </div>
              {/each}
              <div class="flex items-center gap-2">
                <Button
                  size="sm"
                  disabled={busy}
                  onclick={() => void confirmAdd(group, staged.id, addEdgeDraft)}
                >
                  Add
                </Button>
                <Button size="sm" variant="secondary" onclick={() => (addStaged = null)}>
                  Back
                </Button>
              </div>
            </div>
          {:else}
            <Input
              class="h-8 text-sm"
              placeholder="Search {group.targetType ?? 'nodes'}…"
              value={addQuery}
              oninput={(event) => onAddQueryInput(event.currentTarget.value)}
            />
            {#if addSearching}
              <div class="text-muted-foreground flex items-center gap-2 text-xs">
                <LoaderIcon class="size-3 animate-spin" />
                <span>Searching…</span>
              </div>
            {:else if addQuery.trim() && addResults.length === 0}
              <span class="text-muted-foreground text-xs">
                {addAllLinked
                  ? 'No matches — every result is already linked.'
                  : 'No matches.'}
              </span>
            {:else if addResults.length > 0}
              <ul class="grid max-h-40 gap-0.5 overflow-y-auto">
                {#each addResults as node (node.id)}
                  <li>
                    <button
                      type="button"
                      class="rail-item truncate"
                      disabled={busy}
                      onclick={() => pickTarget(group, node.id, nodeLabel(node))}
                    >
                      {nodeLabel(node)}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </div>
      {/if}
    {/if}
  </Dialog.Content>
</Dialog.Root>

<style>
  /*
   * Rail rows follow DESIGN.md's Sidebar convention: `active-nav-background`
   * when active, `hover-background` on hover. Both are read through
   * `hsl(var(...))` rather than a Tailwind class because neither token is
   * mapped in the Tailwind config — the navigation sidebar reads them the same
   * way, so this matches the one existing consumer rather than inventing a
   * second access path.
   *
   * Deliberately NOT `bg-accent`: in this palette `--accent` is a saturated
   * teal, and DESIGN.md reserves teal for interactive affordances while the
   * codebase uses `bg-accent` for TRANSIENT highlight (dropdown focus,
   * autocomplete selection). A rail selection is persistent structural state,
   * which is what `active-nav-background` exists for.
   */
  .rail-item {
    display: flex;
    width: 100%;
    align-items: center;
    gap: 0.5rem;
    border-radius: calc(var(--radius) - 2px);
    padding: 0.375rem 0.5rem;
    text-align: left;
    font-size: 0.875rem;
    line-height: 1.25rem;
  }

  .rail-item:hover {
    background: hsl(var(--hover-background));
    color: hsl(var(--hover-foreground));
  }

  .rail-item--active,
  .rail-item--active:hover {
    background: hsl(var(--active-nav-background));
    color: hsl(var(--foreground));
    font-weight: 500;
  }
</style>
