/**
 * Helpers for `link` field values: a title and an absolute URL.
 *
 * Pure and DOM-free. The stored shape is generated from Rust (`LinkValue`);
 * these read a value that may be unset or malformed, decide what a link shows
 * and whether the app opens it, and build the value an edit writes.
 */

import type { LinkValue } from '$lib/types/generated';
import type { SchemaField } from '$lib/types/schema-node';
import { isExternalUrl } from './external-links';

/** The value as a link, or `null` when it is unset or not link-shaped. */
export function asLinkValue(value: unknown): LinkValue | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const { title, url } = value as Record<string, unknown>;
  return typeof title === 'string' && typeof url === 'string' ? { title, url } : null;
}

/** The links in a list-of-links value, skipping anything that is not one. */
export function asLinkList(value: unknown): LinkValue[] {
  if (!Array.isArray(value)) return [];
  return value.map(asLinkValue).filter((link): link is LinkValue => link !== null);
}

/** What a link shows: its title, or its URL when it has none. */
export function linkLabel(link: LinkValue): string {
  return link.title.trim() || link.url;
}

/**
 * Whether the app opens this URL. Any scheme can be stored; only web pages
 * are opened, and every other scheme is shown as text. The rule is the
 * opener's own, so a link offered as openable is one it accepts.
 */
export function isOpenableLink(url: string): boolean {
  return isExternalUrl(url);
}

/** Whether a URL is absolute: it parses with a scheme and a host. */
export function isAbsoluteUrl(url: string): boolean {
  try {
    return new URL(url).host !== '';
  } catch {
    return false;
  }
}

/** True for a `link` field or an `array` field whose items are links. */
export function isLinkListField(field: SchemaField): boolean {
  return field.type === 'array' && field.itemType === 'link';
}

/**
 * The text a link field shows in a cell, a summary or a title: the link's
 * label, or the labels of a list of links joined by commas. A title computed
 * here matches the backend's, which names a link the same way. `null` when
 * `field` holds no links or the value is not one.
 */
export function linkFieldText(field: SchemaField | undefined, value: unknown): string | null {
  if (field?.type === 'link') {
    const link = asLinkValue(value);
    return link ? linkLabel(link) : null;
  }
  if (field && isLinkListField(field) && Array.isArray(value)) {
    const links = asLinkList(value);
    return links.length === value.length ? links.map(linkLabel).join(', ') : null;
  }
  return null;
}

/** What an edit of a link's two inputs produces. */
export type LinkDraftResult =
  | { kind: 'link'; link: LinkValue }
  | { kind: 'clear' }
  | { kind: 'invalid'; message: string };

/**
 * Turn the two inputs of a link editor into a write. Both blank clears the
 * field; otherwise the URL must be absolute, since a link is stored whole and
 * a partial one is refused.
 */
export function resolveLinkDraft(title: string, url: string): LinkDraftResult {
  const trimmedTitle = title.trim();
  const trimmedUrl = url.trim();
  if (trimmedTitle === '' && trimmedUrl === '') return { kind: 'clear' };
  // The stored URL holds no whitespace: a write carrying any is refused.
  if (/[\s\p{Cc}]/u.test(trimmedUrl)) {
    return { kind: 'invalid', message: 'A URL cannot contain spaces' };
  }
  if (!isAbsoluteUrl(trimmedUrl)) {
    return { kind: 'invalid', message: 'Enter a full URL, such as https://example.com' };
  }
  return { kind: 'link', link: { title: trimmedTitle, url: trimmedUrl } };
}
