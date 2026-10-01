/**
 * LabsSettings — the Settings → Labs section housing experimental/
 * not-yet-ready features. "AI Chat" and "Playbooks" render a real Switch bound
 * to the labs-flags store, both default off. "Team synchronization" is core's
 * contact card (ADR-084 §1): no switch and no flag, just a link that opens
 * through `openUrl`. How the card yields to an extension's `collaboration.entry`
 * contribution is covered in `labs-collaboration-entry.test.ts`.
 */
/* global HTMLButtonElement, HTMLAnchorElement */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { render, fireEvent } from '@testing-library/svelte';

const openUrl = vi.hoisted(() => vi.fn<(url: string) => Promise<void>>());
const invoke = vi.hoisted(() => vi.fn());

vi.mock('$lib/utils/external-links', () => ({ openUrl }));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

import LabsSettings from '$lib/components/settings/sections/labs-settings.svelte';
import { labsFlags } from '$lib/stores/labs-flags.svelte';
import { TEAM_COLLABORATION_CONTACT_URL } from '$lib/constants/contact';

const srcRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');

function readSource(rel: string): string {
  return fs.readFileSync(path.join(srcRoot, rel), 'utf8');
}

/** The Team synchronization card: the one whose heading names it. */
function teamCard(container: HTMLElement): Element {
  const card = Array.from(container.querySelectorAll('[data-slot="card"]')).find(
    (c) => c.querySelector('.font-semibold')?.textContent?.trim() === 'Team synchronization'
  );
  if (!card) throw new Error('Team synchronization card not rendered');
  return card;
}

describe('LabsSettings', () => {
  let fetchSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    localStorage.clear();
    labsFlags.aiChatEnabled = false;
    labsFlags.playbooksEnabled = false;
    openUrl.mockReset();
    openUrl.mockResolvedValue(undefined);
    invoke.mockReset();
    fetchSpy = vi.fn();
    vi.stubGlobal('fetch', fetchSpy);
  });

  afterEach(() => {
    localStorage.clear();
    labsFlags.aiChatEnabled = false;
    labsFlags.playbooksEnabled = false;
    vi.unstubAllGlobals();
  });

  it('renders exactly three entries: AI Chat, Playbooks, then Team synchronization', () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    expect(cards).toHaveLength(3);

    const headings = Array.from(cards).map(
      (card) => card.querySelector('.font-semibold')?.textContent?.trim()
    );
    expect(headings).toEqual(['AI Chat', 'Playbooks', 'Team synchronization']);
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

  it('renders the Playbooks entry with an operable Switch, off by default, that flips the flag', async () => {
    const { container } = render(LabsSettings);

    const cards = container.querySelectorAll('[data-slot="card"]');
    const toggle = cards[1].querySelector('[data-slot="switch"]') as HTMLButtonElement;

    expect(toggle.getAttribute('data-state')).toBe('unchecked');
    expect(toggle.hasAttribute('disabled')).toBe(false);
    expect(labsFlags.playbooksEnabled).toBe(false);

    await fireEvent.click(toggle);

    expect(labsFlags.playbooksEnabled).toBe(true);
    expect(toggle.getAttribute('data-state')).toBe('checked');
    // Independent of the AI Chat flag.
    expect(labsFlags.aiChatEnabled).toBe(false);
  });

  describe('Team synchronization contact card', () => {
    it('pins the contact link to the mailto constant', () => {
      expect(TEAM_COLLABORATION_CONTACT_URL).toBe('mailto:developer@nodespace.ai');
    });

    it('reads exactly "Want team collaboration? Contact us"', () => {
      const { container } = render(LabsSettings);

      const body = teamCard(container).querySelector('p');
      expect(body?.textContent?.replace(/\s+/g, ' ').trim()).toBe(
        'Want team collaboration? Contact us'
      );
    });

    it('links "Contact us" to the contact constant', () => {
      const { container } = render(LabsSettings);

      const links = teamCard(container).querySelectorAll('a');
      expect(links).toHaveLength(1);
      expect(links[0].getAttribute('href')).toBe(TEAM_COLLABORATION_CONTACT_URL);
      expect(links[0].textContent?.trim()).toBe('Contact us');
    });

    it('has no switch', () => {
      const { container } = render(LabsSettings);

      expect(teamCard(container).querySelector('[data-slot="switch"]')).toBeNull();
      expect(teamCard(container).querySelector('[role="switch"]')).toBeNull();
    });

    it('a click opens exactly the contact link through openUrl, and makes no invoke or fetch', async () => {
      const { container } = render(LabsSettings);
      const link = teamCard(container).querySelector('a') as HTMLAnchorElement;

      const click = new MouseEvent('click', { bubbles: true, cancelable: true });
      link.dispatchEvent(click);
      await Promise.resolve();

      expect(openUrl).toHaveBeenCalledTimes(1);
      expect(openUrl).toHaveBeenCalledWith(TEAM_COLLABORATION_CONTACT_URL);
      // The webview must not navigate to the link itself.
      expect(click.defaultPrevented).toBe(true);
      expect(invoke).not.toHaveBeenCalled();
      expect(fetchSpy).not.toHaveBeenCalled();
    });

    it('a failing openUrl is caught, not thrown', async () => {
      openUrl.mockRejectedValueOnce(new Error('no mail client'));
      const { container } = render(LabsSettings);
      const link = teamCard(container).querySelector('a') as HTMLAnchorElement;

      await fireEvent.click(link);
      await Promise.resolve();

      expect(openUrl).toHaveBeenCalledTimes(1);
    });

    it('the card imports nothing beyond its card UI, the constant, openUrl and the logger', () => {
      // It reads no account state and calls no service (ADR-084 §1).
      const source = readSource('lib/components/settings/sections/team-collaboration-card.svelte');
      const imports = [...source.matchAll(/from\s+'([^']+)'/g)].map((m) => m[1]).sort();
      expect(imports).toEqual([
        '$lib/components/ui/card',
        '$lib/constants/contact',
        '$lib/utils/external-links',
        '$lib/utils/logger'
      ]);
    });

    it('neither the Labs page nor the card refers to a team-sync Labs flag', () => {
      const flag = ['sync', 'Enabled'].join('');
      for (const rel of [
        'lib/components/settings/sections/labs-settings.svelte',
        'lib/components/settings/sections/team-collaboration-card.svelte'
      ]) {
        expect(readSource(rel)).not.toContain(flag);
      }
    });
  });
});
