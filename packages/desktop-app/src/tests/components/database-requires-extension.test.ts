/**
 * DatabaseRequiresExtension — the view the app shows in place of the
 * workspace while the daemon refuses the active database because it requires
 * an extension this build does not support (ADR-083 §2).
 *
 * It renders the refusal message and the download link from the
 * REQUIRES_EXTENSION payload verbatim, offers a switch to another registered
 * database or a new one, and nothing that would touch the refused file.
 *
 * The fixture payloads use neutral text: the view must never spell a product
 * name or URL of its own, so these tests only ever see what the payload says.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, cleanup, fireEvent, screen, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const openUrl = vi.fn((..._a: unknown[]) => Promise.resolve());
vi.mock('$lib/utils/external-links', () => ({
  openUrl: (...a: unknown[]) => openUrl(...a)
}));

import DatabaseRequiresExtension from '$lib/components/database-requires-extension.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import type { RequiresExtensionPayload } from '$lib/types/requires-extension';

/** A refusal for an extension id the app binary has a display name for. */
const NAMED_EXTENSION: RequiresExtensionPayload = {
  unsupportedExtensions: ['sync'],
  message: 'This database needs Fixture App',
  downloadLabel: 'Download Fixture App',
  downloadUrl: 'https://example.test/fixture-app'
};

/** A refusal for an extension id the app binary does not know. */
const UNKNOWN_EXTENSION: RequiresExtensionPayload = {
  unsupportedExtensions: ['fixture-ext'],
  message: "This database needs an extension this app doesn't support (fixture-ext)",
  downloadLabel: 'Download Fixture App',
  downloadUrl: 'https://example.test/fixture-app'
};

function db(id: string, name: string, overrides: Partial<DatabaseInfo> = {}): DatabaseInfo {
  return {
    id,
    name,
    path: `/tmp/${id}.db`,
    isDefault: false,
    status: 'closed',
    createdAt: '',
    lastOpenedAt: null,
    extensions: {},
    ...overrides
  };
}

describe('DatabaseRequiresExtension', () => {
  beforeEach(() => {
    openUrl.mockClear();
    databaseStore.databases = [
      // Refused on its first read: the listing need not mark it.
      db('refused', 'Shared notes', { isDefault: true }),
      db('work', 'Work'),
      db('other-refused', 'Archive', { status: 'requires_extension' })
    ];
    databaseStore.activeDatabaseId = 'refused';
    databaseStore.error = null;
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
  });

  it.each([
    ['an extension with a display name', NAMED_EXTENSION],
    ['an unknown extension', UNKNOWN_EXTENSION]
  ])('renders the payload message and download label verbatim for %s', (_case, refusal) => {
    render(DatabaseRequiresExtension, { refusal });

    const heading = screen.getByRole('heading', { level: 1 });
    expect(heading.textContent?.trim()).toBe(refusal.message);
    expect(screen.getByRole('button', { name: refusal.downloadLabel })).toBeTruthy();
    // The refused database is named, so the user knows which one this is.
    expect(screen.getByText('Shared notes')).toBeTruthy();
  });

  it('opens the payload download URL through openUrl', async () => {
    render(DatabaseRequiresExtension, { refusal: NAMED_EXTENSION });

    await fireEvent.click(screen.getByRole('button', { name: NAMED_EXTENSION.downloadLabel }));

    expect(openUrl).toHaveBeenCalledOnce();
    expect(openUrl).toHaveBeenCalledWith(NAMED_EXTENSION.downloadUrl);
  });

  it('switches to another registered database, and offers none the listing marks refused', async () => {
    const switchTo = vi.spyOn(databaseStore, 'switchTo').mockResolvedValue();
    render(DatabaseRequiresExtension, { refusal: NAMED_EXTENSION });

    // Neither the refused database itself nor another refused one is offered.
    expect(screen.queryByRole('button', { name: 'Open Shared notes' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Open Archive' })).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Open Work' }));

    expect(switchTo).toHaveBeenCalledOnce();
    expect(switchTo).toHaveBeenCalledWith('work');
  });

  it('creates a new database through the create dialog, then switches to it', async () => {
    const created = db('new', 'Fresh');
    const create = vi.spyOn(databaseStore, 'create').mockResolvedValue(created);
    const switchTo = vi.spyOn(databaseStore, 'switchTo').mockResolvedValue();
    render(DatabaseRequiresExtension, { refusal: NAMED_EXTENSION });

    await fireEvent.click(screen.getByRole('button', { name: 'New database…' }));
    const input = await waitFor(() => {
      const el = document.getElementById('database-name-input');
      if (!(el instanceof HTMLInputElement)) throw new Error('create dialog not open');
      return el;
    });
    expect(screen.getByText('New Database')).toBeTruthy();

    await fireEvent.input(input, { target: { value: 'Fresh' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => expect(switchTo).toHaveBeenCalledWith('new'));
    expect(create).toHaveBeenCalledWith('Fresh');
  });

  it('shows a switch or create failure the store records', () => {
    databaseStore.error = 'Could not open Work';
    render(DatabaseRequiresExtension, { refusal: NAMED_EXTENSION });

    expect(screen.getByRole('alert').textContent).toContain('Could not open Work');
  });

  it('offers no contact link and nothing that would move, rename, reset or remove the file', async () => {
    const { container } = render(DatabaseRequiresExtension, { refusal: NAMED_EXTENSION });
    // Open the create dialog too, so its controls are covered.
    await fireEvent.click(screen.getByRole('button', { name: 'New database…' }));

    for (const link of Array.from(document.querySelectorAll('a'))) {
      expect(link.getAttribute('href') ?? '').not.toMatch(/^mailto:/i);
    }
    expect(container.textContent).not.toMatch(/contact|@/i);

    const actions = Array.from(document.querySelectorAll('button')).map(
      (button) => button.textContent?.trim() ?? ''
    );
    expect(actions.length).toBeGreaterThan(0);
    for (const action of actions) {
      expect(action).not.toMatch(/move|aside|rename|reset|remove|delete|start fresh|contact/i);
    }
    // The only actions: download, open another database, create a new one, and
    // the create dialog's own controls.
    expect(actions).toEqual(
      expect.arrayContaining([NAMED_EXTENSION.downloadLabel, 'Open', 'New database…'])
    );
  });
});
