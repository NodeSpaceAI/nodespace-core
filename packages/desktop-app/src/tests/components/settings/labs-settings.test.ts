/**
 * LabsSettings — the Settings → Labs section housing experimental/
 * not-yet-ready features. Purely presentational: an AI Chat placeholder
 * (no functional controls) and a disabled "Team synchronization" entry.
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/svelte';
import LabsSettings from '$lib/components/settings/sections/labs-settings.svelte';

describe('LabsSettings', () => {
  it('renders exactly two entries: AI Chat, then Team synchronization', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    expect(cards).toHaveLength(2);

    const headings = Array.from(cards).map(
      (card) => card.querySelector('.font-semibold')?.textContent?.trim()
    );
    expect(headings).toEqual(['AI Chat', 'Team synchronization']);
  });

  it('renders the AI Chat entry as a non-interactive placeholder', () => {
    const { container } = render(LabsSettings);

    // Placeholder only — no buttons, inputs, or other controls anywhere in
    // the section for this issue.
    expect(container.querySelectorAll('button, input, select, textarea')).toHaveLength(0);
  });

  it('renders Team synchronization as visibly disabled with an "In development" label', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const teamSyncCard = cards[1];

    expect(teamSyncCard.getAttribute('aria-disabled')).toBe('true');
    expect(teamSyncCard.textContent).toContain('In development');

    // aria-disabled is only meaningful to assistive tech on an element with a
    // widget/composite role — the Card renders a plain <div>, so it needs an
    // explicit role for the disabled state to actually be announced.
    expect(teamSyncCard.getAttribute('role')).toBe('group');
    expect(teamSyncCard.getAttribute('aria-label')).toMatch(/team synchronization/i);
    expect(teamSyncCard.getAttribute('aria-label')).toMatch(/in development/i);

    // The AI Chat card is not disabled — only Team synchronization is.
    expect(cards[0].getAttribute('aria-disabled')).not.toBe('true');
  });
});
