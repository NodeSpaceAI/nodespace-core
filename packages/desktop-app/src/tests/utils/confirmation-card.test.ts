import { describe, it, expect } from 'vitest';
import {
  ARGUMENT_PREVIEW_CHARS,
  CONFIRMATION_UNSAFE_VECTOR,
  argumentPreview,
  confirmationDigest,
  credentialsMatchSummary,
  toDisplaySafe,
  utf8Length
} from '$lib/utils/confirmation-card';

describe('toDisplaySafe', () => {
  it('shows every unsafe character as a visible escape and none survives', () => {
    for (const ch of CONFIRMATION_UNSAFE_VECTOR) {
      const shown = toDisplaySafe(`a${ch}b`);
      expect(shown, JSON.stringify(ch)).not.toContain(ch);
      expect(shown.startsWith('a') && shown.endsWith('b')).toBe(true);
      expect(shown.length).toBeGreaterThan(3);
    }
  });

  it('names the common ones the way a person reads them', () => {
    expect(toDisplaySafe('a\nb')).toBe('a\\nb');
    expect(toDisplaySafe('a\u202eb')).toBe('a\\u{202E}b');
    expect(toDisplaySafe('a\u200bb')).toBe('a\\u{200B}b');
    expect(toDisplaySafe('a\u{e0041}b')).toBe('a\\u{E0041}b');
  });

  it('leaves ordinary text, accents, emoji and CJK alone', () => {
    const text = 'run `git` \u00e9 \u{1F600} \u65e5\u672c\u8a9e {"q":"x"}; path /a/b?c=d';
    expect(toDisplaySafe(text)).toBe(text);
  });

  it('a right-to-left override cannot reorder what is shown', () => {
    const spoof = 'harmless.txt\u202Egnp.exe';
    expect(toDisplaySafe(spoof)).not.toContain('\u202e');
  });
});

describe('confirmationDigest', () => {
  it('matches the daemon: length-prefixed SHA-256 of the summary then the arguments', async () => {
    // The same two vectors are computed with the daemon's algorithm
    // (confirmation_digest in remote_tools.rs).
    expect(
      await confirmationDigest(
        'run `git`; credentials in the environment: T from keychain:t',
        '{"q":"x"}'
      )
    ).toBe('563fe7bbff2b124c9a14943c7bb8806a125ea123cf4c64c4f6dca928fb08c9af');
    expect(await confirmationDigest('a\u00e9', 'b\u0000c')).toBe(
      '8cd5d2d5358425ebd2b6e8fdd49957e52fec3fedc01c99891e256262481de6fe'
    );
  });

  it('moves a byte across the split and the digest changes', async () => {
    expect(await confirmationDigest('a', 'b\u0000c')).not.toBe(
      await confirmationDigest('a\u0000b', 'c')
    );
    expect(await confirmationDigest('a', 'bc')).not.toBe(await confirmationDigest('ab', 'c'));
  });
});

describe('credentialsMatchSummary', () => {
  const summary = 'run `git`; credentials in the environment: T from keychain:tok, U from env:V';
  it('passes when every reference is in the summary', () => {
    expect(credentialsMatchSummary(summary, ['T from keychain:tok', 'U from env:V'])).toBe(true);
    expect(credentialsMatchSummary(summary, [])).toBe(true);
  });
  it('fails when the credential line names something the summary does not', () => {
    expect(credentialsMatchSummary(summary, ['T from keychain:other'])).toBe(false);
  });
});

describe('argumentPreview', () => {
  it('shows short arguments whole', () => {
    expect(argumentPreview('{"q":"x"}')).toEqual({ text: '{"q":"x"}', moreBytes: 0 });
  });
  it('cuts long arguments and says how much more there is, in bytes', () => {
    const long = 'x'.repeat(ARGUMENT_PREVIEW_CHARS + 50);
    const preview = argumentPreview(long);
    expect(preview.text.length).toBe(ARGUMENT_PREVIEW_CHARS);
    expect(preview.moreBytes).toBe(50);
  });
  it('counts bytes, not characters', () => {
    const long = '\u00e9'.repeat(ARGUMENT_PREVIEW_CHARS + 10);
    expect(argumentPreview(long).moreBytes).toBe(utf8Length('\u00e9'.repeat(10)));
  });
});
