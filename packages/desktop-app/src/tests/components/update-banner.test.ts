/**
 * update-banner component: the Download button is offered only when the update
 * source named a download location, and "Later" is offered either way.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {})
}));
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore({}));

const mockOpenUrl = vi.fn(async (..._args: unknown[]) => {});
vi.mock('$lib/utils/external-links', () => ({ openUrl: (...a: unknown[]) => mockOpenUrl(...a) }));

import { updateStatus } from '$lib/stores/update-status.svelte';
import UpdateBanner from '$lib/components/update-banner.svelte';

const DOWNLOAD_URL = 'https://example.test/releases/latest';

function setUpdate(downloadUrl: string | null) {
  updateStatus.current = '0.2.0';
  updateStatus.latest = '0.3.0';
  updateStatus.available = true;
  updateStatus.downloadUrl = downloadUrl;
}

describe('UpdateBanner', () => {
  beforeEach(() => {
    localStorage.clear();
    updateStatus.current = '';
    updateStatus.latest = null;
    updateStatus.available = false;
    updateStatus.downloadUrl = null;
    updateStatus.dismissedVersion = null;
    mockOpenUrl.mockClear();
  });

  afterEach(() => {
    cleanup();
  });

  it('renders Download when the update names a download location', () => {
    setUpdate(DOWNLOAD_URL);
    render(UpdateBanner);

    expect(screen.getByRole('button', { name: 'Download' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Dismiss update notice' })).toBeTruthy();
  });

  it('opens the named download location when Download is clicked', async () => {
    setUpdate(DOWNLOAD_URL);
    render(UpdateBanner);

    await fireEvent.click(screen.getByRole('button', { name: 'Download' }));

    await waitFor(() => expect(mockOpenUrl).toHaveBeenCalledWith(DOWNLOAD_URL));
  });

  it('shows the new version but no Download when the update names no download location', () => {
    setUpdate(null);
    render(UpdateBanner);

    expect(screen.getByText('0.3.0')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Download' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Dismiss update notice' })).toBeTruthy();
  });

  it('renders nothing when no update is available', () => {
    render(UpdateBanner);

    expect(screen.queryByRole('status')).toBeNull();
  });
});
