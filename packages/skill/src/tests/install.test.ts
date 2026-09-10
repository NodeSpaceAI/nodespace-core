import { describe, it, expect } from 'vitest';
import { extractResourceRoot, skipReasonText, mcpSkipReasonText } from '../install.js';

describe('extractResourceRoot', () => {
  it('returns no resourceRoot and all args unchanged when the flag is absent', () => {
    const { rest, resourceRoot } = extractResourceRoot(['install', 'claude-code']);
    expect(resourceRoot).toBeUndefined();
    expect(rest).toEqual(['install', 'claude-code']);
  });

  it('extracts --resource-root and its value, leaving the remaining args in order', () => {
    const { rest, resourceRoot } = extractResourceRoot([
      'install',
      'claude-code',
      '--resource-root',
      '/path/to/resources/skill'
    ]);
    expect(resourceRoot).toBe('/path/to/resources/skill');
    expect(rest).toEqual(['install', 'claude-code']);
  });

  it('extracts --resource-root when it appears before the positional args', () => {
    const { rest, resourceRoot } = extractResourceRoot([
      '--resource-root',
      '/path/to/resources/skill',
      'install'
    ]);
    expect(resourceRoot).toBe('/path/to/resources/skill');
    expect(rest).toEqual(['install']);
  });

  it('extracts --resource-root with no other args at all', () => {
    const { rest, resourceRoot } = extractResourceRoot(['--resource-root', '/only/this']);
    expect(resourceRoot).toBe('/only/this');
    expect(rest).toEqual([]);
  });

  it('handles a bare --resource-root with no following value without throwing', () => {
    const { rest, resourceRoot } = extractResourceRoot(['install', '--resource-root']);
    // The next token (which doesn't exist) is consumed as the value; there is
    // nothing left to hand `install()` — deliberately not special-cased, this
    // matches every other CLI flag's "you must actually pass a value" shape.
    expect(resourceRoot).toBeUndefined();
    expect(rest).toEqual(['install']);
  });
});

describe('skipReasonText', () => {
  it('names the plugin marketplace when skipReason is plugin-managed', () => {
    const text = skipReasonText({ agent: 'claude-code', installed: [], skipReason: 'plugin-managed' });
    expect(text).toBe('already installed via the Claude Code plugin marketplace, not overwriting');
  });

  it('falls back to the generic incomplete-package message otherwise', () => {
    const text = skipReasonText({ agent: 'codex', installed: [] });
    expect(text).toBe('detected but no files to install (package may be incomplete)');
  });
});

describe('mcpSkipReasonText', () => {
  it('passes through a given reason verbatim', () => {
    expect(mcpSkipReasonText('`nodespace` was not found on $PATH')).toBe(
      '`nodespace` was not found on $PATH'
    );
  });

  it('falls back to a generic message when no reason is given', () => {
    expect(mcpSkipReasonText(undefined)).toBe('detected but nothing to configure');
  });
});
