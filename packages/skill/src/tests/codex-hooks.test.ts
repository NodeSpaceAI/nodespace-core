// The Codex hooks (ADR-093 §5): what the installer registers in Codex's hooks
// file, and what the registered command does with each event. Installed from
// the real package, then run as Codex runs it: the registered command through
// a shell, one process per event, over a fake `nodespace` and `git`.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, chmodSync, symlinkSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { basename, join } from 'node:path';
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
      'SessionStart',
      'UserPromptSubmit',
    ]);
    for (const groups of Object.values(registered())) {
      expect(groups).toHaveLength(1);
      // The path is one single-quoted word: nothing in it is read by the shell.
      expect(groups[0]!.hooks[0]!.command).toContain(`'${hookScript}'`);
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

describe('the registered hook command', () => {
  const bin = join(TMP, 'bin');
  const world = join(TMP, 'world');
  const runtime = join(TMP, 'runtime');

  function item(version: number, contextVersion: string, specVersion = 2): string {
    return JSON.stringify({
      node: { id: 't1', version, title: 'Add the gauge', properties: { status: 'in_progress' }, checkboxes: [] },
      paths: [{ path: 'spec', count: 1, nodes: [{ id: 'spec1', version: specVersion, title: 'Gauge spec' }] }],
      attached_skills: { guidance: [] },
      version: contextVersion,
    });
  }

  function skills(version: string, title: string): string {
    return JSON.stringify({
      version,
      guidance: [{ node_id: 's1', title, use_for: `when ${title} applies`, modified_at: '2026-01-01T00:00:00Z' }],
    });
  }

  /** What the fake `nodespace` answers from: files a test rewrites between events. */
  function set(name: string, content: string): void {
    writeFileSync(join(world, name), content);
  }

  function stubs(): void {
    mkdirSync(bin, { recursive: true });
    mkdirSync(world, { recursive: true });
    // Only the interpreter the hook runs with: a `nodespace` installed on this machine is not found.
    mkdirSync(runtime, { recursive: true });
    symlinkSync(process.execPath, join(runtime, basename(process.execPath)));
    writeFileSync(join(bin, 'git'), '#!/bin/sh\necho git@github.com:acme/widgets.git\n');
    writeFileSync(
      join(bin, 'nodespace'),
      [
        '#!/bin/sh',
        'args=""',
        'for a in "$@"; do case "$a" in --json) ;; *) args="$args $a" ;; esac; done',
        'case "$args" in',
        `  *--version-only*) printf '{"version":"%s"}' "$(cat ${world}/context)" ;;`,
        '  *"--version"*) echo "nodespace 0.2.0" ;;',
        `  *diagnostics*) echo '{"errors":[]}' ;;`,
        `  *"query --type"*) echo '{"nodes":[{"id":"p1","title":"Widgets"}],"count":1}' ;;`,
        `  *"skill guidance"*) cat ${world}/skills ;;`,
        `  *"node context"*) cat ${world}/item ;;`,
        '  *) exit 1 ;;',
        'esac',
        '',
      ].join('\n')
    );
    chmodSync(join(bin, 'git'), 0o755);
    chmodSync(join(bin, 'nodespace'), 0o755);
    set('skills', skills('v1', 'Implementing a task'));
    set('item', item(3, 'c1'));
    set('context', 'c1');
  }

  /** Runs the registered command for `event` through a shell, as Codex does, and parses what it prints. */
  function fire(payload: Record<string, unknown>, path = `${bin}:${runtime}:/usr/bin:/bin`) {
    const command = registered()[String(payload.hook_event_name)]![0]!.hooks[0]!.command;
    const stdout = execFileSync('sh', ['-c', command], {
      input: JSON.stringify({ session_id: 'abc-123', cwd: TMP, ...payload }),
      env: {
        PATH: path,
        HOME: TMP,
        NODESPACE_HOME: TMP,
        // Any elapsed time is past the interval.
        NODESPACE_WATCH_INTERVAL_SECONDS: '0.001',
      },
      encoding: 'utf8',
    });
    return stdout.trim() === '' ? null : (JSON.parse(stdout) as Record<string, any>);
  }

  const shell = (command: string, output = '') => ({
    tool_name: 'Bash',
    tool_input: { command },
    tool_response: { output },
  });

  beforeEach(() => {
    install(['codex']);
    stubs();
  });

  it('gives the session the project and the skill list at start, and keeps its state in a file', () => {
    const started = fire({ hook_event_name: 'SessionStart', source: 'startup' })!;

    expect(started.systemMessage).toBe('NodeSpace: Widgets');
    expect(started.hookSpecificOutput.hookEventName).toBe('SessionStart');
    expect(started.hookSpecificOutput.additionalContext).toContain('Implementing a task');
    expect(existsSync(join(TMP, '.nodespace', 'codex-sessions', 'abc-123.json'))).toBe(true);
  });

  it('says nothing for an event with no usable session id', () => {
    expect(fire({ hook_event_name: 'PreToolUse', session_id: '../x', ...shell('ls') })).toBeNull();
  });

  it('refuses the next call once the task changed under the session, until the user speaks', () => {
    fire({ hook_event_name: 'SessionStart', source: 'startup' });
    fire({ hook_event_name: 'PostToolUse', ...shell('nodespace node context t1') });

    set('item', item(4, 'c2'));
    set('context', 'c2');

    // Each event is a process of its own: the watch, and the refusal, outlive it.
    const refused = fire({ hook_event_name: 'PreToolUse', ...shell('echo hi') })!;

    expect(refused.hookSpecificOutput.hookEventName).toBe('PreToolUse');
    expect(refused.hookSpecificOutput.permissionDecision).toBe('deny');
    expect(refused.hookSpecificOutput.permissionDecisionReason).toContain('changed under it');
    expect(fire({ hook_event_name: 'PreToolUse', ...shell('echo again') })!.hookSpecificOutput.permissionDecision).toBe(
      'deny'
    );

    fire({ hook_event_name: 'UserPromptSubmit', prompt: 'go on' });

    expect(fire({ hook_event_name: 'PreToolUse', ...shell('echo hi') })).toBeNull();
  });

  it('names a changed skill list on the next prompt, with the list itself', () => {
    fire({ hook_event_name: 'SessionStart', source: 'startup' });
    expect(fire({ hook_event_name: 'UserPromptSubmit', prompt: 'hi' })).toEqual({});

    set('skills', skills('v2', 'Implementing a task, revised'));
    const note = fire({ hook_event_name: 'UserPromptSubmit', prompt: 'again' })!;
    const context: string = note.hookSpecificOutput.additionalContext;

    expect(context).toContain("The graph's skill list changed");
    expect(context).toContain('Changed: "Implementing a task, revised"');
    // The section is not in a system prompt here: the new list follows the note.
    expect(context).toContain('## Skills in the graph');
  });

  it('carries a note found before a tool call onto that call\'s result', () => {
    fire({ hook_event_name: 'SessionStart', source: 'startup' });
    fire({ hook_event_name: 'PostToolUse', ...shell('nodespace node context t1') });

    // What governs the task moved; the task itself did not.
    set('item', item(3, 'c2', 3));
    set('context', 'c2');

    expect(fire({ hook_event_name: 'PreToolUse', ...shell('echo hi') })).toBeNull();

    const result = fire({ hook_event_name: 'PostToolUse', ...shell('echo hi', 'hi') })!;

    expect(result.hookSpecificOutput.hookEventName).toBe('PostToolUse');
    expect(result.hookSpecificOutput.additionalContext).toContain('What governs the item you are working on');
    // Delivered once.
    expect(fire({ hook_event_name: 'PostToolUse', ...shell('echo hi', 'hi') })).toEqual({});
  });

  it('continues a resumed thread from its saved state, and starts one with none', () => {
    fire({ hook_event_name: 'SessionStart', source: 'startup' });
    fire({ hook_event_name: 'PostToolUse', ...shell('nodespace node context t1') });
    set('item', item(4, 'c2'));
    set('context', 'c2');
    expect(fire({ hook_event_name: 'PreToolUse', ...shell('echo hi') })!.hookSpecificOutput.permissionDecision).toBe(
      'deny'
    );

    // A new process for the same thread, as `codex resume` is: the refusal is still in force.
    fire({ hook_event_name: 'SessionStart', source: 'resume' });

    expect(fire({ hook_event_name: 'PreToolUse', ...shell('echo hi') })!.hookSpecificOutput.permissionDecision).toBe(
      'deny'
    );

    // A thread this machine has no state for is started, not failed.
    const other = fire({ hook_event_name: 'SessionStart', session_id: 'def-456', source: 'resume' })!;

    expect(other.systemMessage).toBe('NodeSpace: Widgets');
  });

  it('starts again from a state file it cannot use', () => {
    fire({ hook_event_name: 'SessionStart', source: 'startup' });
    writeFileSync(
      join(TMP, '.nodespace', 'codex-sessions', 'abc-123.json'),
      JSON.stringify({ session: { isReached: 'yes' } })
    );

    const prompt = fire({ hook_event_name: 'UserPromptSubmit', prompt: 'hi' });

    expect(prompt).toEqual({});
    expect(JSON.parse(readFileSync(join(TMP, '.nodespace', 'codex-sessions', 'abc-123.json'), 'utf8')).session.isReached).toBe(
      true
    );
  });

  it('reports a missing CLI at start and tries again on a prompt', () => {
    const bare = join(TMP, 'bare');
    mkdirSync(bare, { recursive: true });
    writeFileSync(join(bare, 'git'), '#!/bin/sh\nexit 1\n');
    chmodSync(join(bare, 'git'), 0o755);

    const started = fire(
      { hook_event_name: 'SessionStart', source: 'startup' },
      `${bare}:${runtime}:/usr/bin:/bin`
    )!;

    expect(started.systemMessage).toBe('NodeSpace: the nodespace command was not found');

    // The CLI is there by the next prompt: the session is started then.
    const prompt = fire({ hook_event_name: 'UserPromptSubmit', prompt: 'hi' });

    expect(prompt).toEqual({});
    expect(JSON.parse(readFileSync(join(TMP, '.nodespace', 'codex-sessions', 'abc-123.json'), 'utf8')).session.isReached).toBe(
      true
    );
  });
});
