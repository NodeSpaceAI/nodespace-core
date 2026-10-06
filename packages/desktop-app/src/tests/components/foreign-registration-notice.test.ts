/**
 * ForeignRegistrationNotice — shown when the machine-wide service registration
 * under this app's label runs another NodeSpace product's daemon (ADR-084
 * §4.3), with the decided wording. It does not block the app and can be
 * dismissed.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const getForeignMachineWideRegistration = vi.fn();
vi.mock('$lib/services/daemon-status', () => ({
  getForeignMachineWideRegistration: () => getForeignMachineWideRegistration()
}));

import ForeignRegistrationNotice from '$lib/components/layout/foreign-registration-notice.svelte';

const NOTICE =
  "Another NodeSpace product's background service is registered for all users of this Mac. " +
  "To remove it, run that product's uninstaller.";

function noticeText(): string {
  return (screen.getByRole('status').textContent ?? '').replace(/\s+/g, ' ').trim();
}

describe('ForeignRegistrationNotice', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('tells the user how to remove a foreign registration, with the decided wording', async () => {
    getForeignMachineWideRegistration.mockResolvedValue(true);
    render(ForeignRegistrationNotice);

    await waitFor(() => expect(noticeText()).toContain(NOTICE));
  });

  it('shows nothing when the registration is this app’s own, or there is none', async () => {
    getForeignMachineWideRegistration.mockResolvedValue(false);
    render(ForeignRegistrationNotice);

    await waitFor(() => expect(getForeignMachineWideRegistration).toHaveBeenCalled());
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('shows nothing when the check fails', async () => {
    getForeignMachineWideRegistration.mockRejectedValue(new Error('no backend'));
    render(ForeignRegistrationNotice);

    await waitFor(() => expect(getForeignMachineWideRegistration).toHaveBeenCalled());
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('can be dismissed', async () => {
    getForeignMachineWideRegistration.mockResolvedValue(true);
    render(ForeignRegistrationNotice);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeNull());

    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));

    expect(screen.queryByRole('status')).toBeNull();
  });
});
