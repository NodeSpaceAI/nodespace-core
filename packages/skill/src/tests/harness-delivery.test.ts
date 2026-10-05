// What each harness gets beyond the static skill (ADR-093 §5, §6): a plugin in
// the folder the harness loads code from, or one marked block in its
// instructions file. Installed from the real package, so a file the package
// stops shipping, or a plugin whose import no installed file answers, fails
// here.
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const TMP = join(tmpdir(), `nodespace-skill-delivery-test-${process.pid}`);
const PACKAGE_ROOT = join(import.meta.dirname, '../..');

vi.mock('node:os', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:os')>();
  return { ...actual, homedir: () => TMP };
});

delete process.env.CLAUDE_CONFIG_DIR;
delete process.env.CODEX_HOME;
delete process.env.PI_CODING_AGENT_DIR;
delete process.env.XDG_CONFIG_HOME;

const { install, uninstall, integrationStatus, INSTALL_RECORD } = await import('../installer.js');
const { AGENTS } = await import('../agents.js');
const { BLOCK_BEGIN, BLOCK_END, renderBlock } = await import('../instructions-block.js');

type Agent = typeof AGENTS[number];

const agent = (name: string): Agent => AGENTS.find(a => a.name === name)!;

function record(config: Agent): { files: string[]; plugin_files?: string[]; instructions_file?: string } {
  return JSON.parse(readFileSync(join(config.installDir, INSTALL_RECORD), 'utf8'));
}

beforeEach(() => {
  mkdirSync(TMP, { recursive: true });
});

afterEach(() => {
  rmSync(TMP, { recursive: true, force: true });
});

describe('a plugin in a folder of its own', () => {
  it('installs the Pi extension as a folder Pi loads, with the module it imports beside it', () => {
    const pi = agent('pi');
    mkdirSync(pi.detectionDir, { recursive: true });

    const [result] = install(['pi']);
    const extension = join(TMP, '.pi', 'agent', 'extensions', 'nodespace');

    expect(readdirSync(extension).sort()).toEqual(['index.ts', 'nodespace-session.ts']);
    expect(result.installed).toContain(join(extension, 'index.ts'));
    // The module itself, not the repository's re-export of it.
    expect(readFileSync(join(extension, 'nodespace-session.ts'), 'utf8')).toBe(
      readFileSync(join(PACKAGE_ROOT, 'plugins/shared/nodespace-session.ts'), 'utf8')
    );
    // Nothing of the plugin sits in the skill folder, where Pi loads no code.
    expect(readdirSync(pi.installDir)).not.toContain('index.ts');
    expect(record(pi).plugin_files).toEqual(['index.ts', 'nodespace-session.ts']);
    expect(integrationStatus('pi')).toEqual({ kind: 'plugin', installed: true });
  });

  it('installs one OpenCode plugin file, with the module it imports one folder down', () => {
    const opencode = agent('opencode');
    mkdirSync(opencode.detectionDir, { recursive: true });

    install(['opencode']);
    const plugins = join(TMP, '.config', 'opencode', 'plugins');

    // OpenCode calls every export of every file directly in `plugins/`.
    expect(readdirSync(plugins).sort()).toEqual(['nodespace', 'nodespace.ts']);
    expect(readFileSync(join(plugins, 'nodespace', 'nodespace-session.ts'), 'utf8')).toBe(
      readFileSync(join(PACKAGE_ROOT, 'plugins/shared/nodespace-session.ts'), 'utf8')
    );
    expect(existsSync(join(TMP, '.config', 'opencode', 'skills', 'nodespace', 'SKILL.md'))).toBe(true);
    expect(record(opencode).plugin_files).toEqual(['nodespace.ts', 'nodespace/nodespace-session.ts']);
    expect(integrationStatus('opencode')).toEqual({ kind: 'plugin', installed: true });
  });

  it('does not detect OpenCode by the folder its installer makes', () => {
    mkdirSync(join(TMP, '.opencode', 'bin'), { recursive: true });

    expect(install()).toEqual([]);
  });

  it('reports a re-run over a current install as no change', () => {
    mkdirSync(agent('pi').detectionDir, { recursive: true });

    expect(install(['pi'])[0].changed).toBe(true);
    expect(install(['pi'])[0].changed).toBe(false);
  });

  it('removes the plugin on uninstall and leaves the user\'s own plugins alone', () => {
    const opencode = agent('opencode');
    const plugins = join(TMP, '.config', 'opencode', 'plugins');
    mkdirSync(plugins, { recursive: true });
    writeFileSync(join(plugins, 'mine.ts'), '// the user\'s own plugin', 'utf8');
    install(['opencode']);

    uninstall(['opencode']);

    expect(readdirSync(plugins)).toEqual(['mine.ts']);
    expect(existsSync(opencode.installDir)).toBe(false);
  });

  it('removes the folders it made for the plugin, and never the harness\'s own', () => {
    const pi = agent('pi');
    mkdirSync(pi.detectionDir, { recursive: true });
    install(['pi']);

    uninstall(['pi']);

    expect(readdirSync(pi.detectionDir)).toEqual([]);
  });

  // A plugin runs in every session whether or not the skill folder is there.
  it('removes the plugin even when the skill folder was deleted by hand', () => {
    const pi = agent('pi');
    mkdirSync(pi.detectionDir, { recursive: true });
    install(['pi']);
    rmSync(pi.installDir, { recursive: true });

    const results = uninstall(['pi']);

    expect(results).toHaveLength(1);
    expect(results[0].removed).toHaveLength(2);
    expect(existsSync(pi.plugin!.installDir!)).toBe(false);
  });

  // OpenCode's `plugins/` is the user's folder. With no record of an install,
  // a name is not evidence that a file there is ours.
  it('never removes or replaces a plugin file of the user\'s own that has our file\'s name', () => {
    const opencode = agent('opencode');
    const mine = join(TMP, '.config', 'opencode', 'plugins', 'nodespace.ts');
    mkdirSync(join(TMP, '.config', 'opencode', 'plugins'), { recursive: true });
    writeFileSync(mine, '// written by the user', 'utf8');

    const stderr = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
    uninstall();
    expect(readFileSync(mine, 'utf8')).toBe('// written by the user');
    // Said, not passed over: a plugin left in place runs in every session.
    expect(stderr.mock.calls.map(call => String(call[0])).join('')).toContain(`${mine} was left in place`);
    stderr.mockRestore();

    const [result] = install(['opencode']);
    expect(readFileSync(mine, 'utf8')).toBe('// written by the user');
    expect(existsSync(join(opencode.installDir, 'SKILL.md'))).toBe(true);
    expect(result.installed.some(path => path.includes('plugins'))).toBe(false);
    expect(record(opencode).plugin_files).toBeUndefined();
    expect(integrationStatus('opencode')).toEqual({ kind: 'plugin', installed: false });

    uninstall(['opencode']);
    expect(readFileSync(mine, 'utf8')).toBe('// written by the user');
  });

  it('removes a recorded plugin file the skill no longer ships', () => {
    const pi = agent('pi');
    mkdirSync(pi.detectionDir, { recursive: true });
    install(['pi']);
    const retired = join(pi.plugin!.installDir!, 'retired.ts');
    writeFileSync(retired, '// from an earlier version', 'utf8');
    writeFileSync(
      join(pi.installDir, INSTALL_RECORD),
      JSON.stringify({ ...record(pi), plugin_files: [...record(pi).plugin_files!, 'retired.ts'] }),
      'utf8'
    );

    const [result] = install(['pi']);

    expect(existsSync(retired)).toBe(false);
    expect(result.changed).toBe(true);
    expect(record(pi).plugin_files).toEqual(['index.ts', 'nodespace-session.ts']);
  });

  it('never deletes a recorded plugin file that points outside the plugin folder', () => {
    const pi = agent('pi');
    mkdirSync(pi.detectionDir, { recursive: true });
    install(['pi']);
    const outside = join(TMP, 'precious.ts');
    writeFileSync(outside, 'precious', 'utf8');
    writeFileSync(
      join(pi.installDir, INSTALL_RECORD),
      JSON.stringify({ ...record(pi), plugin_files: ['../../../../precious.ts', outside] }),
      'utf8'
    );

    uninstall(['pi']);

    expect(readFileSync(outside, 'utf8')).toBe('precious');
  });
});

describe('the instructions block in a harness with no plugin', () => {
  const codex = agent('codex');
  const file = join(TMP, '.codex', 'AGENTS.md');

  beforeEach(() => {
    mkdirSync(codex.detectionDir, { recursive: true });
  });

  it('creates the instructions file when it is absent, and records it', () => {
    const [result] = install(['codex']);

    expect(readFileSync(file, 'utf8')).toBe(`${renderBlock()}\n`);
    expect(result.installed).toContain(file);
    expect(record(codex).instructions_file).toBe(file);
    expect(integrationStatus('codex')).toEqual({ kind: 'instructions-block', installed: true });
  });

  it('writes it for Antigravity into the rules file it reads for every project', () => {
    mkdirSync(agent('antigravity').detectionDir, { recursive: true });

    install(['antigravity']);

    expect(readFileSync(join(TMP, '.gemini', 'config', 'AGENTS.md'), 'utf8')).toContain(BLOCK_BEGIN);
  });

  it('appends to a file the user already keeps, and a second install changes nothing', () => {
    const mine = '# My rules\n\nBe terse.\n';
    writeFileSync(file, mine, 'utf8');

    expect(install(['codex'])[0].changed).toBe(true);
    expect(readFileSync(file, 'utf8')).toBe(`${mine}${renderBlock()}\n`);
    expect(install(['codex'])[0].changed).toBe(false);
  });

  it('replaces the block in place on a reinstall, leaving the content around it', () => {
    const before = '# My rules\n\nBe terse.\n';
    const after = '\n## Added after the install\n\nNo emoji.\n';
    writeFileSync(file, `${before}${BLOCK_BEGIN}\nfrom an older version\n${BLOCK_END}\n${after}`, 'utf8');

    const [result] = install(['codex']);

    expect(result.changed).toBe(true);
    expect(readFileSync(file, 'utf8')).toBe(`${before}${renderBlock()}\n${after}`);
  });

  it('removes only the block on uninstall, leaving every other byte', () => {
    const before = '# My rules\n\nBe terse.';
    const after = '\n## Added after the install\r\n\r\nNo emoji.\n\n';
    writeFileSync(file, before, 'utf8');
    install(['codex']);
    writeFileSync(file, readFileSync(file, 'utf8') + after, 'utf8');

    const [result] = uninstall(['codex']);

    expect(readFileSync(file, 'utf8')).toBe(before + after);
    expect(result.removed).toContain(file);
    expect(integrationStatus('codex')).toEqual({ kind: 'instructions-block', installed: false });
  });

  it('deletes the file on uninstall when the block was all it held', () => {
    install(['codex']);

    uninstall(['codex']);

    expect(existsSync(file)).toBe(false);
    expect(existsSync(codex.detectionDir)).toBe(true);
  });

  it('removes the block even when the skill folder was deleted by hand', () => {
    writeFileSync(file, '# Mine\n', 'utf8');
    install(['codex']);
    rmSync(codex.installDir, { recursive: true });

    uninstall(['codex']);

    expect(readFileSync(file, 'utf8')).toBe('# Mine\n');
  });

  // The record names the file the block went into. When the harness's home
  // has moved since, the old file must not keep a block nothing maintains.
  it('takes the block out of the file an earlier install wrote it into, when that was another file', () => {
    const old = join(TMP, 'old-home', 'AGENTS.md');
    mkdirSync(join(TMP, 'old-home'), { recursive: true });
    writeFileSync(old, `# Mine\n${renderBlock()}\n`, 'utf8');
    install(['codex']);
    writeFileSync(
      join(codex.installDir, INSTALL_RECORD),
      JSON.stringify({ ...record(codex), instructions_file: old }),
      'utf8'
    );

    install(['codex']);

    expect(readFileSync(old, 'utf8')).toBe('# Mine\n');
    expect(readFileSync(file, 'utf8')).toContain(BLOCK_BEGIN);
    expect(record(codex).instructions_file).toBe(file);
  });

  // Reading such a file as text and writing it back would change the user's
  // own bytes for good.
  it('leaves an instructions file that is not UTF-8 text exactly as it is', () => {
    const latin1 = Buffer.from([0x23, 0x20, 0x52, 0xe8, 0x67, 0x6c, 0x65, 0x73, 0x0a]);
    writeFileSync(file, latin1);

    const [result] = install(['codex']);

    expect(readFileSync(file).equals(latin1)).toBe(true);
    expect(result.installed).not.toContain(file);
    expect(record(codex).instructions_file).toBeUndefined();
    expect(existsSync(join(codex.installDir, 'SKILL.md'))).toBe(true);
  });

  it('leaves a file with no block untouched on uninstall', () => {
    install(['codex']);
    writeFileSync(file, '# Rewritten by the user\n', 'utf8');

    uninstall(['codex']);

    expect(readFileSync(file, 'utf8')).toBe('# Rewritten by the user\n');
  });
});
