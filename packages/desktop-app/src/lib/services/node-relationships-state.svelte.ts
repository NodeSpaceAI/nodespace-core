/**
 * A property form's view of its node's typed relationships.
 *
 * Every form that edits a node renders two relationship surfaces from the same
 * `get_node_relationships` load: the single-valued groups promoted to fields on
 * the form itself, and the Relationships modal's entry point, gated on whether
 * the modal has anything left to show once those groups have moved out. This
 * owns that one load and its partition so the two surfaces can never disagree
 * about which group lives where.
 */

import { createLogger } from '$lib/utils/logger';
import { loadNodeRelationshipsView } from './relationship-viewer-service';
import {
  hasModalContent,
  modalRelationshipCount,
  partitionGroups,
  type NodeRelationshipsView
} from './relationship-grouping';

const log = createLogger('NodeRelationshipsState');

/**
 * How long `scheduleReload` waits before it fetches. Edge events arrive one
 * per edge, so a burst touching one node (an import, an agent linking many
 * nodes, a reassignment's delete and create) becomes a single fetch.
 */
const RELOAD_COALESCE_MS = 100;

export class NodeRelationshipsState {
  view = $state<NodeRelationshipsView | null>(null);
  /**
   * True when the last load failed. The modal trigger fails OPEN on it, so a
   * transient failure never hides a real feature — the modal runs its own load
   * and surfaces the error there.
   */
  loadFailed = $state(false);

  readonly partitioned = $derived(partitionGroups(this.view?.groups ?? []));
  readonly showModalTrigger = $derived(this.loadFailed || hasModalContent(this.partitioned));
  /**
   * The number the modal's entry point shows. Zero after a failed load, even
   * when an earlier view is still held: a count that may be stale is not shown.
   */
  readonly modalCount = $derived(this.loadFailed ? 0 : modalRelationshipCount(this.partitioned));

  // The node whose data `view` holds or is loading.
  #nodeId: string | null = null;
  // Only the most recently STARTED fetch may write `view`. Keyed on request
  // order rather than node id: two reloads of the same node (one per quick
  // edit) can resolve out of order, and the older one must not overwrite the
  // newer.
  #generation = 0;
  #reloadTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * Load a node's relationships. A call for the node already loaded is a
   * no-op, so it is safe to drive from an effect that re-runs on unrelated
   * changes; switching nodes clears the previous node's view immediately.
   */
  load(nodeId: string): void {
    if (this.#nodeId === nodeId) return;
    this.#nodeId = nodeId;
    this.view = null;
    this.loadFailed = false;
    void this.#fetch(nodeId);
  }

  /** Re-fetch the current node after a write, keeping the view until it lands. */
  async reload(): Promise<void> {
    if (this.#nodeId) await this.#fetch(this.#nodeId);
  }

  /** Reload once after a short wait, however many times this is called in it. */
  scheduleReload(): void {
    if (this.#reloadTimer !== null) return;
    this.#reloadTimer = setTimeout(() => {
      this.#reloadTimer = null;
      void this.reload();
    }, RELOAD_COALESCE_MS);
  }

  /** Drop a scheduled reload, for an owner that is going away. */
  cancelScheduledReload(): void {
    if (this.#reloadTimer === null) return;
    clearTimeout(this.#reloadTimer);
    this.#reloadTimer = null;
  }

  async #fetch(nodeId: string): Promise<void> {
    const generation = ++this.#generation;
    try {
      const view = await loadNodeRelationshipsView(nodeId);
      if (generation !== this.#generation) return;
      this.view = view;
      this.loadFailed = false;
    } catch (error) {
      if (generation !== this.#generation) return;
      log.error('Failed to load relationships', error);
      this.loadFailed = true;
    }
  }
}
