/**
 * LabsSettings — the Settings → Labs section housing experimental/
 * not-yet-ready features. The "AI Chat" entry has a real, working Switch
 * bound to the labs-flags store's `aiChatEnabled` (default off); "Team
 * synchronization" remains an inert, visibly-disabled placeholder pending
 * its own companion issue.
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

  it('renders Team synchronization as visibly disabled with an "In development" label', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const teamSyncCard = cards[1];

    expect(teamSyncCard.getAttribute('aria-disabled')).toBe('true');
    expect(teamSyncCard.textContent).toContain('In development');

    // The AI Chat card is not disabled — only Team synchronization is.
    expect(cards[0].getAttribute('aria-disabled')).not.toBe('true');
  });
});
