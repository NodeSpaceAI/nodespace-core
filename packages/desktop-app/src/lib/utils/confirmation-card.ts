/**
 * The rules of the remote-tool confirmation card: how text from a tool, a model
 * or a peer is made safe to show, how the card checks that what it shows is
 * what the daemon asked, and how a long argument list is shortened.
 */

/**
 * Characters that must never reach the screen as they are: control characters
 * (a newline or an escape sequence can forge or hide lines), the line and
 * paragraph separators, zero-width and invisible formatting (space, joiners,
 * word joiner, BOM), the bidirectional controls (they reorder what a reader
 * sees), the tag block (invisible text) and variation selectors. The same set
 * as the daemon's `is_unsafe_for_log`; `CONFIRMATION_UNSAFE_VECTOR` is the
 * shared test vector.
 */
const UNSAFE = new RegExp(
  '[' +
    '\\u0000-\\u001F\\u007F-\\u009F' + // control characters (C0, DEL, C1)
    '\\u2028\\u2029' + // line and paragraph separators
    '\\u200B-\\u200D\\u2060\\uFEFF' + // zero-width and invisible formatting
    '\\u200E\\u200F\\u061C\\u202A-\\u202E\\u2066-\\u2069' + // bidirectional controls
    '\\uFE00-\\uFE0F\\u180B-\\u180F' + // variation selectors
    ']|[\\u{E0000}-\\u{E007F}\\u{E0100}-\\u{E01EF}]', // tag block, variation selectors supplement
  'gu'
);

/** One of each class above, for tests on both sides to share. */
export const CONFIRMATION_UNSAFE_VECTOR: readonly string[] = [
  '\u0000',
  '\n',
  '\r',
  '\u001b',
  '\u007f',
  '\u0085',
  ' ',
  ' ',
  '​',
  '‌',
  '‍',
  '⁠',
  '﻿',
  '‎',
  '‏',
  '؜',
  '‪',
  '‮',
  '⁦',
  '⁩',
  '︀',
  '️',
  '᠋',
  '᠏',
  '\u{e0041}',
  '\u{e007f}',
  '\u{e0100}',
  '\u{e01ef}'
];

/**
 * Text made safe to show: each unsafe character becomes a visible escape, so a
 * reader sees that it is there and what it is. Applied to every string the card
 * shows that came from a tool, a model or a peer.
 */
export function toDisplaySafe(text: string): string {
  return text.replace(UNSAFE, (ch) => {
    const code = ch.codePointAt(0) ?? 0;
    if (code === 0x0a) return '\\n';
    if (code === 0x0d) return '\\r';
    if (code === 0x09) return '\\t';
    return `\\u{${code.toString(16).toUpperCase()}}`;
  });
}

/** The size of `text` in bytes as UTF-8, the unit the daemon bounds arguments in. */
export function utf8Length(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * The digest the daemon computes for a question: SHA-256 of the length of the
 * summary in bytes (8 bytes, big endian), the summary, and the arguments text,
 * as lowercase hex. The card recomputes it from what it SHOWS and compares it
 * with the daemon's digest on the card; the answer carries the daemon's digest,
 * never this one.
 */
export async function confirmationDigest(summary: string, argumentsJson: string): Promise<string> {
  const encoder = new TextEncoder();
  const summaryBytes = encoder.encode(summary);
  const argumentBytes = encoder.encode(argumentsJson);
  const input = new Uint8Array(8 + summaryBytes.length + argumentBytes.length);
  new DataView(input.buffer).setBigUint64(0, BigInt(summaryBytes.length), false);
  input.set(summaryBytes, 8);
  input.set(argumentBytes, 8 + summaryBytes.length);
  const hash = await crypto.subtle.digest('SHA-256', input);
  return Array.from(new Uint8Array(hash), (b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Whether the credential line agrees with the summary: every reference on it
 * (the text after ` from `) appears in the summary the daemon wrote. A card
 * whose separate credential list was changed to say less than the summary does
 * not pass.
 */
export function credentialsMatchSummary(summary: string, credentials: readonly string[]): boolean {
  return credentials.every((entry) => {
    const at = entry.lastIndexOf(' from ');
    const reference = at >= 0 ? entry.slice(at + ' from '.length) : entry;
    return reference.length > 0 && summary.includes(reference);
  });
}

/** How much of the arguments is shown before the expander. */
export const ARGUMENT_PREVIEW_CHARS = 300;

/** The arguments as shown collapsed: the start, and how much more there is. */
export function argumentPreview(argumentsJson: string): { text: string; moreBytes: number } {
  const chars = Array.from(argumentsJson);
  if (chars.length <= ARGUMENT_PREVIEW_CHARS) return { text: argumentsJson, moreBytes: 0 };
  const shown = chars.slice(0, ARGUMENT_PREVIEW_CHARS).join('');
  return { text: shown, moreBytes: utf8Length(argumentsJson) - utf8Length(shown) };
}
