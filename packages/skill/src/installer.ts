import {
  existsSync,
  mkdirSync,
  rmSync,
  rmdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
  lstatSync,
  realpathSync,
} from 'node:fs';
import { join, dirname, basename, relative, resolve, isAbsolute, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { AGENTS } from './agents.js';
import type { AgentConfig, AgentName, InstallResult, UninstallResult } from './types.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
// Walk up past dist/ if running from compiled output; src/ stays at package root.
const PACKAGE_ROOT = join(__dirname, '..');

/** The directory, under both the package root and an install directory, that holds the skill's reference files. */
const REFERENCES_DIR = 'references';

/**
 * Written into each agent's install directory by `install`: the exact list of
 * files that install put there, as `{ "files": [<paths relative to the install
 * directory>] }`. Uninstall removes what it names, and a later install removes
 * any file it names that the new skill no longer ships, so neither has to guess
 * from a hand-kept list which files are ours.
 *
 * Agent harnesses discover a skill by its `SKILL.md`, so this dotfile is inert
 * to them.
 */
export const INSTALL_RECORD = '.nodespace-install.json';

/**
 * The reference files the installer wrote before it kept a record, when each
 * agent's `shims` named them one by one. An install directory with no record
 * is treated as holding these (plus its `SKILL.md` and harness shim), so
 * uninstalling, or reinstalling from a skill that has dropped one, still
 * cleans them up.
 *
 * It covers only installs made before the record existed, and can be deleted
 * once those are gone.
 */
export const PRE_RECORD_REFERENCES = [
  'references/cli.md',
  'references/graph-authored-guidance.md',
] as const;

/**
 * The reference files of the skill at `packageRoot`: every `*.md` directly
 * under its `references/` directory, as sorted paths relative to the package
 * root (`references/cli.md`), which is also where each installs inside an
 * install directory.
 *
 * The install and the public skill repository both take their reference list
 * from here, so a reference is added or dropped by changing that directory
 * alone. Not recursive: only Markdown files sit directly in `references/`, and
 * anything else there is not part of the skill. A package root with no
 * `references/` directory has none.
 */
export function listReferenceFiles(packageRoot: string): string[] {
  let entries;
  try {
    entries = readdirSync(join(packageRoot, REFERENCES_DIR), { withFileTypes: true });
  } catch (err) {
    const code = (err as NodeJS.ErrnoException).code;
    if (code === 'ENOENT' || code === 'ENOTDIR') return [];
    // Anything else (permissions, I/O) must not read as "the skill has no
    // references": install would then delete the ones it installed earlier.
    throw err;
  }
  return entries
    .filter(entry => entry.isFile() && entry.name.endsWith('.md'))
    .map(entry => `${REFERENCES_DIR}/${entry.name}`)
    .sort();
}

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

/**
 * Installs the skill at `packageRoot` into `targetAgents` (or every detected
 * agent): `SKILL.md`, the agent's harness shim, and every `references/*.md`
 * the package root ships.
 *
 * Each agent's install directory gets a record (`INSTALL_RECORD`) of exactly
 * what was written, and a file an earlier install put there that this skill no
 * longer ships is deleted. An install directory with no record is read as
 * holding what the installer wrote before records existed (`preRecordFiles`).
 */
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
      uninstall(['claude-code'], packageRoot);
      results.push({ agent: agentName, installed: [], changed: false, skipReason: 'plugin-managed' });
      continue;
    }

    // What the skill is made of: the agent's shims (flat, under their
    // basename) plus every reference file the package root ships. Each entry
    // is `[source path, path inside the install directory]`.
    const root = resolve(config.installDir);
    const present = [
      ...config.shims.map((shim): [string, string] => [join(packageRoot, shim), basename(shim)]),
      ...listReferenceFiles(packageRoot).map((ref): [string, string] => [join(packageRoot, ref), ref]),
    ].filter(([src]) => existsSync(src));

    // A skill is discovered by its SKILL.md, so a package without one is
    // broken, not a skill that has shrunk: install nothing, and leave whatever
    // is already installed, and its record, exactly as it is.
    if (!present.some(([, rel]) => rel === 'SKILL.md')) {
      results.push({ agent: agentName, installed: [], changed: false });
      continue;
    }

    const installed: string[] = [];
    const wrote: string[] = [];
    // Whether this run altered anything on disk. A file whose content is
    // already what this skill ships is left alone, so a re-run over a current
    // install changes nothing and is reported as such.
    let changed = false;
    for (const [src, rel] of present) {
      const dest = join(root, rel);
      mkdirSync(dirname(dest), { recursive: true });
      // SKILL.md gets the agent's frontmatter prepended — a skill is
      // discovered by its YAML `name` + `description` under the Agent Skills
      // standard; everything else is copied verbatim.
      const content = config.skillFrontmatter && rel === 'SKILL.md'
        ? Buffer.from(config.skillFrontmatter + '\n' + readFileSync(src, 'utf8'), 'utf8')
        : readFileSync(src);
      if (writeIfDifferent(dest, content)) changed = true;
      installed.push(dest);
      wrote.push(rel);
    }

    // Files an earlier install put here that this skill no longer ships. They
    // are compared by resolved path, so an oddly spelled entry naming a file
    // just written (`./SKILL.md`) is never mistaken for a stale one.
    const previous = readInstallRecord(root) ?? preRecordFiles(config);
    const keep = new Set(wrote.map(rel => resolve(root, rel)));
    const undeleted: string[] = [];
    for (const rel of previous) {
      if (keep.has(resolve(root, rel))) continue;
      try {
        if (removeRecordedFile(root, rel) !== undefined) changed = true;
      } catch (err) {
        // Read-only directory, locked file. Keep it in the record so the next
        // install (or an uninstall) still knows it is ours, and carry on: one
        // stubborn file must not abort this agent's install or the others'.
        undeleted.push(rel);
        const reason = (err as NodeJS.ErrnoException).code ?? (err as Error).message;
        process.stderr.write(
          `WARNING: could not remove ${resolve(root, rel)} (${reason}); it stays in the install record.\n`,
        );
      }
    }
    writeInstallRecord(root, [...wrote, ...undeleted]);

    results.push({ agent: agentName, installed, changed });
  }

  return results;
}

/**
 * Writes `content` to `dest` unless the file already holds exactly that, and
 * returns whether it wrote. A `dest` that cannot be read (missing, or not a
 * file) is written.
 */
function writeIfDifferent(dest: string, content: Buffer): boolean {
  try {
    if (readFileSync(dest).equals(content)) return false;
  } catch {
    // Missing or unreadable: fall through to the write, which reports a real
    // problem (a directory in the way, no permission) itself.
  }
  writeFileSync(dest, content);
  return true;
}

/**
 * What the installer put in `installDir`, as recorded by the install that put
 * it there, or `undefined` when there is no usable record: none written (an
 * install from before the record existed), or one that is unreadable or not
 * the `{ "files": [...] }` shape. Entries that are not strings are dropped.
 */
function readInstallRecord(installDir: string): string[] | undefined {
  try {
    const parsed: unknown = JSON.parse(readFileSync(join(installDir, INSTALL_RECORD), 'utf8'));
    const files = (parsed as { files?: unknown } | null)?.files;
    if (!Array.isArray(files)) return undefined;
    return files.filter((file): file is string => typeof file === 'string');
  } catch {
    return undefined;
  }
}

function writeInstallRecord(installDir: string, files: string[]): void {
  const record = { files: [...files].sort() };
  writeIfDifferent(
    join(installDir, INSTALL_RECORD),
    Buffer.from(JSON.stringify(record, null, 2) + '\n', 'utf8'),
  );
}

/**
 * The files the installer wrote before it kept a record: `SKILL.md`, the
 * agent's harness shim and the references in `PRE_RECORD_REFERENCES`.
 */
function preRecordFiles(config: AgentConfig): string[] {
  return [...config.shims.map(shim => basename(shim)), ...PRE_RECORD_REFERENCES];
}

/**
 * What an uninstall removes from an install directory that has no record: the
 * pre-record files, plus every reference the skill being uninstalled ships
 * (`packageRoot`), which may name ones the pre-record list does not.
 *
 * A `references/` that cannot be read is treated as shipping none, with a
 * warning: the pre-record list is still worth removing, and an uninstall should
 * not abort over the state of a directory it only reads.
 */
function filesWithoutRecord(config: AgentConfig, packageRoot: string): string[] {
  let shipped: string[] = [];
  try {
    shipped = listReferenceFiles(packageRoot);
  } catch (err) {
    const reason = (err as NodeJS.ErrnoException).code ?? (err as Error).message;
    process.stderr.write(
      `WARNING: could not read the skill's references (${reason}); ` +
      'any it ships that the pre-record list does not name are left in place.\n',
    );
  }
  return [...new Set([...preRecordFiles(config), ...shipped])];
}

/** Whether `path` is `root` or lies inside it, comparing the paths as written. */
function isWithin(root: string, path: string): boolean {
  const rel = relative(root, path);
  return rel !== '..' && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

/**
 * Deletes `rel` (relative to `installDir`) if, and only if, it is a regular
 * file or a symlink sitting in a directory that really lies inside
 * `installDir`, and returns the path it removed. Anything else is ignored and
 * returns `undefined`: a file that is already gone, a directory, an absolute
 * path or one that climbs out with `..`, or a file reached through a symlinked
 * directory that leads outside.
 *
 * A symlink entry is unlinked, never followed, so it cannot reach whatever it
 * points at: a user who links the skill's files in from elsewhere loses the
 * link on uninstall, not the file behind it.
 *
 * `rel` comes from a record on disk, which is only as trustworthy as that file:
 * the delete must never reach outside the directory the installer owns, however
 * the entry is spelled.
 */
function removeInstalledFile(installDir: string, rel: string): string | undefined {
  const target = resolve(installDir, rel);
  try {
    // lstat, not stat: a symlink is judged as itself, never by what it points at.
    const stat = lstatSync(target);
    if (!stat.isFile() && !stat.isSymbolicLink()) return undefined;
    // Real paths, so a symlinked parent directory cannot redirect the delete,
    // and an absolute or `..` entry lands outside the real install directory.
    if (!isWithin(realpathSync(installDir), realpathSync(dirname(target)))) return undefined;
  } catch {
    // Missing, unreadable or malformed: nothing this installer can show it wrote.
    return undefined;
  }
  rmSync(target);
  return target;
}

/**
 * `removeInstalledFile`, then prunes the directories above `rel` that are left
 * empty. That runs even when the file was already gone, so a user who deleted
 * every recorded file from `references/` by hand does not leave an empty
 * `references/` (and with it the install directory) behind.
 */
function removeRecordedFile(installDir: string, rel: string): string | undefined {
  const removed = removeInstalledFile(installDir, rel);
  pruneEmptyParents(installDir, resolve(installDir, rel));
  return removed;
}

/**
 * Removes the directories left empty by deleting `file`, walking up from its
 * parent to (never including) `installDir`, and stops at the first that still
 * holds anything. `rmdirSync` refuses a non-empty directory, so a user's own
 * file, or a directory they made, is never touched: uninstall must not delete
 * more than it installed, and leaving something behind is the smaller failure.
 */
function pruneEmptyParents(installDir: string, file: string): void {
  for (let dir = dirname(file); dir !== installDir && isWithin(installDir, dir); dir = dirname(dir)) {
    try {
      // A symlinked directory is the user's link, not something this installer
      // made, and never its to remove: on Windows rmdir would take the link
      // itself even though the directory it points at is not empty.
      if (lstatSync(dir).isSymbolicLink()) return;
      rmdirSync(dir);
    } catch {
      // Not empty, or unreadable. Either way, the directories above it stay too.
      return;
    }
  }
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

/**
 * Removes the skill from `targetAgents` (or every configured agent).
 *
 * With a record, removes exactly the files it names and the record itself.
 * Without one, the install predates the record: it removes the agent's shims,
 * every reference file `packageRoot` ships and `PRE_RECORD_REFERENCES`. Either
 * way it then prunes only the directories those files leave empty, and the
 * install directory itself once nothing is left in it.
 *
 * `packageRoot` is where the skill being uninstalled came from (the staged
 * resource root when the app runs a compiled installer). It matters only for an
 * install with no record, whose reference files it names.
 */
export function uninstall(targetAgents?: AgentName[], packageRoot = PACKAGE_ROOT): UninstallResult[] {
  const agents = targetAgents ?? AGENTS.map(a => a.name);
  const results: UninstallResult[] = [];

  for (const agentName of agents) {
    const config = AGENTS.find(a => a.name === agentName);
    if (!config || !existsSync(config.installDir)) continue;

    const root = resolve(config.installDir);
    const files = readInstallRecord(root) ?? filesWithoutRecord(config, packageRoot);

    const removed: string[] = [];
    for (const rel of files) {
      const path = removeRecordedFile(root, rel);
      if (path !== undefined) removed.push(path);
    }
    // Last, so an uninstall interrupted part-way can be run again.
    removeInstalledFile(root, INSTALL_RECORD);

    if (readdirSync(root).length === 0) {
      try {
        rmdirSync(root);
        // Install makes any missing directory between the harness's own and
        // the install directory (`skills/`). One this leaves empty goes too,
        // whoever made it: nothing records which were ours, and an empty one
        // holds nothing to lose. The harness's own directory is never touched.
        pruneEmptyParents(resolve(config.detectionDir), root);
      } catch {
        // A symlinked install directory (a dotfile manager's link) cannot be
        // rmdir'd. Leaving the empty directory is the smaller failure than
        // aborting the agents after this one.
      }
    }

    results.push({ agent: agentName, removed });
  }

  return results;
}
