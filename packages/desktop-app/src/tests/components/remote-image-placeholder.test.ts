/**
 * RemoteImagePlaceholder: nothing is requested until the button is pressed,
 * one press loads exactly that one address, and a failure says why in words.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor, within } from '@testing-library/svelte';
import { mockTauriCore } from '../helpers/mock-tauri-core';

const mockInvoke = vi.fn();

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import RemoteImagePlaceholder from '$lib/components/chat/remote-image-placeholder.svelte';

const URL_A = 'https://cdn.example.com/chart.png?id=7';

describe('RemoteImagePlaceholder', () => {
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

  it('makes no request of any kind, and shows no image, before the click', () => {
    const { container, getByRole } = render(RemoteImagePlaceholder, { url: URL_A, alt: 'A chart' });
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(container.querySelector('img')).toBeNull();
    expect(getByRole('button', { name: 'Load image' })).toBeTruthy();
  });

  it('shows the alt text, the host and the full address as plain text', () => {
    const { container } = render(RemoteImagePlaceholder, { url: URL_A, alt: 'A chart' });
    const text = container.textContent ?? '';
    expect(text).toContain('A chart');
    expect(text).toContain(URL_A);
    expect(text).toContain('cdn.example.com');
    expect(text).toContain('IP address');
  });

  it('loads exactly that one address on the click and shows the image from the returned bytes', async () => {
    mockInvoke.mockResolvedValue({ mime: 'image/png', data_base64: 'iVBORw0KGgo=' });
    const { container, getByRole } = render(RemoteImagePlaceholder, { url: URL_A, alt: 'A chart' });
    await fireEvent.click(getByRole('button', { name: 'Load image' }));
    await waitFor(() => expect(container.querySelector('img')).not.toBeNull());
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith('fetch_remote_image', { url: URL_A });
    const img = container.querySelector('img');
    expect(img?.getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=');
    expect(img?.getAttribute('alt')).toBe('A chart');
    // The webview itself never fetched anything.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('ignores a second press while a load is in progress', async () => {
    let resolve: (v: unknown) => void = () => {};
    mockInvoke.mockReturnValue(new Promise((r) => (resolve = r)));
    const { getByRole } = render(RemoteImagePlaceholder, { url: URL_A });
    const button = getByRole('button', { name: 'Load image' });
    await fireEvent.click(button);
    await fireEvent.click(button);
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    resolve({ mime: 'image/png', data_base64: 'AAAA' });
  });

  it('says why a load failed and lets the person try again', async () => {
    mockInvoke.mockRejectedValueOnce('blocked_address');
    const { container, getByRole, findByRole } = render(RemoteImagePlaceholder, { url: URL_A });
    await fireEvent.click(getByRole('button', { name: 'Load image' }));
    const alert = await findByRole('alert');
    expect(alert.textContent).toContain('your own machine or network');
    expect(container.querySelector('img')).toBeNull();

    mockInvoke.mockResolvedValueOnce({ mime: 'image/png', data_base64: 'AAAA' });
    await fireEvent.click(getByRole('button', { name: 'Load image' }));
    await waitFor(() => expect(container.querySelector('img')).not.toBeNull());
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('offers no button for an http address and never calls the app process', () => {
    const { queryByRole, container } = render(RemoteImagePlaceholder, {
      url: 'http://cdn.example.com/a.png'
    });
    expect(queryByRole('button')).toBeNull();
    expect(container.textContent).toContain('Only https images can be loaded.');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('loads only its own address when several placeholders are on screen', async () => {
    mockInvoke.mockResolvedValue({ mime: 'image/png', data_base64: 'AAAA' });
    const first = render(RemoteImagePlaceholder, { url: URL_A });
    const second = render(RemoteImagePlaceholder, { url: 'https://other.example.org/b.png' });
    await fireEvent.click(within(second.container).getByRole('button', { name: 'Load image' }));
    await waitFor(() => expect(second.container.querySelector('img')).not.toBeNull());
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith('fetch_remote_image', {
      url: 'https://other.example.org/b.png'
    });
    expect(first.container.querySelector('img')).toBeNull();
  });
});
