// The Antigravity hooks (ADR-093 §5): what the installer registers in
// Antigravity's hooks file, and what the registered command does with each
// event. Installed from the real package, then run as Antigravity runs it: the
// registered command through a shell, one process per event, over a fake
// `nodespace` and `git`.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, chmodSync, symlinkSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { basename, join } from 'node:path';
import { tmpdir } from 'node:os';

const TMP = join(tmpdir(), `nodespace-skill-antigravity-test-${process.pid}`);
const PACKAGE_ROOT = join(import.meta.dirname, '../..');

vi.mock('node:os', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:os')>();
  return { ...actual, homedir: () => TMP };
});

delete process.env.XDG_CONFIG_HOME;

const { install, uninstall, integrationStatus } = await import('../installer.js');

const configDir = join(TMP, '.gemini', 'config');
const hooksFile = join(configDir, 'hooks.json');
const hookScript = join(configDir, 'nodespace', 'hook.ts');

type Handler = { type: string; command: string; timeout?: number };
type Entry = Record<string, Array<Handler | { matcher: string; hooks: Handler[] }>>;

function registered(): Entry {
  return (JSON.parse(readFileSync(hooksFile, 'utf8')) as Record<string, Entry>).nodespace!;
}

beforeEach(() => {
  mkdirSync(join(TMP, '.gemini', 'antigravity-cli'), { recursive: true });
  mkdirSync(configDir, { recursive: true });
});

afterEach(() => {
  rmSync(TMP, { recursive: true, force: true });
});

describe('the Antigravity hooks registration', () => {
  it('installs the hook script, the modules it imports, and one handler per event under its own key', () => {
    const [result] = install(['antigravity']);

    expect(readdirSync(join(configDir, 'nodespace')).sort()).toEqual([
      'hook-process.ts',
      'hook.ts',
      'nodespace-session.ts',
    ]);
    for (const module of ['nodespace-session.ts', 'hook-process.ts']) {
      expect(readFileSync(join(configDir, 'nodespace', module), 'utf8')).toBe(
        readFileSync(join(PACKAGE_ROOT, 'plugins/shared', module), 'utf8')
      );
    }
    expect(Object.keys(registered()).sort()).toEqual(['PostToolUse', 'PreInvocation', 'PreToolUse', 'SessionStart']);

    // Each event's command names it, and the path is one single-quoted word.
    for (const event of ['SessionStart', 'PreInvocation']) {
      const handlers = registered()[event] as Handler[];
      expect(handlers).toHaveLength(1);
      expect(handlers[0]!.command).toContain(`'${hookScript}' ${event}`);
    }
    for (const event of ['PreToolUse', 'PostToolUse']) {
      const groups = registered()[event] as Array<{ matcher: string; hooks: Handler[] }>;
      expect(groups).toHaveLength(1);
      expect(groups[0]!.matcher).toBe('*');
      expect(groups[0]!.hooks[0]!.command).toContain(`'${hookScript}' ${event}`);
    }
    // The skill and the instructions block are still there: a session whose
    // hooks do not run gets them.
    expect(existsSync(join(configDir, 'skills', 'nodespace', 'SKILL.md'))).toBe(true);
    expect(readFileSync(join(configDir, 'AGENTS.md'), 'utf8')).toContain('nodespace');
    // Antigravity runs a registered hook with no trust step.
    expect(result.needsHookTrust).toBeUndefined();
    expect(integrationStatus('antigravity')).toEqual({ kind: 'plugin', installed: true });
  });

  it('leaves the hooks the user keeps, and registers its own once on a second install', () => {
    const mine = { SessionStart: [{ type: 'command', command: 'echo mine' }] };
    writeFileSync(hooksFile, JSON.stringify({ mine }));

    install(['antigravity']);
    install(['antigravity']);

    const file = JSON.parse(readFileSync(hooksFile, 'utf8')) as Record<string, unknown>;

    expect(file.mine).toEqual(mine);
    expect(Object.keys(file).sort()).toEqual(['mine', 'nodespace']);
    expect(registered().SessionStart).toHaveLength(1);
  });

  it('removes its hook and its files on uninstall, and nothing else', () => {
    const mine = { SessionStart: [{ type: 'command', command: 'echo mine' }] };
    writeFileSync(hooksFile, JSON.stringify({ mine }));
    install(['antigravity']);

    uninstall(['antigravity']);

    expect(JSON.parse(readFileSync(hooksFile, 'utf8'))).toEqual({ mine });
    expect(existsSync(join(configDir, 'nodespace'))).toBe(false);
    expect(integrationStatus('antigravity')?.installed).toBe(false);
  });

  it('does not replace or remove a hook of the user\'s that has the same key', () => {
    const theirs = { SessionStart: [{ type: 'command', command: 'echo theirs' }] };
    writeFileSync(hooksFile, JSON.stringify({ nodespace: theirs }));

    const [result] = install(['antigravity']);

    expect(JSON.parse(readFileSync(hooksFile, 'utf8'))).toEqual({ nodespace: theirs });
    expect(result.installed).not.toContain(hooksFile);
    expect(integrationStatus('antigravity')?.installed).toBe(false);

    uninstall(['antigravity']);

    expect(JSON.parse(readFileSync(hooksFile, 'utf8'))).toEqual({ nodespace: theirs });
  });

  it('reports a handler registered under another event as not in place', () => {
    install(['antigravity']);
    const file = JSON.parse(readFileSync(hooksFile, 'utf8'));

    // The PreInvocation handler's command now names another event.
    file.nodespace.PreInvocation[0].command = file.nodespace.PreInvocation[0].command.replaceAll(
      'PreInvocation',
      'SessionStart'
    );
    writeFileSync(hooksFile, JSON.stringify(file));

    expect(integrationStatus('antigravity')?.installed).toBe(false);
  });

  it('removes a hooks file that held nothing but its hook', () => {
    install(['antigravity']);

    uninstall(['antigravity']);

    expect(existsSync(hooksFile)).toBe(false);
  });

  it('does not touch a hooks file it cannot read', () => {
    writeFileSync(hooksFile, '{ not json');

    install(['antigravity']);

    expect(readFileSync(hooksFile, 'utf8')).toBe('{ not json');
  });

  it('reports the plugin as not installed when its hook is gone from the file', () => {
    install(['antigravity']);
    writeFileSync(hooksFile, '{}');

    expect(integrationStatus('antigravity')?.installed).toBe(false);
  });
});

describe('the registered hook command', () => {
  const bin = join(TMP, 'bin');
  const world = join(TMP, 'world');
  const runtime = join(TMP, 'runtime');
  const transcript = join(TMP, 'transcript.jsonl');

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
        `  *"report-harness-session"*) echo '{"node_id":"chat1"}' ;;`,
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

  /** The registered command for `event`, run through a shell as Antigravity runs it; what it prints, parsed. */
  function fire(event: string, payload: Record<string, unknown> = {}) {
    const entry = registered()[event]![0]!;
    const command = 'hooks' in entry ? entry.hooks[0]!.command : entry.command;
    const stdout = execFileSync('sh', ['-c', command], {
      input: JSON.stringify({
        conversationId: 'abc-123',
        workspacePaths: [TMP],
        transcriptPath: transcript,
        ...payload,
      }),
      env: {
        PATH: `${bin}:${runtime}:/usr/bin:/bin`,
        HOME: TMP,
        NODESPACE_HOME: TMP,
        // Any elapsed time is past the interval.
        NODESPACE_WATCH_INTERVAL_SECONDS: '0.001',
      },
      encoding: 'utf8',
    });
    return stdout.trim() === '' ? null : (JSON.parse(stdout) as Record<string, any>);
  }

  const shell = (line: string, stepIdx = 4) => ({
    stepIdx,
    toolCall: { name: 'run_command', args: { CommandLine: line, Cwd: TMP } },
  });

  /** The conversation transcript, as Antigravity writes it: a step per line, a command's result after `Output:`. */
  function writeTranscript(stepIdx: number, output: string): void {
    writeFileSync(
      transcript,
      [
        { step_index: stepIdx - 1, type: 'PLANNER_RESPONSE', tool_calls: [] },
        { step_index: stepIdx, type: 'GENERIC', content: `Created At: x\n\nThe command exited with code 0.\nOutput:\n${output}\n` },
      ]
        .map(step => JSON.stringify(step))
        .join('\n') + '\n'
    );
  }

  const state = () => join(TMP, '.nodespace', 'antigravity-sessions', 'abc-123.json');
  const messageOf = (answer: Record<string, any> | null): string =>
    String(answer?.injectSteps?.[0]?.systemMessage?.systemMessage ?? '');

  beforeEach(() => {
    install(['antigravity']);
    stubs();
  });

  it('tells the session the project and the skill list at start, and keeps its state in a file', () => {
    const started = fire('SessionStart')!;

    expect(messageOf(started)).toContain('Widgets');
    expect(messageOf(started)).toContain('Implementing a task');
    expect(existsSync(state())).toBe(true);
  });

  it('answers every event even when it cannot work: a call is not denied for its silence', () => {
    // No usable conversation id: nothing to restore, and a call goes on.
    expect(fire('PreToolUse', { conversationId: '../x', ...shell('ls') })).toEqual({ decision: 'allow' });
    expect(fire('PostToolUse', { conversationId: '../x', ...shell('ls') })).toEqual({});
    // A state it cannot write (a folder where its file belongs).
    mkdirSync(state(), { recursive: true });
    expect(fire('PreToolUse', shell('ls'))).toEqual({ decision: 'allow' });
    expect(fire('PostToolUse', shell('ls'))).toEqual({});
  });

  it('allows a call the session has no reason to refuse, and refuses once the task changed under it, until the user speaks', () => {
    fire('SessionStart');
    writeTranscript(4, 'ok');
    expect(fire('PostToolUse', shell('nodespace node context t1'))).toEqual({});

    expect(fire('PreToolUse', shell('echo hi'))).toEqual({ decision: 'allow' });

    set('item', item(4, 'c2'));
    set('context', 'c2');

    // Each event is a process of its own: the watch, and the refusal, outlive it.
    const refused = fire('PreToolUse', shell('echo hi'))!;

    expect(refused.decision).toBe('deny');
    expect(refused.reason).toContain('changed under it');
    expect(fire('PreToolUse', shell('echo again'))!.decision).toBe('deny');

    // A tool call is not the user: the model's next call after the refusal does not lift it.
    fire('PreInvocation', { invocationNum: 1 });
    expect(fire('PreToolUse', shell('echo again'))!.decision).toBe('deny');

    // The first call after a user message does.
    fire('PreInvocation', { invocationNum: 0 });
    expect(fire('PreToolUse', shell('echo hi'))).toEqual({ decision: 'allow' });
  });

  it('names a changed skill list on the first model call after a user message, with the list itself', () => {
    fire('SessionStart');
    expect(fire('PreInvocation', { invocationNum: 0 })).toEqual({});

    set('skills', skills('v2', 'Implementing a task, revised'));

    // A call that follows a tool result is not a prompt: nothing is read, nothing said.
    expect(fire('PreInvocation', { invocationNum: 1 })).toEqual({});

    const message = messageOf(fire('PreInvocation', { invocationNum: 0 }));

    expect(message).toContain("The graph's skill list changed");
    expect(message).toContain('Changed: "Implementing a task, revised"');
    expect(message).toContain('## Skills in the graph');
  });

  it('carries a note found before a tool call to the model\'s next call', () => {
    fire('SessionStart');
    writeTranscript(4, 'ok');
    fire('PostToolUse', shell('nodespace node context t1'));

    // What governs the task moved; the task itself did not.
    set('item', item(3, 'c2', 3));
    set('context', 'c2');

    expect(fire('PreToolUse', shell('echo hi'))).toEqual({ decision: 'allow' });
    // A PostToolUse hook answers `{}`, whatever it holds for the agent.
    expect(fire('PostToolUse', shell('echo hi', 5))).toEqual({});

    const note = messageOf(fire('PreInvocation', { invocationNum: 1 }));

    expect(note).toContain('What governs the item you are working on');
    // Delivered once.
    expect(fire('PreInvocation', { invocationNum: 1 })).toEqual({});
  });

  it('reads a command\'s output from the transcript, for the skills the session fetched', () => {
    fire('SessionStart');
    writeTranscript(
      4,
      JSON.stringify({ guidance: [{ node_id: 's1', title: 'Implementing a task', use_for: 'x', modified_at: 'y' }] })
    );
    fire('PostToolUse', shell('nodespace skill get "Implementing a task"'));

    set('skills', skills('v2', 'Implementing a task'));
    set('skills', JSON.stringify({
      version: 'v2',
      guidance: [{ node_id: 's1', title: 'Implementing a task', use_for: 'when it applies', modified_at: '2026-02-02T00:00:00Z' }],
    }));

    const message = messageOf(fire('PreInvocation', { invocationNum: 0 }));

    expect(message).toContain('You fetched this skill earlier');
  });

  it('continues a resumed conversation from its saved state, and starts one with none', () => {
    fire('SessionStart');
    writeTranscript(4, 'ok');
    fire('PostToolUse', shell('nodespace node context t1'));
    set('item', item(4, 'c2'));
    set('context', 'c2');
    expect(fire('PreToolUse', shell('echo hi'))!.decision).toBe('deny');

    // A new process for the same conversation: the refusal is still in force, and nothing is told again.
    expect(fire('SessionStart')).toEqual({});
    expect(fire('PreToolUse', shell('echo hi'))!.decision).toBe('deny');

    // A conversation this machine has no state for is started, not failed.
    expect(messageOf(fire('SessionStart', { conversationId: 'def-456' }))).toContain('Widgets');
  });

  it('does not tell a launched conversation the section twice, but tells it a changed list', () => {
    // A session NodeSpace launched for an item: its context is handed over once, with the first prompt.
    const launched = { NODESPACE_SESSION: 'launch-1', NODESPACE_LAUNCHED_FOR: 't1' };

    const run = (event: string, payload: Record<string, unknown>) => {
      const entry = registered()[event]![0]!;
      const command = 'hooks' in entry ? entry.hooks[0]!.command : entry.command;

      return JSON.parse(
        execFileSync('sh', ['-c', command], {
          input: JSON.stringify({ conversationId: 'abc-123', workspacePaths: [TMP], transcriptPath: transcript, ...payload }),
          env: {
            PATH: `${bin}:${runtime}:/usr/bin:/bin`,
            HOME: TMP,
            NODESPACE_HOME: TMP,
            NODESPACE_WATCH_INTERVAL_SECONDS: '0.001',
            ...launched,
          },
          encoding: 'utf8',
        }).trim() || '{}'
      ) as Record<string, any>;
    };

    expect(messageOf(run('SessionStart', {}))).toContain('Implementing a task');

    const first = messageOf(run('PreInvocation', { invocationNum: 0 }));

    // The opening, once; the section it was told at start is not told again.
    expect(first).toContain('launched to work on the item');
    expect(first).not.toContain('## Skills in the graph');

    set('skills', skills('v2', 'Implementing a task, revised'));

    expect(messageOf(run('PreInvocation', { invocationNum: 0 }))).toContain('## Skills in the graph');
  });

  it('does not take a call with no invocation number for a user message', () => {
    fire('SessionStart');
    writeTranscript(4, 'ok');
    fire('PostToolUse', shell('nodespace node context t1'));
    set('item', item(4, 'c2'));
    set('context', 'c2');
    expect(fire('PreToolUse', shell('echo hi'))!.decision).toBe('deny');

    fire('PreInvocation', {});
    fire('PreInvocation', { invocationNum: '0' });

    expect(fire('PreToolUse', shell('echo hi'))!.decision).toBe('deny');
  });

  it('reads the output of a step among lines it cannot parse', () => {
    fire('SessionStart');
    const listing = JSON.stringify({ guidance: [{ node_id: 's1', title: 'Implementing a task', use_for: 'x', modified_at: 'y' }] });
    writeFileSync(
      transcript,
      [
        '{"step_index":3,"type":"PLANNER_RESPONSE"',
        JSON.stringify({ step_index: 4, type: 'GENERIC', content: `Output:\n${listing}\n` }),
        'not json at all',
      ].join('\n') + '\n'
    );

    expect(fire('PostToolUse', shell('nodespace skill get "Implementing a task"'))).toEqual({});
  });

  it('starts a conversation it first meets on a model call, and tells it the section once', () => {
    const first = fire('PreInvocation', { invocationNum: 0 })!;

    expect(messageOf(first)).toContain('Implementing a task');
    expect(fire('PreInvocation', { invocationNum: 0 })).toEqual({});
  });

  it('starts again from a state file it cannot use', () => {
    fire('SessionStart');
    writeFileSync(state(), JSON.stringify({ session: { isReached: 'yes' } }));

    expect(fire('PreInvocation', { invocationNum: 0 })).toBeTruthy();
    expect(JSON.parse(readFileSync(state(), 'utf8')).session.isReached).toBe(true);
  });

  it('says nothing when the CLI is missing, and does not stop the call', () => {
    rmSync(join(bin, 'nodespace'));

    expect(fire('SessionStart')).toEqual({});
    expect(fire('PreToolUse', shell('echo hi'))).toEqual({ decision: 'allow' });
  });
});
