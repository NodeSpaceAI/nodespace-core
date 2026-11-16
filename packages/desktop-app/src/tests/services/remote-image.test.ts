/**
 * The click-to-load service: one invoke per load, a validated data: URL on
 * success, one stable error code on every failure.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mockTauriCore } from '../helpers/mock-tauri-core';

const mockInvoke = vi.fn();

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import {
  loadRemoteImage,
  describeRemoteImageError,
  RemoteImageLoadError,
  type RemoteImageRefusal
} from '$lib/services/remote-image';

describe('loadRemoteImage', () => {
  beforeEach(() => mockInvoke.mockReset());

  it('asks the app process for exactly the address given and returns a data: URL', async () => {
    mockInvoke.mockResolvedValue({ mime: 'image/png', data_base64: 'iVBORw0KGgo=' });
    const url = await loadRemoteImage('https://cdn.example.com/a.png');
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(mockInvoke).toHaveBeenCalledWith('fetch_remote_image', {
      url: 'https://cdn.example.com/a.png'
    });
    expect(url).toBe('data:image/png;base64,iVBORw0KGgo=');
  });

  it('turns each refusal code into an error that carries that code', async () => {
    const codes: RemoteImageRefusal[] = [
      'invalid_url',
      'not_https',
      'blocked_address',
      'too_many_redirects',
      'bad_status',
      'not_an_image',
      'too_large',
      'network'
    ];
    for (const code of codes) {
      mockInvoke.mockRejectedValueOnce(code);
      await expect(loadRemoteImage('https://x.test/a.png')).rejects.toMatchObject({
        name: 'RemoteImageLoadError',
        code
      });
      expect(describeRemoteImageError(code).length).toBeGreaterThan(10);
    }
  });

  it('reports an unknown failure as a network failure, never as its own text', async () => {
    mockInvoke.mockRejectedValueOnce('boom: 10.0.0.5 refused the connection');
    const error = await loadRemoteImage('https://x.test/a.png').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RemoteImageLoadError);
    expect((error as RemoteImageLoadError).code).toBe('network');
    expect((error as RemoteImageLoadError).message).not.toContain('10.0.0.5');
  });

  it('refuses a payload that is not a plain allowed image', async () => {
    for (const payload of [
      { mime: 'image/svg+xml', data_base64: 'AAAA' },
      { mime: 'text/html', data_base64: 'AAAA' },
      { mime: 'image/png', data_base64: 'AAAA" onerror="alert(1)' },
      { mime: 'image/png;base64,AAAA#', data_base64: 'AAAA' }
    ]) {
      mockInvoke.mockResolvedValueOnce(payload);
      await expect(loadRemoteImage('https://x.test/a.png')).rejects.toMatchObject({
        code: 'not_an_image'
      });
    }
  });
});
