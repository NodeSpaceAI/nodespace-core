/**
 * A backlink whose node is a chat message is labelled by its text, not its raw
 * id, and opens through the same link every backlink uses: `nodespace://<id>`,
 * which navigation redirects to the parent chat scrolled to the message (see
 * navigation-chat-message.test.ts).
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import BacklinksPanel from '$lib/design/components/backlinks-panel.svelte';
import type { NodeReference } from '$lib/types/node';

async function renderOpen(backlinks: NodeReference[]) {
  const view = render(BacklinksPanel, { props: { backlinks } });
  await fireEvent.click(view.getByLabelText('Toggle backlinks panel'));
  return view;
}

describe('BacklinksPanel chat message entries', () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('labels a message as a chat message and links to its id', async () => {
    const { findByText, container } = await renderOpen([
      { id: 'bl-message', title: null, nodeType: 'ai-chat-message' }
    ]);

    await findByText('Chat message');
    expect(container.querySelector('a')?.getAttribute('href')).toBe('nodespace://bl-message');
    expect(container.textContent).not.toContain('bl-message');
  });

  it('keeps a titled backlink labelled by its title', async () => {
    const { findByText } = await renderOpen([
      { id: 'bl-page', title: 'Project plan', nodeType: 'text' }
    ]);

    await findByText('Project plan');
  });
});
