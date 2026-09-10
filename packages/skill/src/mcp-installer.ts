import { existsSync, mkdirSync, readFileSync, writeFileSync, renameSync } from 'node:fs';
import { dirname } from 'node:path';
import { execFileSync } from 'node:child_process';
import { MCP_CLIENTS } from './mcp-clients.js';
import type { McpClientName } from './mcp-clients.js';

/**
 * Writes/removes a bash-less MCP client's config (e.g. Claude Desktop's
 * `claude_desktop_config.json`) so it launches `nodespace mcp` -- the
 * installer gap ADR-038's Trust Boundary section implies: `installer.ts`
 * already does this for each CLI harness's `SKILL.md`/shim files, but
 * nothing wrote an MCP client's own config. Mirrors that module's shape
 * (pure `node:fs`, a target list, `{name, ...}[]` results, no npm dependency
 * -- `packages/skill`'s `dependencies` stays `{}`) rather than inventing a
 * parallel one.
 *
 * Deliberately NOT called by `install()`/`uninstall()` in `installer.ts`, and
 * not run automatically by anything: per ADR-038's Trust Boundary, this tool
 * must not be registered until a user has explicitly turned it on. The only
 * callers are `install.ts`'s `mcp-install`/`mcp-uninstall`/`mcp-status`
 * commands, invoked by `nodespace mcp install`/`uninstall`/`status`
 * (`packages/cli/src/commands/mcp.rs`), which also flips the
 * `~/.nodespace/daemon.toml` `[mcp] enabled` flag that
 * `nodespace mcp`'s own server checks at startup -- so writing a client
 * config here is necessary but not sufficient for the tool to go live.
 */

export interface McpInstallResult {
  client: McpClientName;
  installed: boolean;
  configPath: string;
  /** Set when `installed` is false and there's a reason worth surfacing. */
  skipReason?: string;
}

export interface McpUninstallResult {
  client: McpClientName;
  removed: boolean;
  configPath: string;
  skipReason?: string;
}

function detectMcpClients(): McpClientName[] {
  return MCP_CLIENTS.filter(client => existsSync(client.detectionDir)).map(client => client.name);
}

/**
 * Resolves the `nodespace` binary's absolute path via `which`, rather than
 * returning the bare command name `isNodespaceBinaryOnPath` checks for. GUI
 * apps on macOS/Linux (Claude Desktop among them) spawn child processes
 * without the interactive shell's `$PATH` -- a bare `"nodespace"` `command`
 * in `claude_desktop_config.json` is a well-known way for an MCP server
 * entry to silently fail to launch. Returns `null` (never throws) when
 * `which` itself is unavailable or `nodespace` isn't found.
 */
export function resolveNodespaceBinaryPath(): string | null {
  try {
    const out = execFileSync('which', ['nodespace'], {
      stdio: ['ignore', 'pipe', 'ignore'],
      timeout: 3000,
    })
      .toString()
      .trim();
    return out.length > 0 ? out : null;
  } catch {
    return null;
  }
}

/**
 * Reads `path` as a JSON object, `{}` when the file doesn't exist (a client
 * that has never had any MCP server configured), or `{}` for a genuinely
 * empty file. Throws (rather than silently discarding real content) on
 * anything that parses but isn't a plain object, or that doesn't parse at
 * all -- a client's own config can hold unrelated settings and other
 * `mcpServers` entries this must not clobber, so a config this can't safely
 * read is a reason to stop, not to overwrite.
 */
function readJsonObject(path: string): Record<string, unknown> {
  if (!existsSync(path)) return {};
  const raw = readFileSync(path, 'utf8').trim();
  if (raw === '') return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    throw new Error(
      `could not parse the existing MCP client config at ${path} as JSON: ${(e as Error).message}. ` +
        'Fix or remove it by hand, then re-run.'
    );
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(
      `expected a JSON object at ${path}, got ${Array.isArray(parsed) ? 'an array' : typeof parsed}. ` +
        'Fix or remove it by hand, then re-run.'
    );
  }
  return parsed as Record<string, unknown>;
}

/**
 * Writes `obj` to `path` via a temp file in the same directory followed by
 * an atomic rename, so a client reading `path` mid-write (or a crash
 * partway through) never sees truncated/partial JSON -- the same
 * crash-safety shape `packages/daemon`'s `write_config_atomic` uses for
 * `daemon.toml`, at the rigor this file's own risk warrants (a client
 * config, not a file carrying API keys, so no owner-only permissions step).
 */
function writeJsonObjectAtomic(path: string, obj: Record<string, unknown>): void {
  mkdirSync(dirname(path), { recursive: true });
  const tmpPath = `${path}.tmp-${process.pid}-${Date.now()}`;
  writeFileSync(tmpPath, JSON.stringify(obj, null, 2) + '\n', 'utf8');
  renameSync(tmpPath, path);
}

function mcpServersOf(config: Record<string, unknown>): Record<string, unknown> {
  const existing = config.mcpServers;
  return existing && typeof existing === 'object' && !Array.isArray(existing)
    ? (existing as Record<string, unknown>)
    : {};
}

/**
 * Configures each detected (or specified) client's `mcpServers` entry to
 * launch `nodespace mcp`, preserving every other key in the client's config
 * (including other MCP servers already registered there) untouched. Skips
 * (with a reason, not a thrown error) a client whose config can't be safely
 * read, or when `nodespace`'s absolute path can't be resolved -- writing a
 * config entry that can never actually launch would be worse than not
 * writing one.
 */
export function installMcp(
  targetClients?: McpClientName[],
  nodespacePath: string | null = resolveNodespaceBinaryPath()
): McpInstallResult[] {
  const clients = targetClients ?? detectMcpClients();
  const results: McpInstallResult[] = [];

  for (const name of clients) {
    const config = MCP_CLIENTS.find(c => c.name === name);
    if (!config) continue;

    if (!nodespacePath) {
      results.push({
        client: name,
        installed: false,
        configPath: config.configPath,
        skipReason: '`nodespace` was not found on $PATH -- install it first, then re-run',
      });
      continue;
    }

    let existing: Record<string, unknown>;
    try {
      existing = readJsonObject(config.configPath);
    } catch (e) {
      results.push({
        client: name,
        installed: false,
        configPath: config.configPath,
        skipReason: (e as Error).message,
      });
      continue;
    }

    const mcpServers = mcpServersOf(existing);
    mcpServers[config.serverKey] = { command: nodespacePath, args: ['mcp'] };
    existing.mcpServers = mcpServers;
    writeJsonObjectAtomic(config.configPath, existing);
    results.push({ client: name, installed: true, configPath: config.configPath });
  }

  return results;
}

/**
 * Removes this package's entry from each detected (or specified) client's
 * `mcpServers`, leaving every other key -- including other MCP servers --
 * untouched. A client with no config file, or a config with no entry under
 * this package's key, reports `removed: false` with no error: there was
 * nothing to remove, not a failure.
 */
export function uninstallMcp(targetClients?: McpClientName[]): McpUninstallResult[] {
  const clients = targetClients ?? MCP_CLIENTS.map(c => c.name);
  const results: McpUninstallResult[] = [];

  for (const name of clients) {
    const config = MCP_CLIENTS.find(c => c.name === name);
    if (!config) continue;

    if (!existsSync(config.configPath)) {
      results.push({ client: name, removed: false, configPath: config.configPath });
      continue;
    }

    let existing: Record<string, unknown>;
    try {
      existing = readJsonObject(config.configPath);
    } catch (e) {
      results.push({
        client: name,
        removed: false,
        configPath: config.configPath,
        skipReason: (e as Error).message,
      });
      continue;
    }

    const mcpServers = mcpServersOf(existing);
    if (!(config.serverKey in mcpServers)) {
      results.push({ client: name, removed: false, configPath: config.configPath });
      continue;
    }

    delete mcpServers[config.serverKey];
    existing.mcpServers = mcpServers;
    writeJsonObjectAtomic(config.configPath, existing);
    results.push({ client: name, removed: true, configPath: config.configPath });
  }

  return results;
}

/**
 * Which of `targetClients` (or every configured client, if omitted)
 * currently have an `mcpServers` entry under this package's key -- a pure
 * read, no mutation. A config that fails to parse reads as "not present"
 * rather than throwing: status reporting must not itself crash on a
 * malformed file another tool wrote.
 */
export function checkMcpInstalled(targetClients?: McpClientName[]): McpClientName[] {
  const clients = targetClients ?? MCP_CLIENTS.map(c => c.name);
  return clients.filter(name => {
    const config = MCP_CLIENTS.find(c => c.name === name);
    if (!config || !existsSync(config.configPath)) return false;
    try {
      const existing = readJsonObject(config.configPath);
      return config.serverKey in mcpServersOf(existing);
    } catch {
      return false;
    }
  });
}
