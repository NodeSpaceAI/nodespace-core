/**
 * AI Chats Store
 *
 * Manages the state for the "AI Chats" section in the navigation sidebar:
 * the list of existing ai-chat nodes (most-recently-modified first) and the
 * "+ New chat" create action.
 *
 * Mirrors `collections.svelte.ts`'s data-store shape (a `load*` method called
 * from the sidebar's `onMount`, reactive `$state`), simplified — unlike
 * collections there is no hide-empty filter to work around, so a newly
 * created chat is simply prepended to the loaded list rather than needing an
 * optimistic-insert/reconcile dance.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { createSchemaInstance } from '$lib/services/schema-authoring';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import type { Node } from '$lib/types';

const log = createLogger('AiChatsStore');

/**
 * Cap on how many ai-chat nodes are shown in the sidebar, applied AFTER
 * sorting. The backend query has no order-by (see `NodeQuery`), so the fetch
 * itself is unbounded — passing `limit` to `queryNodes` would ask SQLite for
 * an arbitrary (effectively insertion-order) subset with no ORDER BY clause,
 * and sorting that subset afterward could not recover chats the LIMIT had
 * already excluded, silently hiding genuinely-recent ones. Fetching
 * everything and slicing after the sort mirrors `collectionsData.loadCollections()`,
 * which also fetches its full list with no limit. The list itself scrolls
 * past this cap, matching how Collections / Schema Types handle overflow.
 */
const DISPLAY_LIMIT = 50;

export interface AiChatListItem {
  id: string;
  /** Raw node content. Empty until background titling fills it in. */
  content: string;
  modifiedAt: string;
}

interface AiChatsState {
  chats: AiChatListItem[];
  loading: boolean;
  error: string | null;
}

const initialState: AiChatsState = {
  chats: [],
  loading: false,
  error: null,
};

function toListItem(node: Node): AiChatListItem {
  return { id: node.id, content: node.content, modifiedAt: node.modifiedAt };
}

class AiChatsStore {
  state = $state<AiChatsState>({ ...initialState });

  /** True while a "+ New chat" create is in flight — disables the button. */
  createBusy = $state(false);
  /** Message from the most recent failed create, cleared on the next attempt. */
  createError = $state('');

  /**
   * Bumped whenever the store stops representing the database it did —
   * `invalidateForDatabaseSwitch()`. An in-flight `createChat()` captures
   * this before awaiting and discards its result if the value changed, so a
   * create issued against the previous database cannot write its node into
   * the store representing the newly-active one (mirrors
   * `collectionsData`'s `#generation` guard around `createCollection`).
   */
  #generation = 0;

  /** Load ai-chat nodes from the backend, most-recently-modified first. */
  async loadAiChats(): Promise<void> {
    // A database switch landed while this load was in flight: the fetched
    // nodes belong to the database we just left, so drop them rather than
    // writing them into a store that now represents a different database
    // (mirrors the same guard already around `createChat`).
    const generation = this.#generation;
    this.state = { ...this.state, loading: true, error: null };

    try {
      // No `limit` on the fetch: see DISPLAY_LIMIT's comment — the backend
      // query has no order-by, so limiting before the client-side sort could
      // silently exclude the true most-recent chats.
      const nodes = await backendAdapter.queryNodes({ nodeType: 'ai-chat' });
      if (generation !== this.#generation) {
        log.debug('Discarding AI chats load that resolved after the store moved on');
        return;
      }
      const chats = [...nodes]
        .sort((a, b) => new Date(b.modifiedAt).getTime() - new Date(a.modifiedAt).getTime())
        .slice(0, DISPLAY_LIMIT)
        .map(toListItem);
      log.debug('Loaded AI chats', { count: chats.length, totalFetched: nodes.length });
      this.state = { ...this.state, chats, loading: false };
    } catch (err) {
      if (generation !== this.#generation) return;
      const message = err instanceof Error ? err.message : 'Failed to load AI chats';
      log.error('Failed to load AI chats', { error: message });
      this.state = { ...this.state, loading: false, error: message };
    }
  }

  /**
   * Create a new ai-chat node immediately — no name prompt, since a chat's
   * title comes from its content later rather than being required up front.
   * Prepends the new chat to the local list so it appears without a reload.
   *
   * Returns the created node, or null on failure (`createError` is set for
   * the caller to surface; the caller re-enables its own button once this
   * resolves) or when a database switch invalidated this create while it was
   * in flight (silently discarded — the node is still persisted, in the
   * database that was left, so there is nothing to report or retry here).
   */
  async createChat(): Promise<Node | null> {
    if (this.createBusy) return null;
    const generation = this.#generation;
    this.createBusy = true;
    this.createError = '';

    try {
      const created = await createSchemaInstance('ai-chat');
      if (generation !== this.#generation) return null;
      // `createSchemaInstance` creates the node directly via `backendAdapter`,
      // bypassing SharedNodeStore entirely — it never lands in `persistedNodeIds`.
      // Seed the store now (hydrate + mark persisted in one step, same as
      // QueryNodeViewer's `handleCreateInstance` already does for its own
      // "+ New" call site) so the viewer's first write, once the tab opens,
      // finds the id already known-persisted. Without this, the id is only
      // marked persisted once the sync/domain-event echo of this very create
      // arrives — and if the viewer's first write (model selection or the
      // first message) races ahead of that echo, it wrongly re-issues a
      // CREATE for an id that already has a row, colliding with it.
      sharedNodeStore.setNode(created, { type: 'database', reason: 'ai-chat-created' });
      this.state = { ...this.state, chats: [toListItem(created), ...this.state.chats] };
      return created;
    } catch (err) {
      if (generation !== this.#generation) return null;
      const message = err instanceof Error ? err.message : 'Failed to create chat';
      log.error('Failed to create AI chat', { error: message });
      this.createError = message;
      return null;
    } finally {
      this.createBusy = false;
    }
  }

  /**
   * Patch a chat's content in the local list after a rename, so the sidebar
   * reflects it immediately without a full reload. Mirrors `createChat`'s
   * optimistic local update. No-op if the chat isn't in the loaded list (e.g.
   * it fell outside `DISPLAY_LIMIT`) — the next `loadAiChats` will pick up
   * the persisted value regardless.
   */
  updateChatContent(id: string, content: string): void {
    const index = this.state.chats.findIndex((chat) => chat.id === id);
    if (index === -1) return;
    const chats = [...this.state.chats];
    chats[index] = { ...chats[index], content };
    this.state = { ...this.state, chats };
  }

  /**
   * Invalidate any create issued against a database this store no longer
   * represents, and drop a stale create-error banner left over from it. Call
   * before reloading for a newly-active database (mirrors
   * `collectionsData.forgetLocallyCreated()`); `loadAiChats` itself
   * overwrites `chats` wholesale so no separate list reset is needed.
   */
  invalidateForDatabaseSwitch(): void {
    this.#generation++;
    this.createError = '';
  }

  /** Reset to initial state (test use only — no production caller needs
   * this; production database switches use `invalidateForDatabaseSwitch()`
   * followed by `loadAiChats()`, which replaces `chats` wholesale). Bumps
   * `#generation` too (mirrors `collectionsData.reset()`), so an in-flight
   * `loadAiChats`/`createChat` that resolves after a test calls this cannot
   * write stale data into the state the reset just established. */
  reset(): void {
    this.#generation++;
    this.state = { ...initialState };
    this.createBusy = false;
    this.createError = '';
  }
}

export const aiChatsData = new AiChatsStore();

/** Load ai-chat nodes from the backend and update the store. */
export const loadAiChats = (): Promise<void> => aiChatsData.loadAiChats();

// Registered once at module load (this file is a singleton — ES modules only
// evaluate once), not per component mount. Retries loadAiChats whenever the
// daemon becomes reachable, so a load that failed while the daemon was still
// starting up recovers automatically without a manual reload.
onDaemonReconnect(() => aiChatsData.loadAiChats());
