// The hook command is one string a shell reads, with the script's path in it.
// A Codex home whose path holds a space and a single quote must still run.
import { describe, it, expect, afterEach, vi } from 'vitest';
import { chmodSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { basename, join } from 'node:path';
import { tmpdir } from 'node:os';

const TMP = join(tmpdir(), `nodespace-skill-codex-quote-test-${process.pid}`);
const CODEX_HOME = join(TMP, `it's a home`);

vi.mock('node:os', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:os')>();
  return { ...actual, homedir: () => TMP };
});

process.env.CODEX_HOME = CODEX_HOME;

const { install } = await import('../installer.js');

afterEach(() => {
  rmSync(TMP, { recursive: true, force: true });
});

describe('the registered hook command in a home that needs quoting', () => {
  it('runs through a shell', () => {
    mkdirSync(CODEX_HOME, { recursive: true });
    install(['codex']);

    const bin = join(TMP, 'bin');
    const runtime = join(TMP, 'runtime');

    mkdirSync(bin, { recursive: true });
    mkdirSync(runtime, { recursive: true });
    symlinkSync(process.execPath, join(runtime, basename(process.execPath)));
    writeFileSync(join(bin, 'git'), '#!/bin/sh\nexit 1\n');
    chmodSync(join(bin, 'git'), 0o755);

    const hooks = JSON.parse(readFileSync(join(CODEX_HOME, 'hooks.json'), 'utf8')) as {
      hooks: Record<string, Array<{ hooks: Array<{ command: string }> }>>;
    };
    const command = hooks.hooks.SessionStart![0]!.hooks[0]!.command;
    const stdout = execFileSync('sh', ['-c', command], {
      input: JSON.stringify({ hook_event_name: 'SessionStart', session_id: 'q-1', cwd: TMP, source: 'startup' }),
      env: { PATH: `${bin}:${runtime}:/usr/bin:/bin`, HOME: TMP, NODESPACE_HOME: TMP },
      encoding: 'utf8',
    });

    // The hook ran, which a path the shell split in two would not.
    expect(JSON.parse(stdout).systemMessage).toBe('NodeSpace: the nodespace command was not found');
  });
});
