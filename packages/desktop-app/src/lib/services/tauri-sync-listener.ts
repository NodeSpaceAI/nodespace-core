/**
 * Tauri Domain Event Listener
 *
 * Listens for real-time synchronization events emitted from the Rust backend
 * via domain events. The desktop watcher opens the daemon's `WatchNodes`
 * stream and forwards each node event to the frontend as a Tauri event.
 *
 * This module handles:
 * - Node events (created, updated, deleted) → updates SharedNodeStore
 * - Relationship events (has_child, mentions, member_of) → updates ReactiveStructureTree
 *
 * This enables real-time sync when external sources (MCP, other windows) modify data.
 *
 * Events send only node_id (not full payload) for efficiency.
 * Frontend fetches full node data via getNode() API only when the node is in the active view.
 *
 * All relationship types use unified RelationshipCreated/Updated/Deleted events.
 */

import { isA, isExactly } from '$lib/types/core-node-types';
import { listen } from '@tauri-apps/api/event';
import type {
  NodeEventData,
  RelationshipEvent,
  RelationshipDeletedPayload
} from '$lib/types/event-types';
import type { Node } from '$lib/types';
import { sharedNodeStore } from './shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { backendAdapter } from './backend-adapter';
import { createLogger } from '$lib/utils/logger';
import {
  scheduleCollectionRefresh,
  scheduleSchemaRefresh,
  scheduleAiChatRefresh,
  scheduleSavedQueryRefresh
} from '$lib/utils/collection-refresh';
import { registerSchemaPlugin, unregisterSchemaPlugin } from '$lib/plugins/schema-plugin-loader';
import { applyHasChildCreated, applyHasChildUpdated, applyHasChildDeleted } from './hierarchy-sync';
import { normalizeNodeData } from './node-normalize';
import { savedQueriesData } from '$lib/stores/saved-queries.svelte';
import { isActiveDatabaseEvent } from '$lib/stores/database.svelte';

const log = createLogger('TauriSync');

// ---------------------------------------------------------------------------
// Burst render coalescing
//
// Any large burst of node events — a folder import, a reconnect replay, a bulk
// CLI or agent write — applied one-by-one makes each `node:created/updated`
// trigger its own async fetch + `setNode` → one re-render per node, freezing the
// webview main thread and queueing every other IPC reply (an import command's
// completion included) behind the flood. To render the burst in one pass we
// collect the node ids over a tiny window, fetch them in bounded chunks, then
// apply each chunk in a SYNCHRONOUS `setNode` loop — Svelte batches synchronous
// store mutations into a single render. It only reorders when already-received
// local events are applied (one frame later), never what is applied, and
// delete-wins ordering is preserved below.
// ---------------------------------------------------------------------------

/** How long to gather a burst before flushing. One frame (~16ms) is enough to
 *  collect a daemon-flushed batch without adding perceptible latency. */
const REPLAY_COALESCE_WINDOW_MS = 16;

/** Max node fetches dispatched and applied per chunk of a coalesced flush.
 *  Chunks are separated by a macrotask yield so an initial-pull burst of tens
 *  of thousands of nodes cannot monopolize the webview main thread. */
const NODE_FETCH_CHUNK_SIZE = 200;

// Queued node id -> database epoch at enqueue time (ADR-053). Stamping at enqueue
// (like enqueueHasChildOp) lets the flush drop ids queued before a database switch
// even though the flush itself only runs after the switch.
const pendingNodeIds = new Map<string, number>();
let coalesceTimer: ReturnType<typeof setTimeout> | null = null;
// Ids deleted while a flush is mid-fetch (between the snapshot and the apply
// loop). The flush skips these so a delete that lands during the fetch wins over
// the now-stale upsert it raced — without this, the re-fetch could resurrect a
// just-deleted node (a failure class opened anew by coalescing).
let flushInProgress = false;
const tombstonedDuringFlush = new Set<string>();

/** Drop any queued/in-flight re-fetch for a node that was just deleted, so the
 *  delete wins over a racing upsert in the same coalescing window. */
function cancelPendingNodeFetch(nodeId: string): void {
  pendingNodeIds.delete(nodeId);
  if (flushInProgress) tombstonedDuringFlush.add(nodeId);
}

/** Reset all coalescer state. Called on (re)init so a stale in-flight timer from
 *  a prior listener registration can't fire into fresh state. */
function resetNodeFetchCoalescer(): void {
  if (coalesceTimer !== null) {
    clearTimeout(coalesceTimer);
    coalesceTimer = null;
  }
  pendingNodeIds.clear();
  tombstonedDuringFlush.clear();
  flushInProgress = false;
}

/** Queue a node id for the next coalesced flush . */
function enqueueNodeFetch(nodeId: string): void {
  pendingNodeIds.set(nodeId, sharedNodeStore.currentEpoch());
  if (coalesceTimer !== null) return;
  coalesceTimer = setTimeout(runNodeFlush, REPLAY_COALESCE_WINDOW_MS);
}

/** Timer entry point: the flush is async, so a rejection must be logged here
 *  rather than escape as an unhandled promise rejection. */
function runNodeFlush(): void {
  flushPendingNodeFetches().catch((error) => log.error('burst-coalesce: flush failed', error));
}

/** Fetch all queued nodes in chunks, applying each chunk in one synchronous
 *  pass so it renders once, with a macrotask yield between chunks so a huge
 *  burst never monopolizes the main thread. A failed fetch for a single node
 *  is skipped, never fatal; a node tombstoned (deleted) during the flush is
 *  skipped so the delete wins. */
async function flushPendingNodeFetches(): Promise<void> {
  coalesceTimer = null;
  // A previous flush is still working through its chunks — re-arm the window
  // so this batch flushes after it completes. Two interleaved flushes would
  // share (and prematurely clear) the tombstone/in-progress state.
  if (flushInProgress) {
    coalesceTimer = setTimeout(runNodeFlush, REPLAY_COALESCE_WINDOW_MS);
    return;
  }
  const queued = [...pendingNodeIds];
  pendingNodeIds.clear();
  if (queued.length === 0) return;

  // Drop ids enqueued under a previous database epoch: they belong to the old
  // database and must not be fetched and applied under the new one.
  const flushEpoch = sharedNodeStore.currentEpoch();
  const ids = queued.filter(([, enqueuedEpoch]) => enqueuedEpoch === flushEpoch).map(([id]) => id);
  if (ids.length < queued.length) {
    log.info('replay-coalesce: dropped node ids queued before a database switch', {
      dropped: queued.length - ids.length
    });
  }
  if (ids.length === 0) return;

  flushInProgress = true;
  tombstonedDuringFlush.clear();
  try {
    // ADR-053: capture the database generation before the reads so a switch
    // mid-flush drops the rest of the burst rather than writing the previous
    // database's rows into the now-active store. isActiveDatabaseEvent gates on
    // event arrival, before these async fetches dispatch, so it cannot close
    // this in-flight window on its own.
    const epoch = flushEpoch;
    let applied = 0;
    for (let start = 0; start < ids.length; start += NODE_FETCH_CHUNK_SIZE) {
      if (start > 0) {
        // Yield a macrotask between chunks so rendering and input handling
        // stay responsive while a large burst is applied.
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
      }
      // Re-check the generation guard per chunk: a database switch during an
      // earlier chunk or the yield must drop the remainder of the burst, not
      // just the chunk whose fetch it happened to race.
      if (sharedNodeStore.currentEpoch() !== epoch) return;

      const chunk = ids.slice(start, start + NODE_FETCH_CHUNK_SIZE);
      const fetched = await Promise.all(
        chunk.map((id) =>
          backendAdapter.getNode(id).catch((error) => {
            log.error('replay-coalesce: failed to fetch node', { nodeId: id, error });
            return null;
          })
        )
      );

      // The active database switched while these reads were in flight — the
      // rows belong to the previous database, so apply none of them.
      if (sharedNodeStore.currentEpoch() !== epoch) return;

      // Synchronous apply loop — no `await` between setNode calls, so Svelte
      // coalesces the chunk's reactive updates into a single render. A node
      // deleted while we were fetching is skipped so the delete is not clobbered.
      for (let i = 0; i < fetched.length; i++) {
        const node = fetched[i];
        if (!node || tombstonedDuringFlush.has(chunk[i])) continue;
        // One node that fails to apply must not abort the rest of the burst.
        try {
          const normalizedNode = normalizeNodeData(node);
          sharedNodeStore.setNode(
            normalizedNode,
            { type: 'database', reason: 'domain-event' },
            true
          );
          maybeRefreshSchemaPlugin(normalizedNode);
          maybeRefreshAiChats(normalizedNode);
          maybeRefreshSavedQueries(normalizedNode);
          applied++;
        } catch (error) {
          log.error('burst-coalesce: failed to apply node', { nodeId: chunk[i], error });
        }
      }
    }
    log.info('replay-coalesce: applied node burst', {
      requested: ids.length,
      applied
    });
  } finally {
    flushInProgress = false;
    tombstonedDuringFlush.clear();
  }
}

/**
 * If the given (already-fetched) node is a non-core schema, refresh its
 * plugin registration so `pluginRegistry.hasTitleTemplate`/`getTitleTemplate`
 * pick up whatever changed.
 *
 * Called after every node fetch this listener applies to the store — both
 * `node:created` (redundant with, but harmless alongside, the immediate
 * payload-based call in the `node:created` handler below: `registerSchemaPlugin`
 * upserts, so registering the same data twice is a no-op) and, critically,
 * `node:updated` — whose event payload never carries `nodeType` (see
 * `NodeEventData`), so this is the only point where an update to an existing
 * schema (e.g. `update_schema` adding a `title_template`) is even detectable.
 * Fire-and-forget: a failure here only leaves that type's title stale, never
 * fatal to the node update itself.
 */
function maybeRefreshSchemaPlugin(node: Node): void {
  if (!isExactly(node.nodeType, 'schema')) return;
  registerSchemaPlugin(node.id).catch((err) =>
    log.error('Failed to refresh schema plugin on node event:', err)
  );
}

/**
 * If the given (already-fetched) node is an ai-chat, schedule a debounced
 * refresh of the AI Chats sidebar list.
 *
 * Unlike `node:created` (whose payload carries `nodeType`, letting the
 * collections/schema handlers below branch on the raw event), `node:updated`'s
 * payload never does (see `NodeEventData`) — so gating on the fetched node's
 * type here, rather than the event payload, is the only way to detect an
 * ai-chat *update* at all. It also naturally covers `node:created` for free,
 * since both event types route through this same fetch-then-apply path
 * (`flushPendingNodeFetches`), so one hook handles both
 * "an external chat was created" and "background titling updated a chat's
 * content" (mirrors `maybeRefreshSchemaPlugin`'s reasoning for `node:updated`).
 */
function maybeRefreshAiChats(node: Node): void {
  if (!isA(node.nodeType, 'ai-chat')) return;
  scheduleAiChatRefresh();
}

/**
 * If the given (already-fetched) node is a query, schedule a debounced refresh
 * of the saved queries listed in the sidebar. Gated on the fetched node's type
 * for the same reason as `maybeRefreshAiChats`: `node:updated` carries no type.
 */
function maybeRefreshSavedQueries(node: Node): void {
  if (!isA(node.nodeType, 'query')) return;
  scheduleSavedQueryRefresh();
}

// ---------------------------------------------------------------------------
// has_child relationship-event coalescing
//
// A large import or a bulk CLI or agent write floods the event stream with tens
// of thousands of relationship events. Applied one-by-one, each
// `relationship:*` synchronously mutates the structure tree and triggers a
// full reactive invalidation on the webview main thread, freezing the UI
// (and backing up the daemon's WatchNodes stream until it drops events).
//
// We instead buffer has_child ops over the same small
// window the node coalescer uses, then apply the whole burst inside a single
// structureTree.runBatch — one reactive notification per burst. Ops are
// applied strictly in arrival order through the same per-event appliers, so
// a create followed by a delete of one edge (or the reverse) resolves exactly
// as it would un-coalesced — the delete-wins/tombstone semantics fall out of
// order preservation rather than a separate reconciliation pass.
//
// ---------------------------------------------------------------------------

interface HasChildOp {
  kind: 'created' | 'updated' | 'deleted';
  parentId: string;
  childId: string;
  order: unknown;
  /** Database generation at arrival — ops from before a switch are dropped. */
  epoch: number;
}

let pendingHasChildOps: HasChildOp[] = [];
let relationshipCoalesceTimer: ReturnType<typeof setTimeout> | null = null;

/** Reset all relationship-coalescer state. Called on (re)init so a stale
 *  in-flight timer from a prior listener registration can't fire into fresh
 *  state. */
function resetRelationshipCoalescer(): void {
  if (relationshipCoalesceTimer !== null) {
    clearTimeout(relationshipCoalesceTimer);
    relationshipCoalesceTimer = null;
  }
  pendingHasChildOps = [];
}

/** Queue a has_child op for the next coalesced flush . Arrival
 *  order is preserved so interleaved creates/deletes of the same edge resolve
 *  the same way they would applied one-by-one. */
function enqueueHasChildOp(op: Omit<HasChildOp, 'epoch'>): void {
  pendingHasChildOps.push({ ...op, epoch: sharedNodeStore.currentEpoch() });
  if (relationshipCoalesceTimer !== null) return;
  relationshipCoalesceTimer = setTimeout(flushPendingHasChildOps, REPLAY_COALESCE_WINDOW_MS);
}

/** Apply all queued has_child ops in arrival order inside one structureTree
 *  batch, so the burst costs a single reactive invalidation. Ops that arrived
 *  before a database switch are dropped (the tree was rebuilt for the new
 *  database; those edges belong to the old one). */
function flushPendingHasChildOps(): void {
  relationshipCoalesceTimer = null;
  const ops = pendingHasChildOps;
  pendingHasChildOps = [];
  if (ops.length === 0) return;

  const epoch = sharedNodeStore.currentEpoch();
  let applied = 0;
  structureTree.runBatch(() => {
    for (const op of ops) {
      if (op.epoch !== epoch) continue;
      if (op.kind === 'created') {
        applyHasChildCreated(structureTree, {
          parentId: op.parentId,
          childId: op.childId,
          order: op.order
        });
      } else if (op.kind === 'updated') {
        applyHasChildUpdated(structureTree, {
          parentId: op.parentId,
          childId: op.childId,
          order: op.order
        });
      } else {
        applyHasChildDeleted(structureTree, { parentId: op.parentId, childId: op.childId });
      }
      applied++;
    }
  });
  log.info('relationship-coalesce: applied has_child burst in one batch', {
    queued: ops.length,
    applied
  });
}

/**
 * Strip the `node:` table prefix from a stored record id so it
 * matches the bare-id key shape `reactiveStructureTree` uses
 * elsewhere in the app (the date-page route, the outliner's
 * local-action `addChild` path, and `sharedNodeStore` all key by
 * bare ids). Backend `RelationshipEvent` payloads carry the
 * prefixed form per the serialization contract; the frontend's
 * tree-keyspace is historically bare, so normalize at the boundary.
 */
function stripNodePrefix(id: string): string {
  return id.startsWith('node:') ? id.slice('node:'.length) : id;
}

/**
 * Initialize Tauri real-time synchronization event listeners
 *
 * Registers listeners for backend node and relationship events and applies
 * them to the frontend stores.
 * Should be called once during app initialization.
 *
 * @returns Promise resolving when all listeners are registered
 */
export async function initializeTauriSyncListeners(): Promise<void> {
  if (!isRunningInTauri()) {
    log.debug('Not running in Tauri environment, skipping sync listener initialization');
    return;
  }

  log.info('Initializing Tauri real-time sync listeners');

  // Clear any coalescer state (and stale in-flight timers) from a prior init.
  resetNodeFetchCoalescer();
  resetRelationshipCoalescer();

  try {
    // Listen for node events and update SharedNodeStore
    // Events send only node_id, fetch full data if needed
    // node:created includes nodeType for reactive UI updates
    await listen<NodeEventData>('node:created', (event) => {
      // ADR-053: drop events from a database we are no longer viewing (guards
      // the race where a watch stream open across a switch delivers stale events).
      if (!isActiveDatabaseEvent(event.payload.databaseId)) return;
      log.debug(`Node created: ${event.payload.id} (type: ${event.payload.nodeType})`);

      // If a collection node is created, refresh collections sidebar
      if (isA(event.payload.nodeType, 'collection')) {
        scheduleCollectionRefresh();
      }

      // If a schema node is created, refresh the node types sidebar
      if (isExactly(event.payload.nodeType, 'schema')) {
        scheduleSchemaRefresh();
        registerSchemaPlugin(event.payload.id).catch((err) =>
          log.error('Failed to register schema plugin:', err)
        );
      }

      // If a query node is created, refresh the saved queries in the sidebar
      if (isA(event.payload.nodeType, 'query')) {
        scheduleSavedQueryRefresh();
      }

      // Fetch full node data since the node might be in the current view.
      // Bursts are coalesced into one render per chunk.
      enqueueNodeFetch(event.payload.id);
    });

    await listen<NodeEventData>('node:updated', (event) => {
      if (!isActiveDatabaseEvent(event.payload.databaseId)) return;
      const nodeId = event.payload.id;
      log.debug(`node:updated received`, { nodeId });
      enqueueNodeFetch(nodeId);
    });

    await listen<NodeEventData>('node:deleted', (event) => {
      if (!isActiveDatabaseEvent(event.payload.databaseId)) return;
      log.debug(`Node deleted: ${event.payload.id}`);
      // Evict any coalesced re-fetch first so a delete racing an upsert in the
      // same window can't be clobbered by the queued fetch re-adding the node.
      cancelPendingNodeFetch(event.payload.id);
      sharedNodeStore.deleteNode(
        event.payload.id,
        { type: 'database', reason: 'domain-event' },
        true
      );

      // We don't know if deleted node was a collection without fetching,
      // but if we have it cached in collectionsData, we should refresh
      // For simplicity, we rely on the UI to handle stale data gracefully
      // A more robust solution would cache node types or include type in delete events
      unregisterSchemaPlugin(event.payload.id);

      // The delete event carries no node type, so refresh only when the
      // deleted node is a listed saved query.
      if (savedQueriesData.has(event.payload.id)) {
        scheduleSavedQueryRefresh();
      }
    });

    // ========================================================================
    // Unified Relationship Events
    // All relationship types (has_child, member_of, mentions, custom) use these events.
    // ========================================================================

    await listen<RelationshipEvent>('relationship:created', (event) => {
      if (!isActiveDatabaseEvent(event.payload.databaseId)) return;
      const rel = event.payload;
      log.debug(`Relationship created: ${rel.relationshipType} (${rel.fromId} -> ${rel.toId})`);

      // Handle different relationship types
      if (rel.relationshipType === 'has_child') {
        // Coalesce a burst into one tree batch.
        enqueueHasChildOp({
          kind: 'created',
          parentId: stripNodePrefix(rel.fromId),
          childId: stripNodePrefix(rel.toId),
          order: (rel.properties as { order?: unknown } | undefined)?.order
        });
      } else if (rel.relationshipType === 'member_of') {
        // Collection membership changed - refresh collections sidebar.
        // `scheduleCollectionRefresh` compares the passed id against
        // `state.selectedCollectionId`, which is keyed by bare ids
        // elsewhere in the app — strip the `node:` prefix the
        // serialization contract requires.
        const toId = stripNodePrefix(rel.toId);
        log.debug(`Member added: ${rel.fromId} to collection ${toId}`);
        scheduleCollectionRefresh(toId);
      } else if (rel.relationshipType === 'mentions') {
        // Mention relationship created - target node's backlinks need refresh.
        // mentionedIn is a separate, independently-fetched resource (not carried
        // on the node payload) — refetch it directly for the target rather than
        // reloading the whole tree. Bare-id keyspace, same rationale as
        // `member_of` above.
        const toId = stripNodePrefix(rel.toId);
        log.debug(`Mention created: ${stripNodePrefix(rel.fromId)} mentions ${toId}`);
        sharedNodeStore
          .refreshMentionedIn(toId)
          .catch((err) => log.error(`Failed to refresh mentionedIn for ${toId}:`, err));
      } else {
        // Custom relationship type
        log.debug(`Custom relationship created: ${rel.relationshipType}`);
      }
    });

    await listen<RelationshipEvent>('relationship:updated', (event) => {
      if (!isActiveDatabaseEvent(event.payload.databaseId)) return;
      const rel = event.payload;
      log.debug(`Relationship updated: ${rel.relationshipType} (${rel.fromId} -> ${rel.toId})`);
      if (rel.relationshipType === 'has_child') {
        enqueueHasChildOp({
          kind: 'updated',
          parentId: stripNodePrefix(rel.fromId),
          childId: stripNodePrefix(rel.toId),
          order: rel.properties?.order
        });
      }
    });

    await listen<RelationshipDeletedPayload>('relationship:deleted', (event) => {
      if (!isActiveDatabaseEvent(event.payload.databaseId)) return;
      const { id, fromId, toId, relationshipType } = event.payload;
      log.debug(`Relationship deleted: ${relationshipType} (${id}) from ${fromId} to ${toId}`);

      if (relationshipType === 'has_child') {
        enqueueHasChildOp({
          kind: 'deleted',
          parentId: stripNodePrefix(fromId),
          childId: stripNodePrefix(toId),
          order: undefined
        });
      } else if (relationshipType === 'member_of') {
        // Collection membership removed - refresh collections sidebar.
        // Bare-id keyspace, same rationale as `relationship:created`
        // above.
        const bareToId = stripNodePrefix(toId);
        log.debug(`Member removed from collection: ${id}`);
        scheduleCollectionRefresh(bareToId);
      } else if (relationshipType === 'mentions') {
        // Mention relationship deleted - target node's backlinks need refresh.
        // Same rationale as the created branch above.
        const bareToId = stripNodePrefix(toId);
        log.debug(`Mention deleted: ${id} (${stripNodePrefix(fromId)} -> ${bareToId})`);
        sharedNodeStore
          .refreshMentionedIn(bareToId)
          .catch((err) => log.error(`Failed to refresh mentionedIn for ${bareToId}:`, err));
      }
    });

    log.info('Real-time sync listeners initialized successfully');
  } catch (error) {
    log.error('Failed to initialize sync listeners', error);
    throw new Error(`Failed to initialize sync listeners: ${error}`);
  }
}

/**
 * Check if running in Tauri environment
 */
function isRunningInTauri(): boolean {
  return (
    typeof window !== 'undefined' && ('__TAURI__' in window || '__TAURI_INTERNALS__' in window)
  );
}
