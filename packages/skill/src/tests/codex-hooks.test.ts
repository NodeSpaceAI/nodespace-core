// The Codex hooks (ADR-093 §5): what the installer registers in Codex's hooks
// file, and what the installed hook command does with each event. Installed
// from the real package, then run as Codex runs it: one process per event.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, chmodSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const TMP = join(tmpdir(), `nodespace-skill-codex-test-${process.pid}`);
const PACKAGE_ROOT = join(import.meta.dirname, '../..');

vi.mock('node:os', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:os')>();
  return { ...actual, homedir: () => TMP };
});

delete process.env.CODEX_HOME;

const { install, uninstall, integrationStatus } = await import('../installer.js');

const codexHome = join(TMP, '.codex');
const hooksFile = join(codexHome, 'hooks.json');
const hookScript = join(codexHome, 'nodespace', 'hook.ts');

type Hooks = Record<string, Array<{ hooks: Array<{ command: string; timeout?: number }> }>>;

function registered(): Hooks {
  return (JSON.parse(readFileSync(hooksFile, 'utf8')) as { hooks: Hooks }).hooks;
}

beforeEach(() => {
  mkdirSync(codexHome, { recursive: true });
});

afterEach(() => {
  rmSync(TMP, { recursive: true, force: true });
});

describe('the Codex hooks registration', () => {
  it('installs the hook script, the module it imports, and one handler per event', () => {
    const [result] = install(['codex']);

    expect(readdirSync(join(codexHome, 'nodespace')).sort()).toEqual(['hook.ts', 'nodespace-session.ts']);
    expect(readFileSync(join(codexHome, 'nodespace', 'nodespace-session.ts'), 'utf8')).toBe(
      readFileSync(join(PACKAGE_ROOT, 'plugins/shared/nodespace-session.ts'), 'utf8')
    );
    expect(Object.keys(registered()).sort()).toEqual([
      'PostToolUse',
      'PreToolUse',
      'SessionEnd',
      'SessionStart',
      'UserPromptSubmit',
    ]);
    for (const groups of Object.values(registered())) {
      expect(groups).toHaveLength(1);
      expect(groups[0]!.hooks[0]!.command).toContain(`"${hookScript}"`);
    }
    // The skill and the instructions block are still there: a session whose
    // hooks are not trusted gets them.
    expect(existsSync(join(codexHome, 'skills', 'nodespace', 'SKILL.md'))).toBe(true);
    expect(readFileSync(join(codexHome, 'AGENTS.md'), 'utf8')).toContain('nodespace');
    expect(result.needsHookTrust).toBe(true);
    expect(integrationStatus('codex')).toEqual({ kind: 'plugin', installed: true });
  });

  it('leaves the hooks the user keeps, and registers its own once on a second install', () => {
    const mine = { hooks: [{ type: 'command', command: 'echo mine' }] };
    writeFileSync(hooksFile, JSON.stringify({ hooks: { PreToolUse: [mine] } }));

    install(['codex']);
    install(['codex']);

    expect(registered().PreToolUse).toHaveLength(2);
    expect(registered().PreToolUse![0]).toEqual(mine);
    expect(registered().SessionStart).toHaveLength(1);
  });

  it('removes its handlers and its files on uninstall, and nothing else', () => {
    const mine = { hooks: [{ type: 'command', command: 'echo mine' }] };
    writeFileSync(hooksFile, JSON.stringify({ hooks: { PreToolUse: [mine] } }));
    install(['codex']);

    uninstall(['codex']);

    expect(JSON.parse(readFileSync(hooksFile, 'utf8'))).toEqual({ hooks: { PreToolUse: [mine] } });
    expect(existsSync(join(codexHome, 'nodespace'))).toBe(false);
    expect(integrationStatus('codex')?.installed).toBe(false);
  });

  it('does not touch a hooks file it cannot read', () => {
    writeFileSync(hooksFile, '{ not json');

    install(['codex']);

    expect(readFileSync(hooksFile, 'utf8')).toBe('{ not json');
  });
});

describe('the installed hook command', () => {
  /** Runs the installed script as one event, with no `nodespace` on the path. */
  function fire(payload: Record<string, unknown>): string {
    const bin = join(TMP, 'bin');
    mkdirSync(bin, { recursive: true });
    // Only a `git` that finds no remote is reachable: the CLI is missing.
    writeFileSync(join(bin, 'git'), '#!/bin/sh\nexit 1\n');
    chmodSync(join(bin, 'git'), 0o755);

    return execFileSync(
      process.execPath,
      ['--experimental-strip-types', '--disable-warning=ExperimentalWarning', hookScript],
      {
        input: JSON.stringify(payload),
        env: { PATH: bin, HOME: TMP, NODESPACE_HOME: TMP },
        encoding: 'utf8',
      }
    );
  }

  it('starts a session, keeps its state in a file, and removes it at the end', () => {
    install(['codex']);
    const state = join(TMP, '.nodespace', 'codex-sessions', 'abc-123.json');

    const started = JSON.parse(
      fire({ hook_event_name: 'SessionStart', session_id: 'abc-123', cwd: TMP, source: 'startup' })
    );

    expect(started.systemMessage).toBe('NodeSpace: the nodespace command was not found');
    expect(existsSync(state)).toBe(true);

    // The next event is a process of its own and finds the session again.
    expect(fire({ hook_event_name: 'UserPromptSubmit', session_id: 'abc-123', cwd: TMP, prompt: 'hi' }).trim()).toBe(
      '{}'
    );

    fire({ hook_event_name: 'SessionEnd', session_id: 'abc-123', cwd: TMP });
    expect(existsSync(state)).toBe(false);
  });

  it('says nothing for an event with no usable session id', () => {
    install(['codex']);

    expect(fire({ hook_event_name: 'PreToolUse', session_id: '../x', cwd: TMP })).toBe('');
  });
});
