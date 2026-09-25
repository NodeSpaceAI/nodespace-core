/**
 * LabsSettings — the Settings → Labs section housing experimental/
 * not-yet-ready features. Both entries render a real Switch: "AI Chat" is
 * operable and bound to the labs-flags store's `aiChatEnabled`; "Team
 * synchronization" reflects `syncEnabled` but is disabled until the capability
 * is ready — both default off. "Team synchronization" is a pure
 * client-side visibility flag over EXISTING Pro sign-in/collaboration entry
 * points (gated at `resolveProSyncVariant()` plus two direct `proSync.isPro`
 * reads) — this file only covers the Labs switch itself, not that gating.
 */
/* global HTMLButtonElement */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte';
import LabsSettings from '$lib/components/settings/sections/labs-settings.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';

describe('LabsSettings', () => {
  beforeEach(() => {
    localStorage.clear();
    labsFlags.aiChatEnabled = false;
    labsFlags.syncEnabled = false;
  });

  afterEach(() => {
    localStorage.clear();
    labsFlags.aiChatEnabled = false;
    labsFlags.syncEnabled = false;
  });

  it('renders exactly two entries: AI Chat, then Team synchronization', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    expect(cards).toHaveLength(2);

    const headings = Array.from(cards).map(
      (card) => card.querySelector('.font-semibold')?.textContent?.trim()
    );
    expect(headings).toEqual(['AI Chat', 'Team synchronization']);
  });

  it('renders the AI Chat entry with a real Switch, unchecked by default', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const aiChatCard = cards[0];
    const toggle = aiChatCard.querySelector('[data-slot="switch"]');

    expect(toggle).not.toBeNull();
    expect(toggle?.getAttribute('data-state')).toBe('unchecked');
    expect(toggle?.getAttribute('aria-checked')).toBe('false');
  });

  it('marks the AI Chat copy "Experimental — may not work correctly"', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const normalized = cards[0].textContent?.replace(/\s+/g, ' ').trim();
    expect(normalized).toContain('Experimental — may not work correctly.');
  });

  it('clicking the Switch flips labsFlags.aiChatEnabled and reflects the new checked state', async () => {
    const { container } = render(LabsSettings);

    const toggle = container.querySelector('[data-slot="switch"]') as HTMLButtonElement;
    expect(labsFlags.aiChatEnabled).toBe(false);

    await fireEvent.click(toggle);

    expect(labsFlags.aiChatEnabled).toBe(true);
    expect(toggle.getAttribute('data-state')).toBe('checked');
    expect(toggle.getAttribute('aria-checked')).toBe('true');
  });

  it('reflects a pre-existing enabled flag (e.g. after reload) as checked on mount', () => {
    labsFlags.aiChatEnabled = true;

    const { container } = render(LabsSettings);
    const toggle = container.querySelector('[data-slot="switch"]');

    expect(toggle?.getAttribute('data-state')).toBe('checked');
  });

  it('renders the Team synchronization entry with a real Switch, unchecked by default', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const teamSyncCard = cards[1];
    const toggle = teamSyncCard.querySelector('[data-slot="switch"]');

    expect(toggle).not.toBeNull();
    expect(toggle?.getAttribute('data-state')).toBe('unchecked');
    expect(toggle?.getAttribute('aria-checked')).toBe('false');
    // The switch is disabled; the AI Chat one is not.
    expect(toggle?.hasAttribute('disabled')).toBe(true);
    expect(cards[0].querySelector('[data-slot="switch"]')?.hasAttribute('disabled')).toBe(false);
  });

  it('marks the Team synchronization copy with the "under heavy development" message', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const normalized = cards[1].textContent?.replace(/\s+/g, ' ').trim();
    expect(normalized).toContain(
      'Team synchronization is under heavy development. To help us test this capability, contact us at developer@nodespace.ai'
    );
  });

  it('renders the contact email as a real mailto: link', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const link = cards[1].querySelector('a');
    expect(link?.getAttribute('href')).toBe('mailto:developer@nodespace.ai');
    expect(link?.textContent?.trim()).toBe('developer@nodespace.ai');
  });

  it('the disabled Team synchronization Switch cannot be toggled by click or keyboard', async () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const toggle = cards[1].querySelector('[data-slot="switch"]') as HTMLButtonElement;

    await fireEvent.click(toggle);
    await fireEvent.keyDown(toggle, { key: ' ' });
    await fireEvent.keyDown(toggle, { key: 'Enter' });

    expect(labsFlags.syncEnabled).toBe(false);
    expect(toggle.getAttribute('data-state')).toBe('unchecked');
    expect(toggle.getAttribute('aria-checked')).toBe('false');
  });

  it('reflects a pre-existing enabled syncEnabled flag (e.g. after reload) as checked on mount', () => {
    labsFlags.syncEnabled = true;

    const { container } = render(LabsSettings);
    const cards = container.querySelectorAll('[data-slot="card"]');
    const toggle = cards[1].querySelector('[data-slot="switch"]');

    expect(toggle?.getAttribute('data-state')).toBe('checked');
  });
});
