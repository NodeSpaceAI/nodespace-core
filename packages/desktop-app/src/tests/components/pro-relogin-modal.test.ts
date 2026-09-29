/**
 * ProReloginModal Component Tests
 *
 * The modal the app shell surfaces when the Pro daemon reports AUTH_REQUIRED
 * (refresh token couldn't be renewed). Presentational only — the shell owns
 * visibility and the actions. Covers: gated rendering, both action callbacks,
 * Escape == work-offline, the pending (in-flight) state, and the optional
 * daemon detail line.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ProReloginModal from '$lib/components/pro-relogin-modal.svelte';

describe('ProReloginModal', () => {
  let onSignIn: ReturnType<typeof vi.fn>;
  let onWorkOffline: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    onSignIn = vi.fn();
    onWorkOffline = vi.fn();
  });

  it('does not render when open is false', () => {
    render(ProReloginModal, { props: { open: false, onSignIn, onWorkOffline } });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('renders the prompt and both actions when open', () => {
    render(ProReloginModal, { props: { open: true, onSignIn, onWorkOffline } });
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(screen.getByText('Sign-in required')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Sign In Again' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Work Offline' })).toBeTruthy();
  });

  it('invokes onSignIn when "Sign In Again" is clicked', async () => {
    render(ProReloginModal, { props: { open: true, onSignIn, onWorkOffline } });
    await fireEvent.click(screen.getByRole('button', { name: 'Sign In Again' }));
    expect(onSignIn).toHaveBeenCalledTimes(1);
    expect(onWorkOffline).not.toHaveBeenCalled();
  });

  it('invokes onWorkOffline when "Work Offline" is clicked', async () => {
    render(ProReloginModal, { props: { open: true, onSignIn, onWorkOffline } });
    await fireEvent.click(screen.getByRole('button', { name: 'Work Offline' }));
    expect(onWorkOffline).toHaveBeenCalledTimes(1);
    expect(onSignIn).not.toHaveBeenCalled();
  });

  it('moves focus into the dialog on open and dismisses on Escape from inside (focus-trap)', async () => {
    render(ProReloginModal, { props: { open: true, onSignIn, onWorkOffline } });
    const dialog = screen.getByRole('dialog');
    // focusTrap moves focus inside the dialog on open — real keyboard focus,
    // not a synthetic dispatch on the overlay.
    expect(dialog.contains(document.activeElement)).toBe(true);
    await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(onWorkOffline).toHaveBeenCalledTimes(1);
  });

  it('disables both actions and shows progress while pending', () => {
    render(ProReloginModal, { props: { open: true, pending: true, onSignIn, onWorkOffline } });
    const signIn = screen.getByRole('button', { name: 'Opening…' });
    expect(signIn.hasAttribute('disabled')).toBe(true);
    expect(screen.getByRole('button', { name: 'Work Offline' }).hasAttribute('disabled')).toBe(
      true
    );
  });

  it('shows the daemon detail when provided', () => {
    render(ProReloginModal, {
      props: { open: true, detail: 'refresh token expired', onSignIn, onWorkOffline }
    });
    expect(screen.getByText('refresh token expired')).toBeTruthy();
  });
});
