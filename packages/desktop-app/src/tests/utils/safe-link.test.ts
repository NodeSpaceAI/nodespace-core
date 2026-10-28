import { describe, it, expect } from 'vitest';
import { isSafeLinkHref } from '$lib/utils/safe-link';

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

describe('isSafeLinkHref entity and protocol-relative forms', () => {
  it('rejects entity-encoded schemes', () => {
    for (const h of ['javascript&#58;alert(1)', 'javascript&colon;alert(1)', 'javascript&#x3A;alert(1)', 'java&#9;script:alert(1)', 'java&Tab;script:alert(1)', 'data&colon;text/html,x', 'javascript&#58alert(1)', 'javascript&amp;#58;alert(1)', 'jav&#x61;script:alert(1)', 'javascript&unknownref;:alert(1)']) {
      expect(isSafeLinkHref(h), h).toBe(false);
    }
  });

  it('rejects protocol-relative and backslash-led references', () => {
    for (const h of ['//evil.test/x', '\\\\evil.test', '\\evil', '/\\evil.test', '\\/evil.test', '&#47;/evil.test']) {
      expect(isSafeLinkHref(h), h).toBe(false);
    }
  });

  it('allows an allow-listed scheme written with a character reference', () => {
    expect(isSafeLinkHref('mailto&#58;a@b.test')).toBe(true);
    expect(isSafeLinkHref('https&colon;//a.test')).toBe(true);
  });

  it('still allows ordinary entities in query strings', () => {
    expect(isSafeLinkHref('https://a.test/?a=1&amp;b=2')).toBe(true);
    expect(isSafeLinkHref('?a=1&b=2')).toBe(true);
  });
});
