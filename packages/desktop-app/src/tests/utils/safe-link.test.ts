import { describe, it, expect } from 'vitest';
import { isSafeLinkHref } from '$lib/utils/safe-link';
import { parseMarkdown } from '$lib/services/markdown-utils';

describe('isSafeLinkHref', () => {
  it('allows http, https, mailto, nodespace, relative and fragment', () => {
    for (const h of ['http://a', 'HTTPS://a', 'mailto:a@b', 'nodespace://id', 'a/b', '/a', '#x', '?q=1', 'url', '']) {
      expect(isSafeLinkHref(h), h).toBe(true);
    }
  });

  it('rejects javascript, data, vbscript and other schemes in all spellings', () => {
    for (const h of [
      'javascript:alert(1)',
      'JAVASCRIPT:alert(1)',
      'JaVaScRiPt:alert(1)',
      '  javascript:alert(1)',
      '\tjavascript:alert(1)',
      'java\tscript:alert(1)',
      'java\nscript:alert(1)',
      'java\rscript:alert(1)',
      '\u0001javascript:alert(1)',
      '\u0000javascript:alert(1)',
      'jav\u0000ascript:alert(1)',
      '\u200bjavascript:alert(1)',
      '\u00a0javascript:alert(1)',
      'data:text/html;base64,AAAA',
      'DATA:text/html,x',
      'vbscript:msgbox(1)',
      'file:///etc/passwd',
      '1abc:x'
    ]) {
      expect(isSafeLinkHref(h), JSON.stringify(h)).toBe(false);
    }
  });
});

describe('parseMarkdown link href filtering', () => {
  it('renders unsafe link as plain text without an anchor', () => {
    for (const u of ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', ' javascript:x', 'data:text/html,x', 'vbscript:x', 'java\tscript:x']) {
      const out = parseMarkdown(`[click](${u})`);
      expect(out, u).not.toContain('<a');
      expect(out, u).not.toContain('href');
      expect(out, u).toContain('click');
    }
  });

  it('keeps https links', () => {
    expect(parseMarkdown('[x](https://a.test)')).toContain('<a href="https://a.test"');
  });
});
