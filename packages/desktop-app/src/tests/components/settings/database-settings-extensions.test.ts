/**
 * DatabaseSettings with contributed slot content.
 *
 * Uses the fixture extension, so it exercises the generic host only:
 * `database.actions` renders once in the Databases header while its `when()`
 * holds, `database.row` renders once per database with that row's id, and a
 * throwing row component takes down only its own outlet.
 *
 * No Tauri bridge is mocked: under plain Happy-DOM `databaseStore.load()` takes
 * its no-bridge branch and yields one implicit database, which the tests copy to
 * get more rows.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, waitFor, within } from '@testing-library/svelte';

const log = vi.hoisted(() => ({
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn()
}));

vi.mock('$lib/utils/logger', () => ({ createLogger: () => log }));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn()
}));

import DatabaseSettings from '$lib/components/settings/sections/database-settings.svelte';
import { databaseStore } from '$lib/stores/database.svelte';
import { uiExtensionRegistry } from '$lib/plugins/ui-extensions';
import {
  TEST_EXTENSION_ID,
  createTestExtension,
  resetTestExtension,
  testExtensionFlags,
  testExtensionMounts
} from '../../fixtures/test-extension';

/** Render the page and grow the store to two databases: `default` and `second`. */
async function renderWithTwoDatabases() {
  const view = render(DatabaseSettings);
  await view.findByText('Default');
  databaseStore.databases = [
    ...databaseStore.databases,
    { ...databaseStore.databases[0], id: 'second', name: 'Second', isDefault: false }
  ];
  await view.findByText('Second');
  return view;
}

/** The buttons in the Databases header: the group right of the `Databases` heading. */
function headerButtons(container: HTMLElement): string[] {
  const heading = within(container).getByRole('heading', { name: 'Databases' });
  const group = heading.nextElementSibling as HTMLElement;
  return within(group)
    .getAllByRole('button')
    .map((b) => b.textContent?.trim() ?? '');
}

describe('DatabaseSettings extension slots', () => {
  beforeEach(() => {
    log.error.mockClear();
  });

  afterEach(() => {
    cleanup();
    uiExtensionRegistry.unregister(TEST_EXTENSION_ID);
    resetTestExtension();
    databaseStore.databases = [];
  });

  it('renders nothing extra while nothing is registered', async () => {
    const view = await renderWithTwoDatabases();

    expect(headerButtons(view.container)).toEqual(['New', 'Open existing…']);
    expect(view.queryByTestId('test-database-action')).toBeNull();
    expect(view.queryByTestId('test-database-row')).toBeNull();
  });

  it('renders nothing extra while the contributions’ when() is false', async () => {
    uiExtensionRegistry.register(createTestExtension());
    const view = await renderWithTwoDatabases();

    expect(headerButtons(view.container)).toEqual(['New', 'Open existing…']);
    expect(view.queryByTestId('test-database-action')).toBeNull();
    expect(view.queryByTestId('test-database-row')).toBeNull();
  });

  describe('database.actions', () => {
    it('renders once in the header, beside New and Open existing, however many databases there are', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.databaseActions = true;
      const view = await renderWithTwoDatabases();

      expect(await view.findAllByTestId('test-database-action')).toHaveLength(1);
      expect(headerButtons(view.container)).toEqual(['New', 'Open existing…', 'Fixture action']);
      expect(testExtensionMounts['database-action']).toBe(1);
    });

    it('appears and disappears with its when()', async () => {
      uiExtensionRegistry.register(createTestExtension());
      const view = await renderWithTwoDatabases();
      expect(view.queryByTestId('test-database-action')).toBeNull();

      testExtensionFlags.databaseActions = true;
      expect(await view.findByTestId('test-database-action')).toBeTruthy();

      testExtensionFlags.databaseActions = false;
      await waitFor(() => expect(view.queryByTestId('test-database-action')).toBeNull());
      expect(headerButtons(view.container)).toEqual(['New', 'Open existing…']);
    });
  });

  describe('database.row', () => {
    it('renders once per database with that row’s id', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.databaseRow = true;
      const view = await renderWithTwoDatabases();

      const rows = await view.findAllByTestId('test-database-row');
      expect(rows.map((r) => r.textContent)).toEqual(['row:default', 'row:second']);
      expect(rows.map((r) => r.getAttribute('data-database-id'))).toEqual(['default', 'second']);
      expect(testExtensionMounts['database-row']).toBe(2);
    });

    it('sits inside its own database’s row, in the left column', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.databaseRow = true;
      const view = await renderWithTwoDatabases();

      const [first, second] = await view.findAllByTestId('test-database-row');
      const firstColumn = first.closest('.min-w-0.flex-1') as HTMLElement;
      const secondColumn = second.closest('.min-w-0.flex-1') as HTMLElement;

      expect(firstColumn).not.toBe(secondColumn);
      expect(within(firstColumn).getByText('Default')).toBeTruthy();
      expect(within(firstColumn).queryByText('Second')).toBeNull();
      expect(within(secondColumn).getByText('Second')).toBeTruthy();
    });

    it('appears and disappears with its when(), leaving the rows themselves alone', async () => {
      uiExtensionRegistry.register(createTestExtension());
      const view = await renderWithTwoDatabases();
      expect(view.queryByTestId('test-database-row')).toBeNull();

      testExtensionFlags.databaseRow = true;
      expect(await view.findAllByTestId('test-database-row')).toHaveLength(2);

      testExtensionFlags.databaseRow = false;
      await waitFor(() => expect(view.queryByTestId('test-database-row')).toBeNull());
      expect(view.getByText('Default')).toBeTruthy();
      expect(view.getByText('Second')).toBeTruthy();
    });

    it('follows the list: a database added later gets its own row content', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.databaseRow = true;
      const view = await renderWithTwoDatabases();
      await view.findAllByTestId('test-database-row');

      databaseStore.databases = [
        ...databaseStore.databases,
        { ...databaseStore.databases[0], id: 'third', name: 'Third', isDefault: false }
      ];

      await waitFor(() => expect(view.getAllByTestId('test-database-row')).toHaveLength(3));
      expect(view.getByText('row:third')).toBeTruthy();
    });

    it('a throwing row component removes only its own outlet; the other rows and contributions still render', async () => {
      uiExtensionRegistry.register(createTestExtension());
      testExtensionFlags.databaseRow = true;
      testExtensionFlags.databaseRowThrowing = true;
      const view = await renderWithTwoDatabases();

      await waitFor(() =>
        expect(log.error).toHaveBeenCalledWith(expect.stringContaining('threw'), expect.anything())
      );

      // Both rows are listed with their controls, and the healthy contribution rendered in each.
      expect(view.getByText('Default')).toBeTruthy();
      expect(view.getByText('Second')).toBeTruthy();
      expect(view.getAllByRole('button', { name: 'Remove' })).toHaveLength(2);
      expect((await view.findAllByTestId('test-database-row')).map((r) => r.textContent)).toEqual([
        'row:default',
        'row:second'
      ]);
      expect(view.queryByTestId('test-throwing-row')).toBeNull();
    });
  });
});
