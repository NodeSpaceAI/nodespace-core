/**
 * Plays Store
 *
 * The plays listed in the "Plays" section of the navigation sidebar, each with
 * the state its row shows (ADR-090). The section only lists and opens plays:
 * this store makes no writes.
 *
 * Follows the `savedQueriesData` pattern: a `load*` method called from the
 * sidebar's `onMount`, refreshed by the debounced `schedulePlayRefresh` wired
 * to node domain events, reloaded after a daemon reconnect, and guarded by a
 * database-switch generation counter.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import { isA } from '$lib/types/core-node-types';
import { playState, playTitle, type PlayState } from '$lib/components/play/play-node-model';
import type { PlayNode } from '$lib/types';

const log = createLogger('PlaysStore');

export interface PlayListItem {
  id: string;
  /** The play's type, which picks its viewer. */
  nodeType: string;
  title: string;
  state: PlayState;
  /** The diagnostic a suspended play was stopped with, when it has one. */
  suspendedMessage?: string;
}

/** Case-insensitive title order, then id, so equal titles keep a stable order. */
function comparePlays(a: PlayListItem, b: PlayListItem): number {
  return (
    a.title.localeCompare(b.title, undefined, { sensitivity: 'base' }) || a.id.localeCompare(b.id)
  );
}

function toListItem(play: PlayNode): PlayListItem {
  const state = playState(play);
  return {
    id: play.id,
    nodeType: play.nodeType,
    title: playTitle(play),
    state,
    suspendedMessage: state === 'suspended' ? play.suspendedMessage : undefined
  };
}

class PlaysStore {
  /** The plays the last load returned. */
  #loaded = $state<PlayNode[]>([]);

  /**
   * The listed plays, ordered by title. A play the node store also holds is
   * read from there, so a switch flipped in its viewer shows on its row at
   * once instead of after the next load.
   */
  plays = $derived.by(() =>
    this.#loaded
      .map((loaded) => {
        const live = sharedNodeStore.getNode(loaded.id);
        return toListItem(
          live && isA(live.nodeType, 'play') ? (live as unknown as PlayNode) : loaded
        );
      })
      .sort(comparePlays)
  );

  /** See `SchemasStore.#generation`. */
  #generation = 0;

  /** True when `nodeId` is a listed play (used to react to updates and deletes). */
  has(nodeId: string): boolean {
    return this.#loaded.some((play) => play.id === nodeId);
  }

  /**
   * Load every participating play, core and user-written. The query leaves out
   * archived plays (the participation rule, ADR-087), so an archived play
   * drops off the list on the load that follows its archiving.
   */
  async loadPlays(): Promise<void> {
    const generation = this.#generation;
    try {
      const nodes = await backendAdapter.queryNodes({ nodeType: 'play' });
      if (generation !== this.#generation) {
        log.debug('Discarding plays load that resolved after the store moved on');
        return;
      }
      this.#loaded = nodes as unknown as PlayNode[];
      log.debug('Plays loaded', { count: nodes.length });
    } catch (err) {
      log.error('Failed to load plays', err);
    }
  }

  /** Invalidate any load issued against a database this store no longer represents. */
  invalidateForDatabaseSwitch(): void {
    this.#generation++;
  }

  /** Reset to empty (test use only). */
  reset(): void {
    this.#generation++;
    this.#loaded = [];
  }
}

export const playsData = new PlaysStore();

onDaemonReconnect(() => playsData.loadPlays());
