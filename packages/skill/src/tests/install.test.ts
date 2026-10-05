import { describe, it, expect, afterEach } from 'vitest';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  extractResourceRoot,
  skipReasonText,
  mcpSkipReasonText,
  UP_TO_DATE_TEXT,
  PLUGIN_MANAGED_STATUS_TEXT,
} from '../install.js';

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
    const text = skipReasonText({ agent: 'claude-code', installed: [], changed: false, skipReason: 'plugin-managed' });
    expect(text).toBe('already installed via the Claude Code plugin marketplace, not overwriting');
  });

  it('falls back to the generic incomplete-package message otherwise', () => {
    const text = skipReasonText({ agent: 'codex', installed: [], changed: false });
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

// `uninstall` needs the resource root for an install that predates the install
// record: the files it must remove are named by the skill that was installed, and
// the compiled installer has no package directory of its own to read them from.
// Run as the real CLI, because the argument plumbing is what is under test; HOME
// points at a scratch directory so nothing outside it is touched.
describe('uninstall --resource-root', () => {
  const home = join(tmpdir(), `nodespace-skill-cli-test-${process.pid}`);

  afterEach(() => {
    rmSync(home, { recursive: true, force: true });
  });

  it('reads the skill being uninstalled from the given resource root', () => {
    const installDir = join(home, '.codex', 'skills', 'nodespace');
    const resourceRoot = join(home, 'resources');
    for (const root of [installDir, resourceRoot]) {
      mkdirSync(join(root, 'references'), { recursive: true });
      writeFileSync(join(root, 'SKILL.md'), 'skill', 'utf8');
      writeFileSync(join(root, 'references', 'only-in-this-skill.md'), 'reference', 'utf8');
    }

    const { CLAUDE_CONFIG_DIR: _ignored, ...inherited } = process.env;
    const result = spawnSync(
      'bun',
      [join(import.meta.dirname, '..', 'install.ts'), 'uninstall', 'codex', '--resource-root', resourceRoot],
      { env: { ...inherited, HOME: home, USERPROFILE: home }, encoding: 'utf8' }
    );

    expect(result.stderr).toBe('');
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('codex: removed 2 file(s)');
    expect(existsSync(installDir)).toBe(false);
  });
});

// Run as the real CLI: the lines it prints are what `nodespace skill` parses.
describe('the lines the CLI prints', () => {
  const home = join(tmpdir(), `nodespace-skill-cli-lines-test-${process.pid}`);
  const resourceRoot = join(home, 'resources');

  function run(...args: string[]): string {
    const { CLAUDE_CONFIG_DIR: _ignored, ...inherited } = process.env;
    const result = spawnSync(
      'bun',
      [join(import.meta.dirname, '..', 'install.ts'), ...args, '--resource-root', resourceRoot],
      { env: { ...inherited, HOME: home, USERPROFILE: home }, encoding: 'utf8' }
    );
    expect(result.status).toBe(0);
    return result.stdout;
  }

  afterEach(() => {
    rmSync(home, { recursive: true, force: true });
  });

  it('reports a re-run over a current install as up to date', () => {
    mkdirSync(join(home, '.codex'), { recursive: true });
    mkdirSync(resourceRoot, { recursive: true });
    writeFileSync(join(resourceRoot, 'SKILL.md'), 'skill', 'utf8');

    expect(run('install', 'codex')).toContain('✓ codex: installed 1 file(s)');
    expect(run('install', 'codex')).toContain(`✓ codex: ${UP_TO_DATE_TEXT} (1 file(s))`);
  });

  it('reports a marketplace-managed Claude Code in status as installed that way, not as absent', () => {
    const plugins = join(home, '.claude', 'plugins');
    mkdirSync(plugins, { recursive: true });
    writeFileSync(
      join(plugins, 'installed_plugins.json'),
      JSON.stringify({ plugins: { 'nodespace@some-marketplace': [] } }),
      'utf8'
    );

    const status = run('status', 'claude-code');

    expect(status).toContain(`⚠ claude-code: ${PLUGIN_MANAGED_STATUS_TEXT}`);
    expect(status).not.toContain('not present');
  });
});
