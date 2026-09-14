/**
 * DiagnosticsSettings — the Settings → About section. Covers the Windows
 * daemon-autorun cleanup action: it surfaces a "Remove" button only when
 * `windows_autorun_present` reports an entry exists (always false on
 * macOS/Linux, and on Windows before the daemon has ever registered the HKCU
 * Run key), and clicking it calls `remove_windows_autorun` and hides the row
 * again on success.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import DiagnosticsSettings from '$lib/components/settings/sections/diagnostics-settings.svelte';
import { settingsStore } from '$lib/stores/settings.svelte';

describe('DiagnosticsSettings — Windows autorun cleanup', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settingsStore.appSettings = null;
  });

  it('does not render a startup row when no autorun entry is present (macOS/Linux, or a clean Windows install)', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'windows_autorun_present') return Promise.resolve(false);
      return Promise.resolve(undefined);
    });

    render(DiagnosticsSettings);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('windows_autorun_present');
    });

    expect(screen.queryByText(/starts automatically/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /remove/i })).not.toBeInTheDocument();
  });

  it('renders a Remove action when an autorun entry is present', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'windows_autorun_present') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });

    render(DiagnosticsSettings);

    expect(await screen.findByText(/starts automatically/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /remove/i })).toBeInTheDocument();
  });

  it('removes the entry and hides the row on success', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'windows_autorun_present') return Promise.resolve(true);
      if (cmd === 'remove_windows_autorun') return Promise.resolve(true);
      return Promise.resolve(undefined);
    });

    render(DiagnosticsSettings);

    const button = await screen.findByRole('button', { name: /remove/i });
    await fireEvent.click(button);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('remove_windows_autorun');
    });
    await waitFor(() => {
      expect(screen.queryByText(/starts automatically/i)).not.toBeInTheDocument();
    });
  });

  it('shows an inline error and keeps the row when removal fails', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'windows_autorun_present') return Promise.resolve(true);
      if (cmd === 'remove_windows_autorun') return Promise.reject(new Error('reg.exe failed'));
      return Promise.resolve(undefined);
    });

    render(DiagnosticsSettings);

    const button = await screen.findByRole('button', { name: /remove/i });
    await fireEvent.click(button);

    expect(await screen.findByText(/failed to remove/i)).toBeInTheDocument();
    // Row stays visible so the user can retry.
    expect(screen.getByRole('button', { name: /remove/i })).toBeInTheDocument();
  });
});
