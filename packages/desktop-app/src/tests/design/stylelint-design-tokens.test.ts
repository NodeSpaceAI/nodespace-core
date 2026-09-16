/**
 * Verifies the design-token Stylelint rules (stylelint.config.js) accept the
 * patterns DESIGN.md mandates and reject the drift it forbids.
 *
 * The config is the regression gate for the design system, so the rules
 * themselves need coverage: a config that silently stops matching (a bad
 * regex, a customSyntax that parses a file as empty) fails open and looks
 * exactly like a clean codebase.
 */

import { describe, it, expect } from 'vitest';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import stylelint from 'stylelint';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const configFile = path.join(packageRoot, 'stylelint.config.js');

/** Lint a CSS snippet, returning the rule names that fired. */
async function lintCss(code: string): Promise<string[]> {
  const { results } = await stylelint.lint({ code, codeFilename: 'probe.css', configFile });
  return results[0].warnings.map((w) => w.rule);
}

/** Lint a Svelte component, returning the rule names that fired. */
async function lintSvelte(styleBlock: string): Promise<string[]> {
  const { results } = await stylelint.lint({
    code: `<div></div>\n<style>\n${styleBlock}\n</style>\n`,
    codeFilename: path.join(packageRoot, 'src/probe.svelte'),
    configFile
  });
  return results[0].warnings.map((w) => w.rule);
}

const DISALLOWED = 'declaration-property-value-disallowed-list';
const ALLOWED = 'declaration-property-value-allowed-list';

describe('stylelint design-token rules', () => {
  describe('parses CSS inside Svelte <style> blocks', () => {
    // The whole point of adding Stylelint: ESLint cannot see this region.
    it('flags a raw color in a Svelte style block', async () => {
      expect(await lintSvelte('.a { color: #ff0000; }')).toContain(DISALLOWED);
    });

    it('accepts a token reference in a Svelte style block', async () => {
      expect(await lintSvelte('.a { color: hsl(var(--foreground)); }')).toEqual([]);
    });

    it('still lints plain .css files', async () => {
      // A global postcss-html customSyntax makes .css files parse as empty,
      // silently skipping every rule — including on src/lib/styles/noderef.css.
      expect(await lintCss('.a { color: #ff0000; }')).toContain(DISALLOWED);
    });
  });

  describe('color literals', () => {
    it.each([
      ['hex', '.a { background: #1d9387; }'],
      ['short hex', '.a { color: #abc; }'],
      ['rgba', '.a { background-color: rgba(0, 0, 0, 0.5); }'],
      ['literal hsl', '.a { border-color: hsl(174 67% 35%); }'],
      ['space-separated rgb', '.a { fill: rgb(10 20 30); }']
    ])('rejects %s', async (_label, code) => {
      expect(await lintCss(code)).toContain(DISALLOWED);
    });

    it.each([
      ['hsl(var())', '.a { color: hsl(var(--foreground)); }'],
      ['hsl(var()) with alpha', '.a { border-color: hsl(var(--border) / 0.5); }'],
      ['bare var()', '.a { fill: var(--primary); }'],
      ['transparent', '.a { background-color: transparent; }'],
      ['currentColor', '.a { outline-color: currentColor; }']
    ])('accepts %s', async (_label, code) => {
      expect(await lintCss(code)).toEqual([]);
    });

    it.each([
      ['white', '.a { color: white; }'],
      ['black', '.a { background-color: black; }'],
      ['rebeccapurple', '.a { border-color: rebeccapurple; }'],
      ['red', '.a { fill: red; }']
    ])('rejects the named color %s', async (_label, code) => {
      // A named color is as much a hardcoded palette decision as a hex is, and
      // it is the form drift takes once the obvious literals are blocked.
      expect(await lintCss(code)).toContain(DISALLOWED);
    });

    it.each([
      ['a leading color word', '.a { color: var(--white-overlay); }'],
      ['a trailing color word', '.a { background: var(--surface-red); }'],
      ['an embedded color word', '.a { fill: var(--btn-teal-hover); }'],
      ['a custom property named after a color', '.a { --tan-surface: hsl(var(--card)); }']
    ])('does not mistake %s for a literal', async (_label, code) => {
      // `\b` would match inside an identifier, so a token merely containing a
      // color word would be rejected — a false positive on correct code, which
      // is what erodes trust in a gate.
      expect(await lintCss(code)).toEqual([]);
    });

    it.each([
      ['a URL path', '.a { background-image: url(/img/gold.png); }'],
      ['a bare filename', '.a { background: url(red.svg) no-repeat; }'],
      ['a custom property holding a URL', '.a { --icon: url(white.svg); }'],
      ['a quoted string', '.a { content: "red"; }'],
      ['a grid-area ident', '.a { grid-area: navy; }'],
      ['a font name', ".a { font-family: 'Gill Sans', Tahoma; }"],
      [
        'a data: URI payload',
        `.a { background-image: url("data:image/svg+xml,%3Csvg%3E%3Cpath stroke='white'/%3E%3C/svg%3E"); }`
      ]
    ])('does not read a color word inside %s as a literal', async (_label, code) => {
      // Icon components are full of url() payloads, and `stroke='white'` inside
      // one is a natural thing to write. Flagging it would fire on a correct
      // change, with a message about design tokens that explains nothing.
      expect(await lintCss(code)).toEqual([]);
    });

    it('still flags a named color inside a gradient', async () => {
      // The URL/string boundary must not weaken real detection.
      expect(await lintCss('.a { background: linear-gradient(white, black); }')).toContain(
        DISALLOWED
      );
    });
  });

  describe('custom property declarations', () => {
    it.each([
      ['hex', '.a { --brand: #ff00aa; }'],
      ['rgb', '.a { --accent: rgb(1 2 3); }'],
      ['named', '.a { --surface: white; }']
    ])('rejects a raw %s color declared as a custom property', async (_label, code) => {
      // Otherwise `--brand: #ff00aa` then `color: var(--brand)` launders the
      // literal past every other rule, and reads as more idiomatic than the
      // thing being blocked.
      expect(await lintCss(code)).toContain(DISALLOWED);
    });

    it.each([
      ['a length', '.a { --spacing: 4px; }'],
      ['a calc()', '.a { --radius-custom: calc(var(--radius) * 2); }'],
      ['a token reference', '.a { --surface: hsl(var(--card)); }']
    ])('accepts a custom property holding %s', async (_label, code) => {
      expect(await lintCss(code)).toEqual([]);
    });
  });

  describe('suppression comments', () => {
    // CLAUDE.md forbids lint suppression. `configurationComment` is renamed so
    // `stylelint-disable` is not a recognized directive — a gate that a comment
    // can switch off is a convention, not a gate.
    it.each([
      ['a bare disable', '/* stylelint-disable */\n.a { color: #ff00aa; }'],
      ['a described disable', '/* stylelint-disable -- reason */\n.a { color: #ff00aa; }'],
      [
        'a next-line disable',
        '/* stylelint-disable-next-line declaration-property-value-disallowed-list */\n.a { color: #ff00aa; }'
      ]
    ])('still reports the violation under %s', async (_label, code) => {
      expect(await lintCss(code)).toContain(DISALLOWED);
    });
  });

  describe('box-shadow', () => {
    const APPROVED =
      '0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)';

    it('accepts the DESIGN.md floating-surface shadow', async () => {
      // This value is specified as a literal rgb(), so the raw-color rule must
      // not reject the very shadow the spec mandates.
      expect(await lintCss(`.a { box-shadow: ${APPROVED}; }`)).toEqual([]);
    });

    it('accepts the approved shadow wrapped across lines', async () => {
      // The formatter wraps this value; a line break must not make the one
      // compliant shadow read as a violation.
      const wrapped = '0 4px 6px -1px rgb(0 0 0 / 0.1),\n    0 2px 4px -2px rgb(0 0 0 / 0.1)';
      expect(await lintCss(`.a {\n  box-shadow: ${wrapped};\n}`)).toEqual([]);
    });

    it('accepts none, for modal panels', async () => {
      expect(await lintCss('.a { box-shadow: none; }')).toEqual([]);
    });

    it.each([
      ['focus ring', '.a { box-shadow: 0 0 0 2px hsl(var(--ring) / 0.2); }'],
      ['inset hairline', '.a { box-shadow: inset 0 0 0 1px hsl(var(--ring) / 0.2); }']
    ])('accepts token-based %s', async (_label, code) => {
      // Focus rings use box-shadow but are not elevation; DESIGN.md's shadow
      // rule is about lifting a surface off the content plane.
      expect(await lintCss(code)).toEqual([]);
    });

    it.each([
      ['literal-color elevation', '.a { box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2); }'],
      ['literal hsl elevation', '.a { box-shadow: 0 8px 32px hsl(0 0% 0% / 0.12); }']
    ])('rejects %s', async (_label, code) => {
      expect(await lintCss(code)).toContain(ALLOWED);
    });
  });

  describe('motion', () => {
    it.each([
      ['sidebar collapse', '.a { transition: width 0.25s ease-out; }'],
      ['code-block button fade', '.a { transition: opacity 0.2s ease; }'],
      ['explicitly disabled', '.a { transition: none; }'],
      // `transition` takes a comma list, so two individually-approved values
      // combined on one element must not be rejected for being combined.
      ['both approved, combined', '.a { transition: width 0.25s ease-out, opacity 0.2s ease; }']
    ])('accepts the approved %s', async (_label, code) => {
      expect(await lintCss(code)).toEqual([]);
    });

    it('rejects a comma list mixing an approved and an unapproved transition', async () => {
      expect(await lintCss('.a { transition: width 0.25s ease-out, color 0.3s linear; }')).toContain(
        ALLOWED
      );
    });

    it.each([
      ['transition: all', '.a { transition: all 0.15s ease; }'],
      ['background-color', '.a { transition: background-color 0.15s ease; }'],
      ['off-spec duration', '.a { transition: width 0.5s ease-out; }']
    ])('rejects unauthorized %s', async (_label, code) => {
      expect(await lintCss(code)).toContain(ALLOWED);
    });

    it('rejects animation shorthand', async () => {
      expect(await lintCss('.a { animation: spin 1s linear infinite; }')).toContain(DISALLOWED);
    });

    it.each([
      ['@keyframes', '@keyframes spin { to { transform: rotate(360deg); } }'],
      // This app ships on WebKit (Tauri), so the prefixed form is the
      // plausible spelling here, not an exotic edge case.
      ['@-webkit-keyframes', '@-webkit-keyframes spin { to { opacity: 0; } }'],
      ['@-moz-keyframes', '@-moz-keyframes spin { to { opacity: 0; } }']
    ])('rejects %s', async (_label, code) => {
      expect(await lintCss(code)).toContain('at-rule-disallowed-list');
    });
  });

  describe('token definitions', () => {
    it('allows raw color values in src/app.css', async () => {
      // Raw values are what a token *is*; this is the one place they belong.
      const { results } = await stylelint.lint({
        code: ':root { --primary: 174 67% 35%; background: #ffffff; }',
        codeFilename: path.join(packageRoot, 'src/app.css'),
        configFile
      });
      expect(results[0].warnings.map((w) => w.rule)).toEqual([]);
    });

    it('still enforces motion rules in src/app.css', async () => {
      // The color exemption must not become a blanket disable.
      const { results } = await stylelint.lint({
        code: '.a { animation: spin 1s linear infinite; }',
        codeFilename: path.join(packageRoot, 'src/app.css'),
        configFile
      });
      expect(results[0].warnings.map((w) => w.rule)).toContain(DISALLOWED);
    });
  });
});
