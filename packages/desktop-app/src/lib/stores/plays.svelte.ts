/**
 * Plays Store
 *
 * The plays listed in the "Plays" section of the navigation sidebar, each with
 * the state its row shows (ADR-090). The section only lists and opens plays:
 * this store never changes one.
 *
 * It holds which plays are listed, not the plays: each row is read from
 * `SharedNodeStore`, the same node the play's viewer shows and writes
 * (ADR-049). So a switch flipped in the viewer shows on the row at once, and a
 * node event that updates the play there updates the row with it.
 *
 * Follows the `savedQueriesData` pattern otherwise: a `load*` method called
 * from the sidebar's `onMount`, refreshed by the debounced `schedulePlayRefresh`
 * wired to node domain events, reloaded after a daemon reconnect, and guarded
 * by a database-switch generation counter.
 */

import { backendAdapter } from '$lib/services/backend-adapter';
import {
  sharedNodeStore,
  SimplePersistenceCoordinator
} from '$lib/services/shared-node-store.svelte';
import { createLogger } from '$lib/utils/logger';
import { onDaemonReconnect } from '$lib/services/daemon-status';
import {
  asPlayNode,
  playState,
  playTitle,
  type PlayState
} from '$lib/components/play/play-node-model';
import type { PlayNode } from '$lib/types';

const log = createLogger('PlaysStore');

/** The owner the listed plays are pinned under in `SharedNodeStore`. */
const PIN_OWNER_ID = 'plays-navigation-section';

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
  /** Ids of the plays the last load returned. */
  #ids = $state<string[]>([]);

  /** The listed plays, ordered by title. A play deleted from the node store drops out at once. */
  plays = $derived.by(() =>
    this.#ids
      .flatMap((id) => {
        const play = asPlayNode(sharedNodeStore.getNode(id));
        return play ? [toListItem(play)] : [];
      })
      .sort(comparePlays)
  );

  /**
   * Bumped by every load and by a database switch. A load applies its result
   * only while it is still the latest, so neither an older load that resolves
   * late nor one issued against the previous database changes the list.
   */
  #generation = 0;

  /** True when `nodeId` is a listed play (used to react to its deletion). */
  has(nodeId: string): boolean {
    return this.#ids.includes(nodeId);
  }

  /**
   * Load every participating play, core and user-written. The query leaves out
   * archived plays (the participation rule, ADR-087), so an archived play
   * drops off the list on the load that follows its archiving.
   */
  async loadPlays(): Promise<void> {
    const generation = ++this.#generation;
    try {
      const nodes = await backendAdapter.queryNodes({ nodeType: 'play' });
      if (generation !== this.#generation) {
        log.debug('Discarding plays load that resolved after the store moved on');
        return;
      }
      const coordinator = SimplePersistenceCoordinator.getInstance();
      for (const node of nodes) {
        // A copy the node store already holds is the one a viewer writes and
        // node events keep current, so only a newer version replaces it: a
        // load must not put back what a switch just changed. While that
        // switch's write is in flight the load leaves the play alone: the
        // newer version it may carry is the write itself, which its own
        // response applies.
        const stored = sharedNodeStore.getNode(node.id);
        if (stored && coordinator.hasPending(node.id)) continue;
        if (!stored || node.version > stored.version) {
          sharedNodeStore.setNode(node, { type: 'database', reason: 'plays-list load' });
        }
      }
      this.#ids = nodes.map((node) => node.id);
      // The rows show these plays outside any open document, so the node store
      // must keep them while they are listed.
      sharedNodeStore.pinNodes(PIN_OWNER_ID, this.#ids);
      log.debug('Plays loaded', { count: nodes.length });
    } catch (err) {
      log.error('Failed to load plays', err);
    }
  }

  /**
   * Invalidate any load issued against a database this store no longer
   * represents, and stop listing that database's plays.
   */
  invalidateForDatabaseSwitch(): void {
    this.#generation++;
    this.#ids = [];
    sharedNodeStore.unpinAll(PIN_OWNER_ID);
  }

  /** Reset to empty (test use only). */
  reset(): void {
    this.invalidateForDatabaseSwitch();
  }
}

export const playsData = new PlaysStore();

onDaemonReconnect(() => playsData.loadPlays());
