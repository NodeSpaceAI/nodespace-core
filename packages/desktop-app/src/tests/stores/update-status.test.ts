/**
 * App-update status store: banner surfaces only on a real, un-dismissed update;
 * dismissal is per-version; download opens the location the payload names.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
// Type-only, so it is erased before `vi.mock` hoisting and cannot pull the
// module under test in ahead of its own mocks.
import type { UpdateStatus } from '$lib/stores/update-status.svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

// Capture the event callback so tests can simulate an `update://available` event.
let eventCb: ((e: { payload: unknown }) => void) | null = null;
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: (e: { payload: unknown }) => void) => {
    eventCb = cb;
    return () => {};
  })
}));

// Default: the on-demand check finds no update; individual tests drive via events.
// Annotated rather than inferred: inference from this default alone would fix
// `latest` to `null` and the parameter list to zero-arity, rejecting both the
// forwarded `invoke` arguments and the tests that resolve a real `latest`.
const mockInvoke = vi.fn(
  async (..._args: unknown[]): Promise<UpdateStatus> => ({
    current: '0.2.0',
    latest: null,
    update_available: false,
    download_url: null
  })
);
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...a: unknown[]) => mockInvoke(...a) })
);

// Rest parameter for the same reason as `mockInvoke` above: the call site
// forwards `openUrl`'s arguments, which a zero-arity inferred signature rejects.
const mockOpenUrl = vi.fn(async (..._args: unknown[]) => {});
vi.mock('$lib/utils/external-links', () => ({ openUrl: (...a: unknown[]) => mockOpenUrl(...a) }));

import { updateStatus } from '$lib/stores/update-status.svelte';

const DOWNLOAD_URL = 'https://example.test/releases/latest';

function fireUpdate(current: string, latest: string, download_url: string | null = DOWNLOAD_URL) {
  eventCb?.({ payload: { current, latest, update_available: true, download_url } });
}

describe('updateStatus store', () => {
  beforeEach(() => {
    localStorage.clear();
    updateStatus.stop();
    updateStatus.current = '';
    updateStatus.latest = null;
    updateStatus.available = false;
    updateStatus.downloadUrl = null;
    updateStatus.dismissedVersion = null;
    eventCb = null;
    mockOpenUrl.mockClear();
  });

  it('does not show a banner before any update is known', async () => {
    await updateStatus.init();
    expect(updateStatus.showBanner).toBe(false);
  });

  it('shows the banner when an available update event arrives', async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0');
    expect(updateStatus.showBanner).toBe(true);
    expect(updateStatus.latest).toBe('0.3.0');
    expect(updateStatus.current).toBe('0.2.0');
  });

  it('dismiss hides the banner for that version and persists the choice', async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0');
    updateStatus.dismiss();
    expect(updateStatus.showBanner).toBe(false);
    expect(localStorage.getItem('ns:update-dismissed-version')).toBe('0.3.0');
  });

  it('re-shows the banner when a newer version ships after a dismissal', async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0');
    updateStatus.dismiss();
    expect(updateStatus.showBanner).toBe(false);
    fireUpdate('0.2.0', '0.4.0');
    expect(updateStatus.showBanner).toBe(true);
  });

  it('respects a persisted dismissal for the same version on init', async () => {
    localStorage.setItem('ns:update-dismissed-version', '0.3.0');
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0');
    expect(updateStatus.showBanner).toBe(false);
  });

  it('surfaces an available update reported by the on-demand check (post-reload)', async () => {
    mockInvoke.mockResolvedValueOnce({
      current: '0.2.0',
      latest: '0.3.0',
      update_available: true,
      download_url: DOWNLOAD_URL
    });
    await updateStatus.init();
    expect(updateStatus.showBanner).toBe(true);
    expect(updateStatus.latest).toBe('0.3.0');
    expect(updateStatus.downloadUrl).toBe(DOWNLOAD_URL);
  });

  it("download opens the payload's download_url", async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0', 'https://example.test/somewhere-else');
    expect(updateStatus.canDownload).toBe(true);
    await updateStatus.download();
    expect(mockOpenUrl).toHaveBeenCalledTimes(1);
    expect(mockOpenUrl).toHaveBeenCalledWith('https://example.test/somewhere-else');
  });

  it('download is a no-op without a download_url', async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0', null);
    expect(updateStatus.showBanner).toBe(true);
    expect(updateStatus.canDownload).toBe(false);
    await updateStatus.download();
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });

  it('download logs instead of throwing when the download location cannot be opened', async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0', 'not-a-url');
    mockOpenUrl.mockRejectedValueOnce(new Error('Invalid URL protocol'));
    await expect(updateStatus.download()).resolves.toBeUndefined();
    expect(mockOpenUrl).toHaveBeenCalledWith('not-a-url');
  });

  it('a payload that omits download_url is treated as having none', async () => {
    await updateStatus.init();
    eventCb?.({ payload: { current: '0.2.0', latest: '0.3.0', update_available: true } });
    expect(updateStatus.showBanner).toBe(true);
    expect(updateStatus.downloadUrl).toBeNull();
    expect(updateStatus.canDownload).toBe(false);
  });

  it('a later payload without a download_url clears the earlier one', async () => {
    await updateStatus.init();
    fireUpdate('0.2.0', '0.3.0');
    expect(updateStatus.canDownload).toBe(true);
    fireUpdate('0.2.0', '0.4.0', null);
    expect(updateStatus.canDownload).toBe(false);
    await updateStatus.download();
    expect(mockOpenUrl).not.toHaveBeenCalled();
  });
});
