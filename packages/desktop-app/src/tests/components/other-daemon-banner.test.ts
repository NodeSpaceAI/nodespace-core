/**
 * OtherDaemonBanner — shown while another NodeSpace daemon, outside this app's
 * service registration, holds the socket (ADR-084 §4.3). It names that
 * daemon's executable with the decided wording, and its Retry starts this
 * app's daemon again.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const getOtherDaemon = vi.fn();
const retryDaemonStart = vi.fn();
vi.mock('$lib/services/daemon-status', () => ({
  getOtherDaemon: () => getOtherDaemon(),
  retryDaemonStart: () => retryDaemonStart()
}));

import OtherDaemonBanner from '$lib/components/layout/other-daemon-banner.svelte';

const executable = '/opt/homebrew/opt/other/bin/other-daemon';

function alertText(): string {
  return (screen.getByRole('alert').textContent ?? '').replace(/\s+/g, ' ').trim();
}

describe('OtherDaemonBanner', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    getOtherDaemon.mockResolvedValue(executable);
  });

  it('names the other daemon with the decided wording', async () => {
    render(OtherDaemonBanner);

    await waitFor(() =>
      expect(alertText()).toContain(
        `Another NodeSpace background service is running (${executable}). Stop it, then choose Retry.`
      )
    );
  });

  it('leaves the path out when the daemon named none', async () => {
    getOtherDaemon.mockResolvedValue('');
    render(OtherDaemonBanner);

    await waitFor(() => expect(getOtherDaemon).toHaveBeenCalled());
    expect(alertText()).toContain(
      'Another NodeSpace background service is running. Stop it, then choose Retry.'
    );
    expect(alertText()).not.toContain('()');
  });

  it('Retry starts the daemon again', async () => {
    retryDaemonStart.mockResolvedValue('healthy');
    render(OtherDaemonBanner);

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(retryDaemonStart).toHaveBeenCalledTimes(1));
  });

  it('reads the daemon again when a retry still finds one on the socket', async () => {
    retryDaemonStart.mockResolvedValue('other_daemon');
    render(OtherDaemonBanner);
    await waitFor(() => expect(getOtherDaemon).toHaveBeenCalledTimes(1));

    getOtherDaemon.mockResolvedValue('/usr/local/bin/another-daemon');
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(alertText()).toContain('(/usr/local/bin/another-daemon)'));
    expect(getOtherDaemon).toHaveBeenCalledTimes(2);
  });

  it('shows a failed retry instead of swallowing it', async () => {
    retryDaemonStart.mockRejectedValue(new Error('the app could not reach its backend'));
    render(OtherDaemonBanner);

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    await waitFor(() =>
      expect(alertText()).toContain(
        "Couldn't start the background service: the app could not reach its backend"
      )
    );
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
  });
});
