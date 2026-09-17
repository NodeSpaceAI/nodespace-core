<!--
  KanbanView - Board view for QueryNodeViewer

  Groups results into columns by the values of a selected enum field (the only
  eligible group-by kind — enum values give both the column set and their labels).
  Cards show the node title and read from sharedNodeStore, so a change made in
  another pane (or by a drag here) moves the card reactively. Dragging a card to
  another column — or choosing a column from the card's keyboard-accessible
  "Move to" control — writes that column's value onto the node. Nodes with no
  value land in an "Unassigned" column.

  Each column renders at most a capped batch of cards (matching List/Table's
  PAGE_SIZE) rather than every matching node — a column with thousands of
  cards would otherwise render thousands of card-plus-full-options-<select>
  pairs regardless of scroll position. A "+N more" control grows that
  column's *set* of revealed cards by one more batch; revealing is tracked by
  node id, not by position, so a card already on screen can't vanish because
  some other card's bucket membership changed elsewhere in the result order
  (slicing by position alone can't make that guarantee — inserting a card
  ahead of an already-shown one would push the shown one past a plain
  positional cutoff). True virtualization (windowing) was considered and
  rejected here: this view's drag source is the rendered DOM node itself
  (native HTML5 drag-and-drop), and a windowed list unmounts off-screen rows —
  a card scrolled out of the window mid-drag would be destroyed out from
  under its own drag operation.
-->

<script lang="ts">
  import { untrack } from 'svelte';
  import { SvelteMap } from 'svelte/reactivity';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import { createLogger } from '$lib/utils/logger';
  import type { Node } from '$lib/types';
  import type { SchemaNode } from '$lib/types/schema-node';
  import { labelForField } from '$lib/utils/schema-field-label';
  import {
    UNASSIGNED,
    eligibleGroupByFields,
    enumColumns,
    readGroupValue,
    resolveFieldWrite,
    groupByColumn,
    resolveActiveGroupBy,
    growRevealed
  } from '$lib/components/query/kanban-grouping';

  const log = createLogger('KanbanView');

  // Matches List/Table's PAGE_SIZE — the number of cards a column shows
  // initially and grows by per "+N more" click.
  const CARDS_PER_BATCH = 25;

  let {
    nodeIds,
    schema,
    groupBy,
    onGroupByChange,
    onRowClick
  }: {
    nodeIds: string[];
    schema: SchemaNode | null;
    /**
     * The group-by field for this board, sourced from the query node's
     * `viewConfig.kanban.groupBy`. `undefined` means "not chosen
     * yet — fall back to the first eligible enum field".
     */
    groupBy: string | undefined;
    /**
     * Persist a group-by choice. For a saved query this writes the query node's
     * view config; for the default type view it materializes a query node. The
     * viewer owns which — KanbanView only reports the choice.
     */
    onGroupByChange: (_groupBy: string) => void;
    onRowClick: (_nodeId: string) => void;
  } = $props();

  // The user's explicit picker choice this session; null means "use the
  // query node's stored group-by, or no selection at all" — an unset
  // group-by no longer falls back to an arbitrary eligible field.
  let picked = $state<string | null>(null);
  let draggingId = $state<string | null>(null);
  let dragOverColumn = $state<string | null>(null);

  const eligible = $derived(eligibleGroupByFields(schema));
  const noEnumField = $derived(eligible.length === 0);
  const activeGroupBy = $derived(picked ?? resolveActiveGroupBy(eligible, groupBy));
  const activeField = $derived(eligible.find((f) => f.name === activeGroupBy) ?? null);

  // Columns from the enum's values, plus a trailing Unassigned bucket — but
  // only when the field can actually hold "no value". A `required: true`
  // field (e.g. task's `status`, which the backend models as a
  // non-nullable `TaskStatus` with no clear semantics — see
  // `resolveFieldWrite`'s doc comment) always has a real value on every
  // node, so its board would never naturally bucket anything into
  // Unassigned; offering it as a drop target would just silently no-op the
  // write instead of clearing the field (there is nothing to clear it TO).
  const columns = $derived(enumColumns(activeField));
  const displayColumns = $derived(
    activeField?.required ? columns : [...columns, { value: UNASSIGNED, label: 'Unassigned' }]
  );

  // Bucket the (existing) result nodes by their group-by value. Reads each node
  // from the store, so a move — which rewrites the value — re-derives the board.
  const buckets = $derived.by(() => {
    if (!activeGroupBy) return new Map<string, string[]>();
    const items = nodeIds
      .map((id) => {
        const n = sharedNodeStore.getNode(id);
        return n ? { id, value: readGroupValue(n, activeGroupBy) } : null;
      })
      .filter((it): it is { id: string; value: string | null } => it !== null);
    return groupByColumn(items, columns.map((c) => c.value));
  });

  // The set of node ids currently revealed per column — see CARDS_PER_BATCH
  // above. Keyed by column value. Reseeded with the first batch (by current
  // bucket order) whenever the grouping field changes (a different field's
  // column values are a different vocabulary, so both the columns themselves
  // and any prior reveal state keyed by value string — e.g. two fields both
  // having "open" — must reset together) OR whenever `nodeIds` itself
  // changes (the query re-executed against a disjoint result set — reveal
  // state keyed to the OLD set's ids would otherwise reveal nothing for an
  // over-cap column of all-new ids, behind a stale "+N more" count).
  // `buckets` is read `untrack`-ed here deliberately: seeding must run once
  // per grouping choice / result set, not on every bucket recompute (a card
  // move recomputes buckets on every drag) — the whole point of tracking ids
  // instead of a count is that routine bucket churn from moves within the
  // SAME result set must NOT reseed/reshuffle what's already been revealed.
  let revealedIds = new SvelteMap<string, Set<string>>();
  $effect(() => {
    const cols = displayColumns;
    // Tracked dependency: reseed whenever the result set itself changes,
    // even though buckets (derived from it) is read untracked below.
    const _nodeIdsDep = nodeIds;
    void _nodeIdsDep;
    const currentBuckets = untrack(() => buckets);
    revealedIds.clear();
    for (const col of cols) {
      const ids = currentBuckets.get(col.value) ?? [];
      revealedIds.set(col.value, growRevealed(new Set(), ids, CARDS_PER_BATCH));
    }
  });

  /**
   * The visible cards for a column: everything, if the column doesn't
   * currently exceed one batch — the common case, and the one that matters
   * for the interactive drag/drop and keyboard-move flows: a card just
   * moved into a small column must appear immediately, not sit hidden
   * behind "+N more" pending a click nobody asked for. Only once a column
   * genuinely exceeds the batch size does the revealed set take over,
   * showing whichever of its current members are in that set, in current
   * bucket order — `moveCard` below separately ensures a card THIS view
   * just placed is always in that set, so a drop into an already-oversized
   * column is visible too, not just the under-cap case this function
   * special-cases directly.
   *
   * Before the seeding effect above has run for a brand-new column (e.g.
   * the render that follows a groupBy switch, same tick), fall back to a
   * plain positional slice — `growRevealed` from an empty set produces the
   * identical result, so the effect's write that follows changes nothing
   * the user can see.
   */
  function visibleIdsFor(columnValue: string, ids: string[]): string[] {
    if (ids.length <= CARDS_PER_BATCH) return ids;
    const revealed = revealedIds.get(columnValue);
    if (!revealed) return ids.slice(0, CARDS_PER_BATCH);
    return ids.filter((id) => revealed.has(id));
  }

  /**
   * Read-modify-write a column's revealed set: look it up (or start from
   * empty), hand it to `compute`, store whatever comes back. `compute` must
   * treat its input as immutable and return a new `Set` (or the same
   * instance, unchanged, for a no-op) — `SvelteMap` only picks up reactivity
   * on `.set()`, not on mutating a `Set` already inside it.
   */
  function updateRevealed(
    columnValue: string,
    compute: (_revealed: Set<string>) => Set<string>
  ): void {
    const current = revealedIds.get(columnValue) ?? new Set<string>();
    revealedIds.set(columnValue, compute(current));
  }

  function showMore(columnValue: string, ids: string[]): void {
    updateRevealed(columnValue, (revealed) => growRevealed(revealed, ids, CARDS_PER_BATCH));
  }

  function titleOf(node: Node): string {
    return pluginRegistry.resolveDisplayTitle(node) || 'Untitled';
  }

  /** The column a node currently belongs to (for the move control's value). */
  function currentColumn(node: Node): string {
    const v = activeGroupBy ? readGroupValue(node, activeGroupBy) : null;
    return v !== null && columns.some((c) => c.value === v) ? v : UNASSIGNED;
  }

  function onPickGroupBy(name: string): void {
    picked = name;
    onGroupByChange(name);
  }

  // The value a node's group-by field held before the start of a *chain* of
  // still-unsettled moves — not necessarily what the most recent move in
  // that chain itself read, which can be an earlier move's own optimistic
  // (and, if this one also fails, equally unconfirmed) value. Without this,
  // two rapid moves of the same card that BOTH fail would revert to the
  // first move's target instead of the last confirmed value: move 1 (A→B)
  // fails and reverts correctly to A, but move 2 (queued behind it, reading
  // B before move 1's failure was known) fails too and — reverting to
  // *its own* `from` of "B" — would stomp move 1's already-correct revert
  // back to a value ("B") that was never actually persisted by anyone.
  // Chained through `onPersistSuccess`/`onPersistError`: the origin is set
  // once per chain (on the first move) and cleared once the chain resolves
  // either way, so a later move's revert always targets the true starting
  // point instead of an intermediate.
  //
  // Keyed by `${id}:${field}`, not just `id`: switching the "Group by"
  // picker mid-chain changes which field `moveCard` targets, and a chain
  // keyed only by node id would let a later move for a DIFFERENT field
  // reuse — and on failure, write — an origin value that belongs to the
  // field the FIRST move in the chain was for, corrupting the second
  // field with a value from a completely different vocabulary.
  let chainOrigin = new Map<string, string | null>();

  /**
   * Move a card into the column identified by `toColumn` (UNASSIGNED clears it).
   *
   * `id` must be a member of THIS board's `nodeIds` — enforced here, not just
   * at each call site, so every current and future caller gets the same
   * guarantee for free. The one path that can hand this an id from *outside*
   * that set is onDrop's dataTransfer fallback (kanban-view is the only
   * component that sets node-id drag data, so a split view with two boards on
   * screen can hand this instance a foreign id via the shared DataTransfer);
   * the keyboard "Move to" select's id is already sourced from a loop over
   * this board's own rendered cards, so it can never actually trigger this,
   * but a defense-in-depth check at the point that reads and writes the node
   * is worth more than trusting every caller to have filtered correctly —
   * this is exactly the kind of write a wrong id here would otherwise
   * silently make onto another board's node, in a vocabulary it may not even
   * use.
   */
  function moveCard(id: string, toColumn: string): void {
    if (!nodeIds.includes(id)) return;
    const node = sharedNodeStore.getNode(id);
    if (!node || !activeGroupBy) return;
    // Captured now, for the onPersistError closure below — activeGroupBy is
    // reactive and could pick a different field by the time a failure
    // callback fires (e.g. the user switches the group-by picker while this
    // write is still in flight); the revert must always target the field
    // this specific move actually changed.
    const field = activeGroupBy;
    const from = readGroupValue(node, field);
    const target = toColumn === UNASSIGNED ? null : toColumn;
    // Defense-in-depth alongside `displayColumns` omitting Unassigned for a
    // required field (see that computation's doc comment): a required field
    // has no clear semantics on the backend, so writing `null` to it would
    // silently no-op rather than actually move the card anywhere.
    if (target === null && activeField?.required) return;
    if (from === target) return; // dropping into its own column is a no-op — no write
    const chainKey = `${id}:${field}`;
    if (!chainOrigin.has(chainKey)) chainOrigin.set(chainKey, from);
    const changes = resolveFieldWrite(node, field, target);
    log.debug('KanbanView: moving card', { id, field, toColumn });
    sharedNodeStore.updateNode(
      id,
      changes,
      { type: 'viewer', viewerId: 'kanban-view' },
      {
        // Optimistic store write: the card moves as soon as the value
        // changes. If the persisted write is rejected, revert just this
        // field back to the chain's origin value — a correction scoped
        // to this node's field, not a store-wide mechanism (an earlier
        // version routed this through a general server-resync path; review
        // surfaced real races in using that store-wide mechanism for an
        // arbitrary write failure — see onPersistError's doc comment in
        // update-protocol.ts). The store still raises its own generic
        // write-failure notification regardless; this only handles putting
        // the card back where it was.
        onPersistSuccess: () => {
          // This write's value is now the confirmed baseline — any chain
          // that was in flight for this (node, field) is resolved.
          chainOrigin.delete(chainKey);
        },
        onPersistError: () => {
          const currentNode = sharedNodeStore.getNode(id);
          if (!currentNode) {
            chainOrigin.delete(chainKey);
            return; // node no longer exists locally — nothing to revert
          }
          // Only revert if the field still holds exactly the value this
          // move set. If it doesn't, something else changed it since — most
          // likely the user dragged the same card again before this write's
          // failure was known — and reverting now would stomp on that
          // newer, unrelated intent instead of just undoing this write.
          // Leave chainOrigin alone in that case: that newer move is still
          // part of the same unresolved chain and will settle it itself.
          if (readGroupValue(currentNode, field) !== target) return;
          // `.has()`, not `??`: a card that started in "Unassigned" has a
          // genuinely-stored origin of `null` — `chainOrigin.get(chainKey)
          // ?? from` would treat that stored `null` as "nothing recorded"
          // (same as a missing key) and silently fall back to `from`
          // instead, which for a chain of 2+ moves is an intermediate,
          // unconfirmed value, exactly what this map exists to avoid.
          const revertTo = chainOrigin.has(chainKey) ? chainOrigin.get(chainKey)! : from;
          chainOrigin.delete(chainKey); // this failure settles the chain
          log.debug('KanbanView: reverting failed move', { id, field, revertTo, target });
          const revertChanges = resolveFieldWrite(currentNode, field, revertTo);
          sharedNodeStore.updateNode(
            id,
            revertChanges,
            { type: 'viewer', viewerId: 'kanban-view' },
            { skipPersistence: true }
          );
        }
      }
    );

    // Reveal the card in its destination column immediately, regardless of
    // that column's current cap — the user just explicitly placed it there
    // (by drag or the "Move to" select), so it must be visible right where
    // they put it, cap or no cap. Without this, a card dropped into an
    // already-capped (or even empty-but-unseeded) column would silently land
    // behind a "+N more" control instead of where the user just dropped it.
    // This only affects the card THIS view just moved; an unrelated card
    // arriving via some other cause (another pane, a background resync)
    // still respects the cap normally.
    updateRevealed(toColumn, (revealed) => {
      if (revealed.has(id)) return revealed;
      const next = new Set(revealed);
      next.add(id);
      return next;
    });
  }

  function onDragStart(e: DragEvent, id: string): void {
    draggingId = id;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', id);
    }
  }

  function onDragEnd(): void {
    draggingId = null;
    dragOverColumn = null;
  }

  function onDragOver(e: DragEvent, columnValue: string): void {
    e.preventDefault();
    dragOverColumn = columnValue;
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  }

  function onDrop(e: DragEvent, columnValue: string): void {
    e.preventDefault();
    const id = draggingId ?? e.dataTransfer?.getData('text/plain') ?? null;
    draggingId = null;
    dragOverColumn = null;
    // The dataTransfer fallback below can hand this a foreign id from
    // another board's drag (see moveCard's doc comment) — moveCard itself
    // is the trust boundary that rejects it, not this call site.
    if (id) moveCard(id, columnValue);
  }
</script>

<div class="kanban">
  <div class="kanban-toolbar">
    <label class="groupby">
      <span>Group by</span>
      <select
        value={activeGroupBy ?? ''}
        disabled={noEnumField}
        aria-label="Group by"
        onchange={(e) => onPickGroupBy(e.currentTarget.value)}
      >
        {#if !activeGroupBy}
          <option value="" disabled selected>Choose a field…</option>
        {/if}
        {#each eligible as f (f.name)}
          <option value={f.name}>{labelForField(f)}</option>
        {/each}
      </select>
    </label>
    {#if noEnumField}
      <span class="kanban-groupby-hint" role="status">
        This type has no enum field to group by. Add one to the schema to use the Kanban view.
      </span>
    {/if}
  </div>

  {#if noEnumField}
    <!-- Control stays visible (disabled) above so its absence isn't mistaken
         for a rendering failure — only the board itself is withheld. -->
  {:else if !activeGroupBy}
    <div class="kanban-empty">
      <p>Choose a field to group this board by.</p>
    </div>
  {:else}
    <div class="kanban-board">
      {#each displayColumns as col (col.value)}
        {@const ids = buckets.get(col.value) ?? []}
        {@const visibleIds = visibleIdsFor(col.value, ids)}
        {@const hiddenCount = ids.length - visibleIds.length}
        <section
          class="kanban-column"
          class:drag-over={dragOverColumn === col.value}
          role="group"
          aria-label={`${col.label} column`}
          ondragover={(e) => onDragOver(e, col.value)}
          ondragleave={() => (dragOverColumn = null)}
          ondrop={(e) => onDrop(e, col.value)}
        >
          <header class="kanban-column-header">
            <span class="kanban-column-title">{col.label}</span>
            <span class="kanban-column-count">{ids.length}</span>
          </header>
          <div class="kanban-cards">
            {#each visibleIds as id (id)}
              {@const node = sharedNodeStore.getNode(id)}
              {#if node}
                {@const title = titleOf(node)}
                <article
                  class="kanban-card"
                  class:dragging={draggingId === id}
                  draggable="true"
                  ondragstart={(e) => onDragStart(e, id)}
                  ondragend={onDragEnd}
                >
                  <button class="kanban-card-title" onclick={() => onRowClick(id)} title={`Open ${title}`}>
                    {title}
                  </button>
                  <select
                    class="kanban-card-move"
                    aria-label={`Move ${title} to another column`}
                    value={currentColumn(node)}
                    onchange={(e) => moveCard(id, e.currentTarget.value)}
                  >
                    {#each displayColumns as target (target.value)}
                      <option value={target.value}>{target.label}</option>
                    {/each}
                  </select>
                </article>
              {/if}
            {/each}
            {#if hiddenCount > 0}
              <button
                class="kanban-show-more"
                onclick={() => showMore(col.value, ids)}
              >+{hiddenCount} more</button>
            {/if}
          </div>
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .kanban {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    height: 100%;
  }

  .kanban-toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
  }

  .groupby {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .groupby select {
    font-size: 0.8125rem;
    padding: 0.25rem 0.5rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
  }

  .groupby select:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .kanban-groupby-hint {
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }

  .kanban-board {
    display: flex;
    gap: 0.75rem;
    align-items: flex-start;
    overflow-x: auto;
    padding-bottom: 0.5rem;
    flex: 1;
  }

  .kanban-column {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-width: 15rem;
    max-width: 18rem;
    flex: 0 0 auto;
    background: hsl(var(--muted) / 0.4);
    border: 1px solid hsl(var(--border));
    border-radius: 0.5rem;
    padding: 0.5rem;
  }

  .kanban-column.drag-over {
    background: hsl(var(--primary) / 0.08);
    border-color: hsl(var(--primary));
  }

  .kanban-column-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.25rem 0.25rem 0.5rem;
    border-bottom: 1px solid hsl(var(--border));
  }

  .kanban-column-title {
    font-size: 0.8125rem;
    font-weight: 600;
    color: hsl(var(--foreground));
  }

  .kanban-column-count {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted));
    border-radius: 9999px;
    padding: 0.0625rem 0.5rem;
  }

  .kanban-cards {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-height: 2rem;
  }

  .kanban-card {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    padding: 0.5rem;
    cursor: grab;
  }

  .kanban-card.dragging {
    opacity: 0.5;
  }

  .kanban-card-title {
    text-align: left;
    background: transparent;
    border: none;
    padding: 0;
    font-size: 0.8125rem;
    font-weight: 500;
    color: hsl(var(--foreground));
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .kanban-card-title:hover {
    color: hsl(var(--primary));
    text-decoration: underline;
  }

  .kanban-card-move {
    font-size: 0.75rem;
    padding: 0.125rem 0.25rem;
    border: 1px solid hsl(var(--border));
    border-radius: 0.25rem;
    background: hsl(var(--background));
    color: hsl(var(--muted-foreground));
  }

  .kanban-show-more {
    text-align: left;
    background: transparent;
    border: 1px dashed hsl(var(--border));
    border-radius: 0.375rem;
    padding: 0.375rem 0.5rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
  }

  .kanban-show-more:hover {
    color: hsl(var(--primary));
    border-color: hsl(var(--primary));
  }

  .kanban-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    gap: 0.5rem;
  }

  .kanban-empty p {
    margin: 0;
  }
</style>
