/**
 * Pending seed updates (ADR-094 §8): the store, and the Settings page where a
 * user compares the shipped version of a built-in item with their own and
 * chooses. The page applies nothing without an explicit action, and taking
 * the shipped version needs a confirmation.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

import BuiltInUpdatesSettings from '$lib/components/settings/sections/built-in-updates-settings.svelte';
import {
  seedKindLabel,
  seedUpdatesStore,
  type PendingSeedUpdate
} from '$lib/stores/seed-updates.svelte';

function update(overrides: Partial<PendingSeedUpdate> = {}): PendingSeedUpdate {
  return {
    nodeId: 'skill-1',
    nodeType: 'skill',
    title: 'Research & Search',
    aspect: 'guidance',
    shippedVersion: 'abc',
    recordedAt: '2026-10-01T00:00:00Z',
    lastEditedAt: '2026-09-20T12:00:00Z',
    shippedAvailable: true,
    ...overrides
  };
}

/** Commands the page or store issued, without their arguments. */
const commands = () => mockInvoke.mock.calls.map((call) => call[0] as string);

function button(container: HTMLElement, label: string) {
  const found = Array.from(container.ownerDocument.querySelectorAll('button')).find(
    (b) => b.textContent?.trim() === label
  );
  if (!found) throw new Error(`No "${label}" button rendered`);
  return found;
}

beforeEach(() => {
  mockInvoke.mockReset();
  seedUpdatesStore.invalidateForDatabaseSwitch();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('seed updates store', () => {
  it('loads what is pending and remembers the database had some', async () => {
    mockInvoke.mockResolvedValueOnce([update()]);

    expect(await seedUpdatesStore.load()).toBe(true);

    expect(mockInvoke).toHaveBeenCalledWith('list_pending_seed_updates');
    expect(seedUpdatesStore.updates).toEqual([update()]);
    expect(seedUpdatesStore.loaded).toBe(true);
    expect(seedUpdatesStore.hadUpdates).toBe(true);
  });

  it('leaves the store as it was when the daemon cannot be asked', async () => {
    seedUpdatesStore.updates = [update()];
    mockInvoke.mockRejectedValueOnce(new Error('no daemon'));

    expect(await seedUpdatesStore.load()).toBe(false);

    // Not "nothing to review": the read never answered.
    expect(seedUpdatesStore.updates).toEqual([update()]);
    expect(seedUpdatesStore.loaded).toBe(false);
  });

  it('drops a load that resolves after a database switch', async () => {
    let resolve: (value: PendingSeedUpdate[]) => void = () => {};
    mockInvoke.mockReturnValueOnce(new Promise((r) => (resolve = r)));

    const loading = seedUpdatesStore.load();
    seedUpdatesStore.invalidateForDatabaseSwitch();
    resolve([update()]);

    expect(await loading).toBe(false);
    expect(seedUpdatesStore.updates).toEqual([]);
    expect(seedUpdatesStore.hadUpdates).toBe(false);
  });

  it('settles one aspect and leaves the item’s other aspect pending', async () => {
    seedUpdatesStore.updates = [update(), update({ aspect: 'config' })];
    mockInvoke.mockResolvedValueOnce(undefined);

    await seedUpdatesStore.keepMine(update());

    expect(mockInvoke).toHaveBeenCalledWith('resolve_pending_seed_update', {
      nodeId: 'skill-1',
      aspect: 'guidance',
      choice: 'keep_mine'
    });
    expect(seedUpdatesStore.updates).toEqual([update({ aspect: 'config' })]);
  });

  it('keeps an update listed when settling it fails', async () => {
    seedUpdatesStore.updates = [update()];
    mockInvoke.mockImplementation((command: string) =>
      command === 'list_pending_seed_updates'
        ? Promise.resolve([update()])
        : Promise.reject(new Error('refused'))
    );

    await expect(seedUpdatesStore.takeShipped(update())).rejects.toThrow('refused');

    expect(seedUpdatesStore.updates).toEqual([update()]);
  });

  it('re-reads the list after a failed choice, dropping what was settled elsewhere', async () => {
    seedUpdatesStore.updates = [update()];
    mockInvoke.mockImplementation((command: string) =>
      command === 'list_pending_seed_updates'
        ? Promise.resolve([])
        : Promise.reject(new Error('nothing pending'))
    );

    await expect(seedUpdatesStore.keepMine(update())).rejects.toThrow('nothing pending');

    await vi.waitFor(() => expect(seedUpdatesStore.updates).toEqual([]));
  });

  it('names each seeded kind, and falls back to the type id', () => {
    expect(seedKindLabel('query')).toBe('Saved query');
    expect(seedKindLabel('play')).toBe('Play');
    expect(seedKindLabel('custom-kind')).toBe('custom-kind');
  });
});

describe('Built-in updates page', () => {
  function serve(pending: PendingSeedUpdate[]) {
    mockInvoke.mockImplementation((command: string) => {
      if (command === 'list_pending_seed_updates') return Promise.resolve(pending);
      if (command === 'get_pending_seed_update') {
        return Promise.resolve({ shipped: 'Search first.', yours: 'Search last.' });
      }
      return Promise.resolve();
    });
  }

  it('lists each pending item with its kind and part, and applies nothing', async () => {
    serve([update(), update({ nodeId: 'play-1', nodeType: 'play', title: 'Roll up', aspect: 'config' })]);

    const { container, findByText } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    const rows = Array.from(container.querySelectorAll('[data-testid="seed-update"]')).map(
      (row) => row.textContent ?? ''
    );
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain('Skill');
    expect(rows[0]).toContain('Body');
    expect(rows[1]).toContain('Roll up');
    expect(rows[1]).toContain('Play');
    expect(rows[1]).toContain('Name and settings');
    // Opening the page reads; it never chooses.
    expect(commands()).toEqual(['list_pending_seed_updates']);
  });

  it('shows the shipped version beside the user’s on Compare', async () => {
    serve([update()]);
    const { container, findByText } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    await fireEvent.click(button(container, 'Compare'));

    await waitFor(() => {
      expect(container.querySelector('[data-testid="seed-update-shipped"]')?.textContent).toBe(
        'Search first.'
      );
    });
    expect(container.querySelector('[data-testid="seed-update-yours"]')?.textContent).toBe(
      'Search last.'
    );
    expect(mockInvoke).toHaveBeenCalledWith('get_pending_seed_update', {
      nodeId: 'skill-1',
      aspect: 'guidance'
    });
    expect(commands()).not.toContain('resolve_pending_seed_update');
  });

  it('keeps the user’s version on Keep mine and removes the row', async () => {
    serve([update()]);
    const { container, findByText } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    await fireEvent.click(button(container, 'Keep mine'));

    await findByText(/Nothing to review/);
    expect(mockInvoke).toHaveBeenCalledWith('resolve_pending_seed_update', {
      nodeId: 'skill-1',
      aspect: 'guidance',
      choice: 'keep_mine'
    });
  });

  it('takes the shipped version only after the confirmation', async () => {
    serve([update()]);
    const { container, findByText } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    await fireEvent.click(button(container, 'Take shipped…'));
    await findByText('Take the shipped version');
    expect(commands()).not.toContain('resolve_pending_seed_update');

    await fireEvent.click(button(container, 'Replace mine'));

    await findByText(/Nothing to review/);
    expect(mockInvoke).toHaveBeenCalledWith('resolve_pending_seed_update', {
      nodeId: 'skill-1',
      aspect: 'guidance',
      choice: 'take_shipped'
    });
  });

  it('takes nothing when the confirmation is cancelled', async () => {
    serve([update()]);
    const { container, findByText } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    await fireEvent.click(button(container, 'Take shipped…'));
    await findByText('Take the shipped version');
    await fireEvent.click(button(container, 'Cancel'));

    expect(commands()).not.toContain('resolve_pending_seed_update');
    expect(seedUpdatesStore.updates).toHaveLength(1);
  });

  it('offers only Keep mine for an item this build has no shipped version of', async () => {
    serve([update({ shippedAvailable: false })]);
    const { container, findByText } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    expect(button(container, 'Compare').disabled).toBe(true);
    expect(button(container, 'Take shipped…').disabled).toBe(true);
    expect(button(container, 'Keep mine').disabled).toBe(false);
    expect(container.textContent).toContain('it can only be');
  });

  it('reports a choice the daemon refused and keeps the row', async () => {
    mockInvoke.mockImplementation((command: string) => {
      if (command === 'list_pending_seed_updates') return Promise.resolve([update()]);
      return Promise.reject(new Error('this build ships no seed for that node'));
    });
    const { container, findByText, findByRole } = render(BuiltInUpdatesSettings);
    await findByText('Research & Search');

    await fireEvent.click(button(container, 'Keep mine'));

    const alert = await findByRole('alert');
    expect(alert.textContent).toContain('this build ships no seed for that node');
    expect(container.querySelectorAll('[data-testid="seed-update"]')).toHaveLength(1);
  });
});
