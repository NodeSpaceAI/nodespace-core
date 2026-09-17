import { createLogger } from '$lib/utils/logger';

const log = createLogger('LabsFlagsStore');

const LOCAL_STORAGE_KEY = 'nodespace-labs-flags';

/**
 * Labs flags gate experimental surfaces that are wired up but not yet ready
 * for ordinary users — a pure per-device UI-visibility switch, not synced
 * data and not a security boundary. Both flags default to `false`: a fresh
 * install hides these surfaces until a user opts in via Settings → Labs.
 */
export interface LabsFlags {
  aiChatEnabled: boolean;
  /** Owned/consumed by the companion "Team synchronization" Labs toggle. */
  syncEnabled: boolean;
}

const DEFAULT_LABS_FLAGS: LabsFlags = {
  aiChatEnabled: false,
  syncEnabled: false,
};

function readLocalFlags(): Partial<LabsFlags> {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(LOCAL_STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as Partial<LabsFlags>;
  } catch {
    return {};
  }
}

function writeLocalFlags(patch: Partial<LabsFlags>): void {
  if (typeof localStorage === 'undefined') return;
  try {
    const existing = readLocalFlags();
    localStorage.setItem(LOCAL_STORAGE_KEY, JSON.stringify({ ...existing, ...patch }));
  } catch (err) {
    log.warn('Failed to persist labs flags to localStorage', err);
  }
}

class LabsFlagsStore {
  flags = $state<LabsFlags>({ ...DEFAULT_LABS_FLAGS, ...readLocalFlags() });

  get aiChatEnabled(): boolean {
    return this.flags.aiChatEnabled;
  }

  set aiChatEnabled(value: boolean) {
    this.flags.aiChatEnabled = value;
    writeLocalFlags({ aiChatEnabled: value });
  }

  get syncEnabled(): boolean {
    return this.flags.syncEnabled;
  }

  set syncEnabled(value: boolean) {
    this.flags.syncEnabled = value;
    writeLocalFlags({ syncEnabled: value });
  }
}

export const labsFlags = new LabsFlagsStore();
