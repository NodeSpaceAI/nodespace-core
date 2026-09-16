import { existsSync, mkdirSync, copyFileSync, rmSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { AGENTS } from './agents.js';
import type { AgentName, InstallResult, UninstallResult } from './types.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
// Walk up past dist/ if running from compiled output; src/ stays at package root.
const PACKAGE_ROOT = join(__dirname, '..');

/**
 * Which configured agents are present on this machine, by the existence of
 * their `detectionDir` alone -- the same check `install` uses to decide what
 * to target, exported so a caller can ask that question *before* installing
 * anything. `checkInstalled` is the deliberately different question ("which
 * already have SKILL.md on disk"), and answers it only after an install has
 * happened; neither substitutes for the other.
 */
export function detectAgents(): AgentName[] {
  return AGENTS
    .filter(agent => existsSync(agent.detectionDir))
    .map(agent => agent.name);
}

/**
 * Check whether `nodespace` resolves on $PATH by running `nodespace --version`.
 * Returns true if the binary is found and exits 0, false otherwise.
 * Safe to call without a running daemon — `--version` is handled by clap
 * before any socket connection is attempted.
 */
export function isNodespaceBinaryOnPath(): boolean {
  try {
    execFileSync('nodespace', ['--version'], { stdio: 'ignore', timeout: 3000 });
    return true;
  } catch {
    return false;
  }
}

/**
 * Whether the NodeSpace skill is already installed for Claude Code via its
 * plugin marketplace (`/plugin install nodespace@<marketplace>`), rather
 * than by this installer writing plain files to
 * `<claudeConfigDir>/skills/nodespace/`.
 *
 * `claudeConfigDir` is the SAME resolved directory `agents.ts`'s
 * `claude-code` entry uses (honors `$CLAUDE_CONFIG_DIR`), not a hardcoded
 * `~/.claude` -- the plugin registry lives alongside it either way.
 *
 * Matches on the plugin-name half of the `<plugin-name>@<marketplace-name>`
 * key only, not a specific marketplace name -- the marketplace side is
 * whatever label the user's Claude Code registered it under locally, which
 * this installer has no way to predict.
 */
export function claudeCodePluginManagedSkillExists(claudeConfigDir: string): boolean {
  const registryPath = join(claudeConfigDir, 'plugins', 'installed_plugins.json');
  if (!existsSync(registryPath)) return false;
  try {
    const registry = JSON.parse(readFileSync(registryPath, 'utf8')) as {
      plugins?: Record<string, unknown>;
    };
    return Object.keys(registry.plugins ?? {}).some(key => key.split('@')[0] === 'nodespace');
  } catch {
    // A malformed or unreadable registry must not block installation --
    // fail open (treat as "no plugin-managed copy found"), not closed.
    return false;
  }
}

export function install(targetAgents?: AgentName[], packageRoot = PACKAGE_ROOT): InstallResult[] {
  if (!isNodespaceBinaryOnPath()) {
    process.stderr.write(
      'WARNING: `nodespace` is not on $PATH. The skill will be installed, but the CLI\n' +
      'must be installed and on $PATH before agents can use NodeSpace.\n' +
      'Install it with `curl -fsSL https://nodespace.ai/install.sh | sh`, via the\n' +
      'NodeSpace DMG, or `brew install --cask nodespaceai/nodespace/nodespace`.\n',
    );
  }

  const detected = targetAgents ?? detectAgents();
  const results: InstallResult[] = [];

  for (const agentName of detected) {
    const config = AGENTS.find(a => a.name === agentName);
    if (!config) continue;

    // Claude Code's own plugin marketplace (`/plugin install nodespace@...`)
    // is a separate, self-updating install path for the same skill. When a
    // plugin-managed copy is already registered, it stays authoritative —
    // writing files here would create a second, divergent copy Claude Code
    // sees twice. Applies to claude-code only: the other harnesses have no
    // marketplace of their own.
    if (agentName === 'claude-code' && claudeCodePluginManagedSkillExists(config.detectionDir)) {
      // A copy from before this reconciliation existed may already sit at
      // our own installDir (from an earlier app-install run) — clean it up
      // so there is truly one copy, not just "no new copy going forward".
      // Reuses uninstall()'s own directory-pruning logic rather than
      // duplicating it; a no-op when nothing is there.
      uninstall(['claude-code']);
      results.push({ agent: agentName, installed: [], skipReason: 'plugin-managed' });
      continue;
    }

    const installed: string[] = [];
    for (const shim of config.shims) {
      const src = join(packageRoot, shim);
      const dest = join(config.installDir, installedName(shim));
      if (existsSync(src)) {
        mkdirSync(dirname(dest), { recursive: true });
        // SKILL.md gets the agent's frontmatter prepended — a skill is
        // discovered by its YAML `name` + `description` under the Agent Skills
        // standard; everything else is copied verbatim.
        if (config.skillFrontmatter && basename(shim) === 'SKILL.md') {
          writeFileSync(dest, config.skillFrontmatter + '\n' + readFileSync(src, 'utf8'), 'utf8');
        } else {
          copyFileSync(src, dest);
        }
        installed.push(dest);
      }
    }

    results.push({ agent: agentName, installed });
  }

  return results;
}

/**
 * Where a source path lands inside the installed skill directory.
 *
 * Shims flatten to a basename (`shims/codex/x.ts` installs as `x.ts`), but
 * `references/` keeps its directory: SKILL.md links to `references/cli.md` by
 * relative path, so flattening it would leave the body pointing at a file that
 * isn't where it says.
 *
 * Install and uninstall MUST agree on this, which is why it is one function
 * rather than the same expression written twice. When it was duplicated, only
 * the install side learned about `references/` — so uninstall looked for a
 * `cli.md` that never existed, left the real file behind, and the
 * "directory is now empty" check then preserved a folder containing a
 * reference and no SKILL.md: a malformed skill in the harness's scan path,
 * created by the cleanup command itself.
 */
function installedName(shim: string): string {
  return shim.startsWith('references/') ? shim : basename(shim);
}

/**
 * Which of `targetAgents` (or every configured agent, if omitted) actually
 * have `SKILL.md` sitting at their `installDir` right now -- a pure
 * filesystem check, no mutation. Used to revalidate a persisted
 * `agents_installed` list against reality: the list is only ever written by
 * a successful install, so it goes stale the moment a user manually deletes
 * a harness's skill directory (or the harness itself) by hand.
 *
 * Checks for `SKILL.md` specifically, not just `existsSync(installDir)` --
 * an empty or partially-cleaned directory must not read as "installed".
 */
export function checkInstalled(targetAgents?: AgentName[]): AgentName[] {
  const agents = targetAgents ?? AGENTS.map(a => a.name);
  return agents.filter(agentName => {
    const config = AGENTS.find(a => a.name === agentName);
    return config !== undefined && existsSync(join(config.installDir, 'SKILL.md'));
  });
}

export function uninstall(targetAgents?: AgentName[]): UninstallResult[] {
  const agents = targetAgents ?? AGENTS.map(a => a.name);
  const results: UninstallResult[] = [];

  for (const agentName of agents) {
    const config = AGENTS.find(a => a.name === agentName);
    if (!config || !existsSync(config.installDir)) continue;

    const removed: string[] = [];
    for (const shim of config.shims) {
      const dest = join(config.installDir, installedName(shim));
      if (existsSync(dest)) {
        rmSync(dest);
        removed.push(dest);
      }
    }

    // Removing `references/cli.md` leaves an empty `references/` behind, which
    // would make the directory look non-empty and strand the whole folder.
    //
    // Prune ONLY directories this installer created, derived from `shims` —
    // never "any empty directory". Uninstall must not reach outside what it
    // installed: a user's own empty folder here is not ours to delete, and
    // removing it would also empty the parent and take the whole install
    // directory with it, silently and without reporting any of it in
    // `removed`. Deleting more than we installed is a worse failure than
    // leaving something behind.
    //
    // This prunes the immediate parent only, which covers every shim today
    // (`references/` is the one nested case). A deeper shim such as
    // `references/api/cli.md` would prune `references/api` and strand
    // `references/` — reviving the stranded-directory bug this exists to
    // prevent. Prune leaf-upward if such a shim is ever added.
    const ownedDirs = new Set(
      config.shims
        .map(shim => dirname(installedName(shim)))
        .filter(dir => dir !== '.' && dir !== '')
    );
    for (const dir of ownedDirs) {
      const subdir = join(config.installDir, dir);
      // An unreadable directory (bad permissions) must not abort the uninstall
      // for this agent or the ones after it — leaving a stale directory is a
      // far smaller problem than skipping every remaining agent's cleanup.
      try {
        if (existsSync(subdir) && readdirSync(subdir).length === 0) {
          rmSync(subdir, { recursive: true });
        }
      } catch {
        // Leave it in place; the is-empty check below then keeps the install
        // directory too, which is the correct conservative outcome.
      }
    }

    const remaining = readdirSync(config.installDir);
    if (remaining.length === 0) {
      rmSync(config.installDir, { recursive: true });
    }

    results.push({ agent: agentName, removed });
  }

  return results;
}
