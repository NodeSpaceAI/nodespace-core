/**
 * `shell.modalOpen` (host API) is true exactly while one of core's hand-rolled
 * modals is mounted, and is released on every exit path: close, unmount, and
 * a registration released twice.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { flushSync, tick } from 'svelte';
import { render, cleanup } from '@testing-library/svelte';
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({
    invoke: (cmd: string) =>
      Promise.resolve(
        cmd === 'detect_agents'
          ? { agents: [], detectionFailed: false }
          : {
              completed: false,
              pathConfigured: false,
              skillConfigured: false,
              pathAlreadyConfigured: false
            }
      )
  })
);

import { shell } from '@nodespace/extension-api';
import { registerCoreModal } from '$lib/stores/core-modal-presence.svelte';
import DeleteConfirmationModal from '$lib/components/delete-confirmation-modal.svelte';
import SubtreeAccessDeniedModal from '$lib/components/subtree-access-denied-modal.svelte';
import OnboardingWizard from '$lib/components/onboarding/onboarding-wizard.svelte';
import {
  confirmNodeDeletion,
  getDeleteConfirmationState
} from '$lib/services/delete-confirmation.svelte';
import {
  showSubtreeAccessDenied,
  getSubtreeAccessDeniedState
} from '$lib/services/subtree-access-denied.svelte';

beforeEach(() => {
  expect(shell.modalOpen).toBe(false);
});

afterEach(() => {
  cleanup();
  getDeleteConfirmationState().cancel();
  getSubtreeAccessDeniedState().dismiss();
});

describe('shell.modalOpen', () => {
  it('follows a registration and ignores a second release', () => {
    const a = registerCoreModal();
    const b = registerCoreModal();
    expect(shell.modalOpen).toBe(true);
    a();
    a();
    expect(shell.modalOpen).toBe(true); // b still holds it
    b();
    expect(shell.modalOpen).toBe(false);
  });

  it('is reactive: a derivation re-runs when it flips', () => {
    const seen: boolean[] = [];
    const stop = $effect.root(() => {
      const open = $derived(shell.modalOpen);
      $effect(() => {
        seen.push(open);
      });
    });
    flushSync();
    const release = registerCoreModal();
    flushSync();
    release();
    flushSync();
    stop();
    expect(seen).toEqual([false, true, false]);
  });

  it('is on while the delete confirmation shows and off once it resolves', async () => {
    confirmNodeDeletion(1);
    await tick();
    render(DeleteConfirmationModal);
    expect(shell.modalOpen).toBe(true);
    getDeleteConfirmationState().cancel();
    await tick();
    expect(shell.modalOpen).toBe(false);
  });

  it('is on while the subtree-access-denied modal shows and off once dismissed', async () => {
    showSubtreeAccessDenied(2);
    await tick();
    render(SubtreeAccessDeniedModal);
    expect(shell.modalOpen).toBe(true);
    getSubtreeAccessDeniedState().dismiss();
    await tick();
    expect(shell.modalOpen).toBe(false);
  });

  it('is on while the onboarding wizard is open, off when closed, and off after unmount', async () => {
    const { rerender, unmount } = render(OnboardingWizard, {
      props: { open: true, onClose: vi.fn() }
    });
    expect(shell.modalOpen).toBe(true);
    await rerender({ open: false, onClose: vi.fn() });
    expect(shell.modalOpen).toBe(false);
    await rerender({ open: true, onClose: vi.fn() });
    expect(shell.modalOpen).toBe(true);
    unmount();
    expect(shell.modalOpen).toBe(false);
  });

  it('is off when the modal unmounts while still pending', async () => {
    confirmNodeDeletion(1);
    await tick();
    const { unmount } = render(DeleteConfirmationModal);
    expect(shell.modalOpen).toBe(true);
    unmount();
    expect(shell.modalOpen).toBe(false);
  });
});
