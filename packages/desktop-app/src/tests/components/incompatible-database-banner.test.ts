/**
 * IncompatibleDatabaseBanner — the explanation and way out shown when the
 * daemon refused a database another version of NodeSpace created. Moving the
 * database aside must take an explicit confirm, and a refusal from the backend
 * must be shown rather than swallowed.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const getIncompatibleDatabase = vi.fn();
const resetIncompatibleDatabase = vi.fn();
vi.mock('$lib/services/daemon-status', () => ({
  getIncompatibleDatabase: () => getIncompatibleDatabase(),
  resetIncompatibleDatabase: () => resetIncompatibleDatabase()
}));

import IncompatibleDatabaseBanner from '$lib/components/layout/incompatible-database-banner.svelte';

const record = {
  databasePath: '/Users/u/.nodespace/database/nodespace.db',
  detail: 'relationship: missing reverse_relationship_type',
  detectedAt: '2026-09-28T10:00:00Z'
};

describe('IncompatibleDatabaseBanner', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getIncompatibleDatabase.mockResolvedValue(record);
  });

  it('explains the problem and names the refused database', async () => {
    render(IncompatibleDatabaseBanner);

    expect(screen.getByRole('alert').textContent).toContain(
      "This database was created by a different version of NodeSpace and can't be opened."
    );
    expect(await screen.findByText(record.databasePath)).toBeTruthy();
    // No generic Retry: retrying cannot fix an incompatible database.
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('moves the database aside only after an explicit confirm', async () => {
    resetIncompatibleDatabase.mockResolvedValue({
      backupPath: `${record.databasePath}.incompatible-20260928-101500`,
      status: 'healthy'
    });
    render(IncompatibleDatabaseBanner);

    await fireEvent.click(screen.getByRole('button', { name: 'Start fresh…' }));
    expect(resetIncompatibleDatabase).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole('button', { name: 'Move aside and start fresh' }));
    await waitFor(() => expect(resetIncompatibleDatabase).toHaveBeenCalledTimes(1));
  });

  it('cancel backs out of the confirm without moving anything', async () => {
    render(IncompatibleDatabaseBanner);

    await fireEvent.click(screen.getByRole('button', { name: 'Start fresh…' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.getByRole('button', { name: 'Start fresh…' })).toBeTruthy();
    expect(resetIncompatibleDatabase).not.toHaveBeenCalled();
  });

  it('shows the backend refusal instead of swallowing it', async () => {
    resetIncompatibleDatabase.mockRejectedValue(
      'the NodeSpace background service is running, so its database is in use'
    );
    render(IncompatibleDatabaseBanner);

    await fireEvent.click(screen.getByRole('button', { name: 'Start fresh…' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Move aside and start fresh' }));

    expect(
      await screen.findByText(
        "Couldn't move the database aside: the NodeSpace background service is running, so its database is in use"
      )
    ).toBeTruthy();
  });
});
