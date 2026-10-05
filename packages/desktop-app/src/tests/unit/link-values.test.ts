import { describe, it, expect } from 'vitest';
import type { SchemaField, SchemaFieldType } from '$lib/types/schema-node';
import {
  asLinkValue,
  asLinkList,
  linkLabel,
  isOpenableLink,
  isAbsoluteUrl,
  linkFieldText,
  resolveLinkDraft
} from '$lib/utils/link-values';

function field(name: string, type: SchemaFieldType, extra: Partial<SchemaField> = {}): SchemaField {
  return { name, type, friendlyName: name, protection: 'user', indexed: false, ...extra };
}

const core = { title: 'Core', url: 'https://example.com/core' };
const untitled = { title: '', url: 'https://example.com/raw' };

describe('asLinkValue', () => {
  it('reads a title and URL pair', () => {
    expect(asLinkValue(core)).toEqual(core);
  });

  it('is null for anything that is not a link', () => {
    for (const value of [null, undefined, 'https://example.com', [core], { url: 'x' }, { title: 1, url: 'x' }]) {
      expect(asLinkValue(value)).toBeNull();
    }
  });

  it('reads the links of a list, skipping anything else', () => {
    expect(asLinkList([core, 'nope', untitled])).toEqual([core, untitled]);
    expect(asLinkList(core)).toEqual([]);
  });
});

describe('linkLabel', () => {
  it('is the title, or the URL when there is none', () => {
    expect(linkLabel(core)).toBe('Core');
    expect(linkLabel(untitled)).toBe('https://example.com/raw');
    expect(linkLabel({ title: '   ', url: 'https://example.com/raw' })).toBe('https://example.com/raw');
  });
});

describe('isOpenableLink', () => {
  it('opens only http and https', () => {
    expect(isOpenableLink('https://example.com')).toBe(true);
    expect(isOpenableLink('http://example.com/a?b=c')).toBe(true);
    for (const url of [
      'ssh://git@example.com/a.git',
      'file:///etc/passwd',
      'javascript://example.com/%0Aalert(1)',
      'ftp://example.com/a',
      'nodespace://abc',
      'not a url',
      // What the opener refuses is not offered as openable.
      'HTTPS://example.com',
      ' https://example.com'
    ]) {
      expect(isOpenableLink(url), url).toBe(false);
    }
  });
});

describe('isAbsoluteUrl', () => {
  it('needs a scheme and a host', () => {
    expect(isAbsoluteUrl('https://example.com')).toBe(true);
    expect(isAbsoluteUrl('ssh://git@example.com/a.git')).toBe(true);
    for (const url of ['example.com/a', '/a', 'mailto:a@example.com', '']) {
      expect(isAbsoluteUrl(url), url).toBe(false);
    }
  });
});

describe('linkFieldText', () => {
  it('shows a link as its title and a list as its titles', () => {
    expect(linkFieldText(field('repository', 'link'), core)).toBe('Core');
    expect(linkFieldText(field('commits', 'array', { itemType: 'link' }), [core, untitled])).toBe(
      'Core, https://example.com/raw'
    );
  });

  it('is null for a field that holds no links', () => {
    expect(linkFieldText(field('address', 'object'), core)).toBeNull();
    expect(linkFieldText(field('tags', 'array', { itemType: 'text' }), ['a'])).toBeNull();
    expect(linkFieldText(undefined, core)).toBeNull();
    expect(linkFieldText(field('repository', 'link'), 'https://example.com')).toBeNull();
    // A list that is not all links is not read as one, as in the backend's title.
    const commits = field('commits', 'array', { itemType: 'link' });
    expect(linkFieldText(commits, [core, 'x'])).toBeNull();
    expect(linkFieldText(commits, core)).toBeNull();
    expect(linkFieldText(commits, [])).toBe('');
  });
});

describe('resolveLinkDraft', () => {
  it('builds a trimmed link', () => {
    expect(resolveLinkDraft('  Core ', ' https://example.com/core ')).toEqual({ kind: 'link', link: core });
    expect(resolveLinkDraft('', 'https://example.com/raw')).toEqual({ kind: 'link', link: untitled });
  });

  it('clears when both inputs are blank', () => {
    expect(resolveLinkDraft(' ', '')).toEqual({ kind: 'clear' });
  });

  it('refuses a URL that is not absolute', () => {
    expect(resolveLinkDraft('Core', 'example.com/core').kind).toBe('invalid');
    expect(resolveLinkDraft('Core', '').kind).toBe('invalid');
  });

  it('refuses a URL with whitespace inside it, as the backend does', () => {
    for (const url of ['https://example.com/a b', 'https://example.com/?q=a b', 'https://example.com/a\tb']) {
      expect(resolveLinkDraft('Core', url), url).toEqual({
        kind: 'invalid',
        message: 'A URL cannot contain spaces'
      });
    }
  });
});
