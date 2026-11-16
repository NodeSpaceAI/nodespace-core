/**
 * Click-to-load for one remote image in an AI chat reply.
 *
 * The webview cannot load a remote image (the content security policy allows
 * only the app, `asset:` and `data:` for images), so the one image the person
 * asked for is fetched by the app process (`fetch_remote_image`), which checks
 * the address, the response and the bytes, and returns the image as base64. It
 * is shown as a `data:` image. Nothing calls this except the "Load image"
 * button of a placeholder.
 */

import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('RemoteImage');

/** The reasons the app process refuses a load; mirrors `Refusal::code` in Rust. */
export type RemoteImageRefusal =
  | 'invalid_url'
  | 'not_https'
  | 'blocked_address'
  | 'too_many_redirects'
  | 'bad_status'
  | 'not_an_image'
  | 'too_large'
  | 'network';

/** Mirrors the Rust `RemoteImage` payload. */
interface RemoteImagePayload {
  mime: string;
  data_base64: string;
}

const REFUSAL_CODES: ReadonlySet<string> = new Set<RemoteImageRefusal>([
  'invalid_url',
  'not_https',
  'blocked_address',
  'too_many_redirects',
  'bad_status',
  'not_an_image',
  'too_large',
  'network'
]);

const ALLOWED_MIME = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;

/** A load that did not produce an image. `code` is one of the refusal codes. */
export class RemoteImageLoadError extends Error {
  readonly code: RemoteImageRefusal;

  constructor(code: RemoteImageRefusal) {
    super(code);
    this.name = 'RemoteImageLoadError';
    this.code = code;
  }
}

/** The sentence shown under a placeholder when a load fails. */
export function describeRemoteImageError(code: RemoteImageRefusal): string {
  switch (code) {
    case 'not_https':
      return 'Only https images can be loaded.';
    case 'blocked_address':
      return 'This address points at your own machine or network, so it was not loaded.';
    case 'too_large':
      return 'The image is larger than 5 MB.';
    case 'not_an_image':
      return 'The address did not return a PNG, JPEG, GIF or WebP image.';
    case 'too_many_redirects':
      return 'The address redirected too many times.';
    case 'bad_status':
      return 'The server did not return the image.';
    case 'invalid_url':
      return 'This is not an address that can be loaded.';
    case 'network':
      return 'The image could not be reached.';
  }
}

function asRefusal(error: unknown): RemoteImageRefusal {
  const code = typeof error === 'string' ? error : '';
  return REFUSAL_CODES.has(code) ? (code as RemoteImageRefusal) : 'network';
}

/**
 * Loads the image at `url` and returns it as a `data:` URL. Throws
 * `RemoteImageLoadError` for every failure, so a caller shows one sentence.
 */
export async function loadRemoteImage(url: string): Promise<string> {
  let payload: RemoteImagePayload;
  try {
    payload = await invoke<RemoteImagePayload>('fetch_remote_image', { url });
  } catch (error) {
    const code = asRefusal(error);
    log.debug(`Remote image refused: ${code}`);
    throw new RemoteImageLoadError(code);
  }
  // Checked again here, so a payload that is not a plain allowed image never
  // reaches an `<img src>`.
  if (!ALLOWED_MIME.has(payload.mime) || !BASE64.test(payload.data_base64)) {
    throw new RemoteImageLoadError('not_an_image');
  }
  return `data:${payload.mime};base64,${payload.data_base64}`;
}
