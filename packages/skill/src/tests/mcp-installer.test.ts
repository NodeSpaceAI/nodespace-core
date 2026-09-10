import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mkdirSync, rmSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const TMP = join(tmpdir(), `nodespace-mcp-installer-test-${process.pid}`);

vi.mock('node:os', async importOriginal => {
  const actual = await importOriginal<typeof import('node:os')>();
  return { ...actual, homedir: () => TMP };
});

const { MCP_CLIENTS } = await import('../mcp-clients.js');
const { installMcp, uninstallMcp, checkMcpInstalled, resolveNodespaceBinaryPath } = await import(
  '../mcp-installer.js'
);

const FAKE_NODESPACE_PATH = '/opt/homebrew/bin/nodespace';
const claudeDesktop = MCP_CLIENTS.find(c => c.name === 'claude-desktop')!;

beforeEach(() => {
  mkdirSync(TMP, { recursive: true });
});

afterEach(() => {
  rmSync(TMP, { recursive: true, force: true });
});

describe('MCP_CLIENTS config', () => {
  it('defines claude-desktop with a detectionDir, configPath, and serverKey', () => {
    expect(claudeDesktop).toBeTruthy();
    expect(claudeDesktop.detectionDir).toBeTruthy();
    expect(claudeDesktop.configPath).toContain('claude_desktop_config.json');
    expect(claudeDesktop.serverKey).toBe('nodespace');
  });

  it('configPath sits inside detectionDir', () => {
    for (const client of MCP_CLIENTS) {
      expect(client.configPath.startsWith(client.detectionDir)).toBe(true);
    }
  });
});

describe('installMcp', () => {
  it('returns empty array when no clients are detected', () => {
    expect(installMcp(undefined, FAKE_NODESPACE_PATH)).toEqual([]);
  });

  it('writes a fresh config with mcpServers.nodespace pointing at the resolved absolute path', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });

    const results = installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);
    expect(results).toHaveLength(1);
    expect(results[0]).toEqual({
      client: 'claude-desktop',
      installed: true,
      configPath: claudeDesktop.configPath,
    });

    const written = JSON.parse(readFileSync(claudeDesktop.configPath, 'utf8'));
    expect(written.mcpServers.nodespace).toEqual({
      command: FAKE_NODESPACE_PATH,
      args: ['mcp'],
    });
  });

  it('preserves other top-level keys and other mcpServers entries already in the file', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(
      claudeDesktop.configPath,
      JSON.stringify({
        someUnrelatedSetting: true,
        mcpServers: { filesystem: { command: 'npx', args: ['mcp-fs'] } },
      }),
      'utf8'
    );

    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);

    const written = JSON.parse(readFileSync(claudeDesktop.configPath, 'utf8'));
    expect(written.someUnrelatedSetting).toBe(true);
    expect(written.mcpServers.filesystem).toEqual({ command: 'npx', args: ['mcp-fs'] });
    expect(written.mcpServers.nodespace).toEqual({
      command: FAKE_NODESPACE_PATH,
      args: ['mcp'],
    });
  });

  it('overwrites a stale nodespace entry from a previous install (re-run is safe)', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(
      claudeDesktop.configPath,
      JSON.stringify({ mcpServers: { nodespace: { command: '/old/stale/path', args: ['mcp'] } } }),
      'utf8'
    );

    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);

    const written = JSON.parse(readFileSync(claudeDesktop.configPath, 'utf8'));
    expect(written.mcpServers.nodespace.command).toBe(FAKE_NODESPACE_PATH);
  });

  it('creates the client config directory when it does not exist yet', () => {
    // detectionDir NOT created here -- installMcp must create configPath's
    // parent itself when a target is passed explicitly (bypassing
    // detection), matching install()'s own explicit-target behavior.
    expect(existsSync(claudeDesktop.detectionDir)).toBe(false);
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);
    expect(existsSync(claudeDesktop.configPath)).toBe(true);
  });

  it('skips with a reason and writes nothing when nodespace cannot be resolved', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });

    const results = installMcp(['claude-desktop'], null);
    expect(results).toHaveLength(1);
    expect(results[0].installed).toBe(false);
    expect(results[0].skipReason).toContain('not found on $PATH');
    expect(existsSync(claudeDesktop.configPath)).toBe(false);
  });

  it('skips with a reason and writes nothing when the existing config is malformed JSON', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(claudeDesktop.configPath, '{ not valid json', 'utf8');

    const results = installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);
    expect(results[0].installed).toBe(false);
    expect(results[0].skipReason).toContain('could not parse');
    // The malformed file must survive untouched -- never silently clobbered.
    expect(readFileSync(claudeDesktop.configPath, 'utf8')).toBe('{ not valid json');
  });

  it('skips with a reason when the existing config parses but is not an object', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(claudeDesktop.configPath, '[1, 2, 3]', 'utf8');

    const results = installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);
    expect(results[0].installed).toBe(false);
    expect(results[0].skipReason).toContain('expected a JSON object');
  });

  it('detects claude-desktop only when its detectionDir exists', () => {
    expect(installMcp(undefined, FAKE_NODESPACE_PATH)).toEqual([]);
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    const results = installMcp(undefined, FAKE_NODESPACE_PATH);
    expect(results.map(r => r.client)).toEqual(['claude-desktop']);
  });
});

describe('uninstallMcp', () => {
  it('reports removed: false when no config file exists', () => {
    const results = uninstallMcp(['claude-desktop']);
    expect(results).toEqual([
      { client: 'claude-desktop', removed: false, configPath: claudeDesktop.configPath },
    ]);
  });

  it('removes the nodespace entry, preserving other keys and other servers', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(
      claudeDesktop.configPath,
      JSON.stringify({
        someUnrelatedSetting: true,
        mcpServers: {
          nodespace: { command: FAKE_NODESPACE_PATH, args: ['mcp'] },
          filesystem: { command: 'npx', args: ['mcp-fs'] },
        },
      }),
      'utf8'
    );

    const results = uninstallMcp(['claude-desktop']);
    expect(results[0].removed).toBe(true);

    const written = JSON.parse(readFileSync(claudeDesktop.configPath, 'utf8'));
    expect(written.someUnrelatedSetting).toBe(true);
    expect(written.mcpServers.filesystem).toEqual({ command: 'npx', args: ['mcp-fs'] });
    expect(written.mcpServers.nodespace).toBeUndefined();
  });

  it('reports removed: false when the file exists but has no nodespace entry', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(claudeDesktop.configPath, JSON.stringify({ mcpServers: {} }), 'utf8');

    const results = uninstallMcp(['claude-desktop']);
    expect(results[0].removed).toBe(false);
  });

  it('is idempotent -- uninstalling twice does not error the second time', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);

    expect(uninstallMcp(['claude-desktop'])[0].removed).toBe(true);
    expect(uninstallMcp(['claude-desktop'])[0].removed).toBe(false);
  });

  it('skips with a reason and writes nothing when the existing config is malformed JSON', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(claudeDesktop.configPath, '{ not valid json', 'utf8');

    const results = uninstallMcp(['claude-desktop']);
    expect(results[0].removed).toBe(false);
    expect(results[0].skipReason).toContain('could not parse');
  });

  it('defaults to every configured client when no target list is given', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);

    const results = uninstallMcp();
    expect(results.map(r => r.client)).toEqual(MCP_CLIENTS.map(c => c.name));
  });
});

describe('checkMcpInstalled', () => {
  it('returns empty array when nothing is configured', () => {
    expect(checkMcpInstalled()).toEqual([]);
  });

  it('reports a client as installed once installMcp has written its config', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);

    expect(checkMcpInstalled(['claude-desktop'])).toEqual(['claude-desktop']);
  });

  it('no longer reports a client once its nodespace entry is removed by hand', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);
    expect(checkMcpInstalled(['claude-desktop'])).toEqual(['claude-desktop']);

    writeFileSync(claudeDesktop.configPath, JSON.stringify({ mcpServers: {} }), 'utf8');
    expect(checkMcpInstalled(['claude-desktop'])).toEqual([]);
  });

  it('returns false (not throw) for a client whose config is malformed JSON', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    writeFileSync(claudeDesktop.configPath, '{ not valid json', 'utf8');

    expect(checkMcpInstalled(['claude-desktop'])).toEqual([]);
  });

  it('defaults to checking every configured client when no target list is given', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);

    expect(checkMcpInstalled()).toEqual(['claude-desktop']);
  });
});

describe('resolveNodespaceBinaryPath', () => {
  it('returns the trimmed absolute path when `which` finds it', async () => {
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => Buffer.from('/opt/homebrew/bin/nodespace\n'),
    }));
    const { resolveNodespaceBinaryPath: resolve } = await import('../mcp-installer.js');
    expect(resolve()).toBe('/opt/homebrew/bin/nodespace');
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });

  it('returns null when `which` throws (binary not found)', async () => {
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => {
        throw Object.assign(new Error('ENOENT'), { code: 'ENOENT' });
      },
    }));
    const { resolveNodespaceBinaryPath: resolve } = await import('../mcp-installer.js');
    expect(resolve()).toBeNull();
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });

  it('returns null on empty output rather than an empty-string path', async () => {
    vi.resetModules();
    vi.doMock('node:child_process', () => ({
      execFileSync: () => Buffer.from('\n'),
    }));
    const { resolveNodespaceBinaryPath: resolve } = await import('../mcp-installer.js');
    expect(resolve()).toBeNull();
    vi.doUnmock('node:child_process');
    vi.resetModules();
  });
});

describe('install → uninstall round trip', () => {
  it('removes everything install() created', () => {
    mkdirSync(claudeDesktop.detectionDir, { recursive: true });
    installMcp(['claude-desktop'], FAKE_NODESPACE_PATH);
    expect(checkMcpInstalled(['claude-desktop'])).toEqual(['claude-desktop']);

    uninstallMcp(['claude-desktop']);
    expect(checkMcpInstalled(['claude-desktop'])).toEqual([]);
    // The config file itself survives (it may hold other servers/settings) --
    // only this package's own entry is removed.
    expect(existsSync(claudeDesktop.configPath)).toBe(true);
  });
});
