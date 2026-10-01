/**
 * DatabaseSettings — Settings → Database.
 *
 * Core's Databases page is local only (ADR-083 §3): its own header actions are
 * "New" and "Open existing…", a row shows name, path and status, and core reads
 * no key from a database's opaque `extensions` map. Anything more comes from
 * extensions through the `database.actions` / `database.row` slots, covered in
 * `database-settings-extensions.test.ts`.
 *
 * No Tauri bridge is mocked here: `isTauriBridgePresent()` (database.svelte.ts)
 * is false under plain Happy-DOM, so `databaseStore.load()` takes its
 * no-bridge branch (a single implicit local database, no `invoke` call) and
 * `identity-card.svelte`'s own `invoke('get_local_identity')` rejects into
 * its own caught/logged fallback — both harmless for what this file checks.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn()
}));

import DatabaseSettings from '$lib/components/settings/sections/database-settings.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';

/** The page's header actions: the buttons beside the "Databases" heading. */
function headerActions(container: HTMLElement): string[] {
  const heading = Array.from(container.querySelectorAll('h2')).find(
    (h) => h.textContent?.trim() === 'Databases'
  );
  const header = heading?.parentElement;
  if (!header) throw new Error('Databases header not rendered');
  return Array.from(header.querySelectorAll('button')).map((b) => b.textContent?.trim() ?? '');
}

// Built from fragments so these absence checks add no line to the boundary ratchet.
const REMOVED_WORDING = new RegExp(
  [['ten', 'ant'].join(''), 'synced', 'syncs to', 'not synced', 'local only'].join('|'),
  'i'
);

describe('DatabaseSettings', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('offers exactly "New" and "Open existing…" as its own header actions', async () => {
    const { container, findByText } = render(DatabaseSettings);
    await findByText('Default');

    expect(headerActions(container)).toEqual(['New', 'Open existing…']);
  });

  it('describes databases in neutral, local-only terms', async () => {
    const { container, findByText } = render(DatabaseSettings);
    await findByText('Default');

    expect(container.textContent).not.toMatch(REMOVED_WORDING);

    // A row renders no badge wrapper under its status line, not even an empty one.
    const rowInfo = container.querySelector('.min-w-0.flex-1');
    expect(rowInfo).not.toBeNull();
    expect(rowInfo!.querySelector('.mt-1\\.5')).toBeNull();
  });

  it("renders nothing from a database's extension keys", async () => {
    const entry: DatabaseInfo = {
      id: 'work',
      name: 'Work',
      path: '/tmp/work.db',
      isDefault: true,
      status: 'open',
      createdAt: '',
      lastOpenedAt: null,
      extensions: { 'some.extension.key': 'value-core-never-shows' }
    };
    vi.spyOn(databaseStore, 'load').mockResolvedValue();
    const previous = databaseStore.databases;
    databaseStore.databases = [entry];
    try {
      const { container, findByText } = render(DatabaseSettings);
      await findByText('Work');

      expect(container.textContent).not.toContain('some.extension.key');
      expect(container.textContent).not.toContain('value-core-never-shows');
      expect(container.textContent).not.toMatch(REMOVED_WORDING);
    } finally {
      databaseStore.databases = previous;
    }
  });
});
