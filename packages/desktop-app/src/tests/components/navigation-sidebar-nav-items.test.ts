/**
 * navigation-sidebar.svelte — the generic nav items and their alert badge.
 *
 * Favorites and Conflicts are hidden until ready, and the old database switcher
 * is gone; its one unique signal — a missing active database — now surfaces as
 * an alert badge on the Settings item, where the user goes to resolve it.
 */
/* global HTMLButtonElement */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';

import NavigationSidebar from '$lib/components/layout/navigation-sidebar.svelte';
import { layoutStore } from '$lib/stores/layout.svelte';
import { databaseStore, type DatabaseInfo } from '$lib/stores/database.svelte';
import { openSettings } from '$lib/utils/open-settings';

vi.mock('$lib/utils/open-settings', () => ({ openSettings: vi.fn() }));

const SETTINGS_BADGED = 'Settings — Active database is missing';

function setCollapsed(collapsed: boolean) {
  layoutStore.state = { ...layoutStore.state, sidebarCollapsed: collapsed };
}

function setActiveDatabaseStatus(status: string) {
  const db: DatabaseInfo = {
    id: 'db-1',
    name: 'Work',
    path: '/tmp/work.db',
    isDefault: true,
    status,
    createdAt: '2026-01-01T00:00:00Z',
    lastOpenedAt: null,
    boundTenantSchema: null,
    boundTenantCollection: null
  };
  databaseStore.databases = [db];
  databaseStore.activeDatabaseId = db.id;
}

describe('NavigationSidebar — nav items', () => {
  beforeEach(() => {
    // The sidebar loads the registry on mount; keep the seeded state instead.
    vi.spyOn(databaseStore, 'load').mockResolvedValue();
    setActiveDatabaseStatus('open');
    setCollapsed(false);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.mocked(openSettings).mockClear();
    databaseStore.databases = [];
    databaseStore.activeDatabaseId = null;
    setCollapsed(false);
  });

  it('does not render Favorites, Conflicts, or the database switcher', () => {
    const { container, queryByText } = render(NavigationSidebar);

    expect(queryByText('Favorites')).toBeNull();
    expect(queryByText('Conflicts')).toBeNull();
    expect(container.querySelector('.db-trigger')).toBeNull();
    expect(container.querySelector('button[aria-label="Settings"]')).not.toBeNull();
  });

  it('shows no badge on Settings while the active database is present', () => {
    const { container } = render(NavigationSidebar);

    expect(container.querySelector('[data-testid="nav-badge-settings"]')).toBeNull();
  });

  it.each([false, true])(
    'badges Settings with an accessible alert when the active database is missing (collapsed: %s)',
    (collapsed) => {
      setCollapsed(collapsed);
      setActiveDatabaseStatus('missing');
      const { container } = render(NavigationSidebar);

      expect(container.querySelector('[data-testid="nav-badge-settings"]')).not.toBeNull();
      const settings = container.querySelector(`button[aria-label="${SETTINGS_BADGED}"]`);
      expect(settings).not.toBeNull();
      expect(settings?.getAttribute('title')).toBe(SETTINGS_BADGED);
    }
  );

  it('clears the badge once the database is no longer missing', async () => {
    setActiveDatabaseStatus('missing');
    const { container } = render(NavigationSidebar);
    expect(container.querySelector('[data-testid="nav-badge-settings"]')).not.toBeNull();

    setActiveDatabaseStatus('open');
    await Promise.resolve();

    expect(container.querySelector('[data-testid="nav-badge-settings"]')).toBeNull();
    expect(container.querySelector('button[aria-label="Settings"]')).not.toBeNull();
  });

  it('a badged Settings item still opens Settings', async () => {
    setActiveDatabaseStatus('missing');
    const { container } = render(NavigationSidebar);

    const settings = container.querySelector(
      `button[aria-label="${SETTINGS_BADGED}"]`
    ) as HTMLButtonElement;
    await fireEvent.click(settings);

    expect(openSettings).toHaveBeenCalledOnce();
  });
});
