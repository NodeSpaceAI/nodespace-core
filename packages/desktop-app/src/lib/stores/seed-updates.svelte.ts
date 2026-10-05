import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('SeedUpdates');

/** The part of a built-in item an update is about: its name and fields, or its body. */
export type SeedAspect = 'config' | 'guidance';

/**
 * A shipped change to a built-in item the user has edited (ADR-094 §8). The
 * user's version stays in place until they keep it or take the shipped one.
 */
export interface PendingSeedUpdate {
  nodeId: string;
  /** The item's node type: the kind of item. */
  nodeType: string;
  title: string;
  aspect: SeedAspect;
  shippedVersion: string;
  recordedAt: string;
  lastEditedAt: string;
}

/** Both versions of one pending aspect, as text. */
export interface PendingSeedUpdateDetail {
  shipped: string;
  yours: string;
}

type SeedUpdateChoice = 'keep_mine' | 'take_shipped';

/** What each seeded kind is called on the review screen. */
const SEED_KIND_LABELS: Record<string, string> = {
  skill: 'Skill',
  play: 'Play',
  query: 'Saved query',
  'agent-guidance': 'Agent guidance',
  'tool-native': 'Tool'
};

/** A display name for a seeded item's kind; the type id itself for a kind not listed. */
export function seedKindLabel(nodeType: string): string {
  return SEED_KIND_LABELS[nodeType] ?? nodeType;
}

/** Identifies one pending update: a built-in item has at most one per aspect. */
export function seedUpdateKey(update: Pick<PendingSeedUpdate, 'nodeId' | 'aspect'>): string {
  return `${update.nodeId}/${update.aspect}`;
}

/**
 * The pending seed updates of the active database. `$state` fields on a plain
 * class, methods that read state directly, no `$effect` (ADR-049).
 *
 * Nothing here applies a shipped version on its own: `takeShipped` is called
 * only from the review screen's confirmed action.
 */
class SeedUpdatesStore {
  updates = $state<PendingSeedUpdate[]>([]);
  loaded = $state(false);
  /**
   * Whether this database has had anything to review since it became active.
   * The review screen stays listed after its last item is settled, so settling
   * it does not close the screen under the user.
   */
  hadUpdates = $state(false);

  /**
   * Bumped by `invalidateForDatabaseSwitch()`. A read or a choice issued
   * against the previous database is dropped when it resolves, so it cannot
   * write that database's updates into a store that now shows another's.
   */
  #generation = 0;

  /**
   * Load what is pending. Resolves `true` when the result was applied, `false`
   * when a database switch landed while it was in flight.
   */
  async load(): Promise<boolean> {
    const generation = this.#generation;
    let updates: PendingSeedUpdate[];
    try {
      updates = (await invoke<PendingSeedUpdate[]>('list_pending_seed_updates')) ?? [];
    } catch (e) {
      log.warn('Failed to load pending seed updates', { error: e });
      updates = [];
    }
    if (generation !== this.#generation) return false;
    this.updates = updates;
    this.loaded = true;
    if (updates.length > 0) this.hadUpdates = true;
    return true;
  }

  /** Forget the previous database's updates and any load in flight against it. */
  invalidateForDatabaseSwitch(): void {
    this.#generation++;
    this.updates = [];
    this.loaded = false;
    this.hadUpdates = false;
  }

  /** The shipped version and the user's version of one pending update. */
  async detail(update: PendingSeedUpdate): Promise<PendingSeedUpdateDetail> {
    return invoke<PendingSeedUpdateDetail>('get_pending_seed_update', {
      nodeId: update.nodeId,
      aspect: update.aspect
    });
  }

  /** Keep the user's version. It is not listed again until what ships changes. */
  async keepMine(update: PendingSeedUpdate): Promise<void> {
    await this.resolve(update, 'keep_mine');
  }

  /**
   * Replace the user's version of this aspect with the shipped one.
   * **User-initiated only**: it discards an edit, so call it from an explicit,
   * confirmed action and nowhere else.
   */
  async takeShipped(update: PendingSeedUpdate): Promise<void> {
    await this.resolve(update, 'take_shipped');
  }

  private async resolve(update: PendingSeedUpdate, choice: SeedUpdateChoice): Promise<void> {
    const generation = this.#generation;
    try {
      await invoke('resolve_pending_seed_update', {
        nodeId: update.nodeId,
        aspect: update.aspect,
        choice
      });
    } catch (e) {
      log.warn('Failed to settle a pending seed update', { error: e, update, choice });
      throw e;
    }
    if (generation !== this.#generation) return;
    const settled = seedUpdateKey(update);
    this.updates = this.updates.filter((u) => seedUpdateKey(u) !== settled);
  }
}

export const seedUpdatesStore = new SeedUpdatesStore();
