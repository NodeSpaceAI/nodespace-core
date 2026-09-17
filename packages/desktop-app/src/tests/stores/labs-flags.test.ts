/**
 * Unit tests for the Labs flags store — a minimal, localStorage-backed,
 * per-device visibility gate for experimental surfaces (AI Chat, and the
 * companion "Team synchronization" toggle). No daemon round-trip: reading
 * and writing are both synchronous against localStorage.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

const STORAGE_KEY = 'nodespace-labs-flags';

describe('Labs flags store', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.resetModules();
  });

  afterEach(() => {
    localStorage.clear();
  });

  it('defaults both flags to false on a fresh profile (no localStorage entry)', async () => {
    const { labsFlags } = await import('$lib/stores/labs-flags.svelte');

    expect(labsFlags.aiChatEnabled).toBe(false);
    expect(labsFlags.syncEnabled).toBe(false);
  });

  it('setting aiChatEnabled updates state and persists under the dedicated key', async () => {
    const { labsFlags } = await import('$lib/stores/labs-flags.svelte');

    labsFlags.aiChatEnabled = true;

    expect(labsFlags.aiChatEnabled).toBe(true);
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
    expect(stored.aiChatEnabled).toBe(true);
  });

  it('setting one flag does not clobber the other in storage', async () => {
    const { labsFlags } = await import('$lib/stores/labs-flags.svelte');

    labsFlags.aiChatEnabled = true;
    labsFlags.syncEnabled = true;

    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
    expect(stored).toEqual({ aiChatEnabled: true, syncEnabled: true });

    labsFlags.aiChatEnabled = false;
    const storedAfter = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}');
    expect(storedAfter).toEqual({ aiChatEnabled: false, syncEnabled: true });
  });

  it('a fresh module load picks up a previously persisted true value (survives reload/restart)', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ aiChatEnabled: true, syncEnabled: false }));

    const { labsFlags } = await import('$lib/stores/labs-flags.svelte');

    expect(labsFlags.aiChatEnabled).toBe(true);
    expect(labsFlags.syncEnabled).toBe(false);
  });

  it('falls back to defaults when localStorage holds corrupt JSON', async () => {
    localStorage.setItem(STORAGE_KEY, '{not valid json');

    const { labsFlags } = await import('$lib/stores/labs-flags.svelte');

    expect(labsFlags.aiChatEnabled).toBe(false);
    expect(labsFlags.syncEnabled).toBe(false);
  });

  it('does not collide with the existing settings.svelte.ts localStorage key', async () => {
    const { labsFlags } = await import('$lib/stores/labs-flags.svelte');
    labsFlags.aiChatEnabled = true;

    // The Settings store persists under 'nodespace-settings' — a distinct key.
    expect(localStorage.getItem('nodespace-settings')).toBeNull();
    expect(localStorage.getItem(STORAGE_KEY)).not.toBeNull();
  });
});
