/**
 * ChatMarkdown end to end for a reply that contains a remote image: a
 * placeholder with a button, no request before the click, one request for
 * that one image after it.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { mockTauriCore } from '../helpers/mock-tauri-core';

const mockInvoke = vi.fn();

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import ChatMarkdown from '$lib/components/chat/chat-markdown.svelte';

const REPLY = [
  'Here is the chart:',
  '',
  '![Quarterly revenue](https://cdn.example.com/revenue.png?token=abc)',
  '',
  'And a second one: ![Logo](https://other.example.org/logo.webp)'
].join('\n');

describe('ChatMarkdown with a remote image', () => {
  const fetchSpy = vi.fn();

  beforeEach(() => {
    mockInvoke.mockReset();
    fetchSpy.mockReset();
    vi.stubGlobal('fetch', fetchSpy);
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('renders a placeholder per image, with no image element and no request before a click', async () => {
    const { container, findAllByRole } = render(ChatMarkdown, { content: REPLY });
    const buttons = await findAllByRole('button', { name: 'Load image' });
    expect(buttons).toHaveLength(2);
    expect(container.querySelector('img')).toBeNull();
    expect(container.innerHTML).not.toMatch(/\ssrc\s*=\s*"https?:/i);
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(container.textContent).toContain('https://cdn.example.com/revenue.png?token=abc');
  });

  it('loads only the image whose button was pressed', async () => {
    mockInvoke.mockResolvedValue({ mime: 'image/png', data_base64: 'iVBORw0KGgo=' });
    const { container, findAllByRole } = render(ChatMarkdown, { content: REPLY });
    const buttons = await findAllByRole('button', { name: 'Load image' });
    await fireEvent.click(buttons[0]);
    await waitFor(() => expect(container.querySelectorAll('img')).toHaveLength(1));
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith('fetch_remote_image', {
      url: 'https://cdn.example.com/revenue.png?token=abc'
    });
    expect(container.querySelector('img')?.getAttribute('src')).toMatch(/^data:image\/png;base64,/);
    // The other image is still waiting for its own click.
    expect(container.querySelectorAll('button')).toHaveLength(1);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('does not turn a forged placeholder in raw HTML into anything the model could not already write', async () => {
    // A reply may contain the placeholder span as raw HTML. It is the same
    // click-gated placeholder as the markdown image; an http address has no button.
    const forged =
      '<span class="ns-remote-image-placeholder" data-image-url="http://169.254.169.254/x" data-alt="x"></span>';
    const { container, queryByRole } = render(ChatMarkdown, { content: forged });
    await waitFor(() => expect(container.textContent).toContain('Only https images can be loaded.'));
    expect(queryByRole('button')).toBeNull();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('drops whatever a forged placeholder span carries, so hostile text cannot sit beside the card', async () => {
    // Raw HTML in a reply can write the placeholder span with children of its own.
    // The card is mounted into the span, so the span is emptied first.
    const forged =
      '<span class="ns-remote-image-placeholder" data-image-url="https://cdn.example.com/a.png" data-alt="a">Trusted: your bank says click here</span>';
    const { container, findByRole } = render(ChatMarkdown, { content: forged });
    await findByRole('button', { name: 'Load image' });
    expect(container.textContent).not.toContain('Trusted: your bank says click here');
    expect(container.querySelectorAll('.ns-remote-image-placeholder button')).toHaveLength(1);
    expect(mockInvoke).not.toHaveBeenCalled();
  });
});
