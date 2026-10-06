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
import { join, dirname, relative, resolve, isAbsolute, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { AGENTS, SHARED_PLUGIN_DIR } from './agents.js';
import { hasBlock, removeBlock, upsertBlock } from './instructions-block.js';
import type {
  AgentConfig,
  AgentName,
  InstallResult,
  IntegrationStatus,
  UninstallResult,
} from './types.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
// Walk up past dist/ if running from compiled output; src/ stays at package root.
const PACKAGE_ROOT = join(__dirname, '..');

/** The directory, under both the package root and an install directory, that holds the skill's reference files. */
const REFERENCES_DIR = 'references';

/**
 * Written into each agent's install directory by `install`: exactly what that
 * install put on this machine for the agent.
 *
 * - `files`: paths relative to the install directory.
 * - `plugin_files`: the harness plugin's files, relative to the folder the
 *   harness loads it from, when that is not the install directory.
 * - `instructions_file`: the file the instructions block was written into.
 *
 * Uninstall removes what it names, and a later install removes any file it
 * names that the new skill no longer ships, so neither has to guess from a
 * hand-kept list which files are ours.
 *
 * Agent harnesses discover a skill by its `SKILL.md`, so this dotfile is inert
 * to them.
 */
export const INSTALL_RECORD = '.nodespace-install.json';

/**
 * The reference files the installer wrote before it kept a record. An install
 * directory with no record is treated as holding these (plus its `SKILL.md`), so
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

/** The skill file every agent gets, at the root of its install directory. */
const SKILL_FILE = 'SKILL.md';

/** What an install directory with no record is read as holding. */
const PRE_RECORD_FILES: readonly string[] = [SKILL_FILE, ...PRE_RECORD_REFERENCES];

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
 * agent): `SKILL.md`, every `references/*.md` the package root ships, the
 * agent's harness plugin where it has one (ADR-093 §5), and otherwise the
 * instructions block in the harness's instructions file (§6).
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

    // What the skill is made of: `SKILL.md`, the agent's harness plugin where
    // the harness loads one from its skill folder, and every reference file
    // the package root ships. Each entry is `[source path, path inside the
    // install directory]`.
    const root = resolve(config.installDir);
    // A plugin is installed whole or not at all: a manifest without the
    // module its hooks file names is a plugin the harness cannot load.
    const pluginRoot = pluginRootOf(config);
    const pluginEntries = pluginEntriesOf(config, packageRoot);
    const plugin = pluginEntries.every(([src]) => existsSync(src)) ? pluginEntries : [];
    const present = [
      [join(packageRoot, SKILL_FILE), SKILL_FILE] as [string, string],
      ...(pluginRoot === undefined ? plugin : []),
      ...listReferenceFiles(packageRoot).map((ref): [string, string] => [join(packageRoot, ref), ref]),
    ].filter(([src]) => existsSync(src));

    // A skill is discovered by its SKILL.md, so a package without one is
    // broken, not a skill that has shrunk: install nothing, and leave whatever
    // is already installed, and its record, exactly as it is.
    if (!present.some(([, rel]) => rel === SKILL_FILE)) {
      results.push({ agent: agentName, installed: [], changed: false });
      continue;
    }

    const previous = readInstallRecord(root);
    // SKILL.md gets the agent's frontmatter prepended — a skill is discovered
    // by its YAML `name` + `description` under the Agent Skills standard;
    // everything else is copied verbatim.
    const skill = syncFiles(root, present, previous?.files ?? PRE_RECORD_FILES, (rel, content) =>
      config.skillFrontmatter && rel === SKILL_FILE
        ? Buffer.from(config.skillFrontmatter + '\n' + content.toString('utf8'), 'utf8')
        : content,
    );
    // The plugin of a harness that loads it from a folder of its own. That
    // folder may be one the user keeps plugins of their own in, so a file
    // already there that no install of ours recorded is not ours to replace.
    const recordedPlugin = previous?.pluginFiles ?? [];
    const foreign = pluginRoot === undefined ? [] : foreignFiles(pluginRoot, plugin, recordedPlugin);
    for (const file of foreign) {
      process.stderr.write(
        `WARNING: ${file} exists and was not installed by NodeSpace; the ${agentName} plugin was not installed. ` +
        'Move the file away and install again.\n',
      );
    }
    const loaded = pluginRoot === undefined
      ? undefined
      : foreign.length > 0
        ? { installed: [], recorded: recordedPlugin, changed: false }
        : syncFiles(pluginRoot, plugin, recordedPlugin);
    const block = syncInstructionsBlock(config.instructionsFile, previous?.instructionsFile);

    writeInstallRecord(root, {
      files: skill.recorded,
      pluginFiles: loaded?.recorded ?? [],
      instructionsFile: block.file,
    });

    results.push({
      agent: agentName,
      installed: [...skill.installed, ...(loaded?.installed ?? []), ...(block.file ? [block.file] : [])],
      changed: skill.changed || (loaded?.changed ?? false) || block.changed,
    });
  }

  return results;
}

/** The folder a harness loads its plugin from, when that is not the skill folder. */
function pluginRootOf(config: AgentConfig): string | undefined {
  return config.plugin?.installDir === undefined ? undefined : resolve(config.plugin.installDir);
}

/** A harness plugin's files, each as `[source path, path inside the folder it installs into]`. */
function pluginEntriesOf(config: AgentConfig, packageRoot: string): Array<[string, string]> {
  const plugin = config.plugin;
  if (!plugin) return [];
  return [
    ...plugin.files.map((file): [string, string] => [join(packageRoot, plugin.dir, file), file]),
    ...(plugin.shared ?? []).map(([file, rel]): [string, string] => [join(packageRoot, SHARED_PLUGIN_DIR, file), rel]),
  ];
}

/**
 * The files among `entries` (`[source path, path inside root]`) that already
 * exist in `root` without an earlier install having recorded them (`recorded`)
 * and without holding what this skill ships: someone else's.
 */
function foreignFiles(root: string, entries: Array<[string, string]>, recorded: readonly string[]): string[] {
  const ours = new Set(recorded.map(rel => resolve(root, rel)));
  return entries
    .map(([src, rel]): [string, string] => [src, resolve(root, rel)])
    .filter(([src, dest]) => !ours.has(dest) && existsSync(dest) && !sameBytes(src, dest))
    .map(([, dest]) => dest);
}

function sameBytes(a: string, b: string): boolean {
  try {
    return readFileSync(a).equals(readFileSync(b));
  } catch {
    return false;
  }
}

/**
 * A file's text, when the file is UTF-8 that reads and writes back to the same
 * bytes; `''` for a file that does not exist. Throws for any other: rewriting
 * such a file would alter the user's own text along with the block.
 */
function readRoundTrippable(file: string): string {
  let bytes: Buffer;
  try {
    bytes = readFileSync(file);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === 'ENOENT') return '';
    throw err;
  }
  const text = bytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(bytes)) throw new Error('it is not UTF-8 text');
  return text;
}

/**
 * Makes `root` hold `entries` (`[source path, path inside root]`), and removes
 * each of `previous`, the files an earlier install recorded there, that is not
 * among them. `recorded` is what the next record names: the files written,
 * and any stale one that could not be removed.
 *
 * A file whose content is already what this skill ships is left alone, so a
 * re-run over a current install changes nothing and is reported as such.
 */
function syncFiles(
  root: string,
  entries: Array<[string, string]>,
  previous: readonly string[],
  transform: (rel: string, content: Buffer) => Buffer = (_rel, content) => content,
): { installed: string[]; recorded: string[]; changed: boolean } {
  const installed: string[] = [];
  const wrote: string[] = [];
  let changed = false;
  for (const [src, rel] of entries) {
    const dest = join(root, rel);
    mkdirSync(dirname(dest), { recursive: true });
    if (writeIfDifferent(dest, transform(rel, readFileSync(src)))) changed = true;
    installed.push(dest);
    wrote.push(rel);
  }

  // Files an earlier install put here that this skill no longer ships. They
  // are compared by resolved path, so an oddly spelled entry naming a file
  // just written (`./SKILL.md`) is never mistaken for a stale one.
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
  return { installed, recorded: [...wrote, ...undeleted], changed };
}

/**
 * Puts the instructions block into `file` (ADR-093 §6): replaced where it
 * already is, appended otherwise, in a file created when absent. A block an
 * earlier install wrote into a different file (`previous`) is taken out of
 * that one. `file` in the result is the file that now holds the block.
 *
 * A file that cannot be read or written, or that is not UTF-8 text, is left
 * alone with a warning: the skill itself is installed either way. The file is
 * written in place, not replaced, so one that is a link stays a link.
 */
function syncInstructionsBlock(
  file: string | undefined,
  previous: string | undefined,
): { file: string | undefined; changed: boolean } {
  let changed = false;
  if (previous !== undefined && (file === undefined || resolve(previous) !== resolve(file))) {
    changed = removeInstructionsBlock(previous);
  }
  if (file === undefined) return { file: undefined, changed };
  try {
    const content = readRoundTrippable(file);
    mkdirSync(dirname(file), { recursive: true });
    if (writeIfDifferent(file, Buffer.from(upsertBlock(content), 'utf8'))) changed = true;
    return { file, changed };
  } catch (err) {
    const reason = (err as NodeJS.ErrnoException).code ?? (err as Error).message;
    process.stderr.write(`WARNING: could not write the NodeSpace instructions into ${file} (${reason}).\n`);
    // Still recorded when it already holds a block from an earlier install.
    return { file: fileHasBlock(file) ? file : undefined, changed };
  }
}

function fileHasBlock(file: string): boolean {
  try {
    return hasBlock(readFileSync(file, 'utf8'));
  } catch {
    return false;
  }
}

/**
 * Takes the instructions block out of `file`, leaving every other byte, and
 * returns whether it did. A file the block was the whole of is deleted: the
 * installer made it.
 */
function removeInstructionsBlock(file: string): boolean {
  try {
    if (!existsSync(file)) return false;
    const content = readRoundTrippable(file);
    if (!hasBlock(content)) return false;
    const rest = removeBlock(content);
    if (rest === '') rmSync(file);
    else writeFileSync(file, rest, 'utf8');
    return true;
  } catch (err) {
    const reason = (err as NodeJS.ErrnoException).code ?? (err as Error).message;
    process.stderr.write(`WARNING: could not remove the NodeSpace instructions from ${file} (${reason}).\n`);
    return false;
  }
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

/** An install record as read: see `INSTALL_RECORD`. */
type InstallRecord = { files: string[]; pluginFiles: string[]; instructionsFile: string | undefined };

/**
 * What the installer put on this machine for the agent whose install
 * directory is `installDir`, as recorded by the install that put it there, or
 * `undefined` when there is no usable record: none written (an install from
 * before the record existed), or one that is unreadable or not the
 * `{ "files": [...] }` shape. Entries that are not strings are dropped.
 */
function readInstallRecord(installDir: string): InstallRecord | undefined {
  try {
    const parsed: unknown = JSON.parse(readFileSync(join(installDir, INSTALL_RECORD), 'utf8'));
    const record = parsed as { files?: unknown; plugin_files?: unknown; instructions_file?: unknown } | null;
    if (!Array.isArray(record?.files)) return undefined;
    const strings = (value: unknown): string[] =>
      Array.isArray(value) ? value.filter((file): file is string => typeof file === 'string') : [];
    return {
      files: strings(record.files),
      pluginFiles: strings(record.plugin_files),
      instructionsFile: typeof record.instructions_file === 'string' ? record.instructions_file : undefined,
    };
  } catch {
    return undefined;
  }
}

function writeInstallRecord(installDir: string, record: InstallRecord): void {
  const written = {
    files: [...record.files].sort(),
    ...(record.pluginFiles.length > 0 ? { plugin_files: [...record.pluginFiles].sort() } : {}),
    ...(record.instructionsFile === undefined ? {} : { instructions_file: record.instructionsFile }),
  };
  writeIfDifferent(
    join(installDir, INSTALL_RECORD),
    Buffer.from(JSON.stringify(written, null, 2) + '\n', 'utf8'),
  );
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
function filesWithoutRecord(packageRoot: string): string[] {
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
  return [...new Set([...PRE_RECORD_FILES, ...shipped])];
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
 * What the agent has beyond the static skill, and whether it is in place: its
 * harness plugin (every file of it, where the harness loads it from), or the
 * instructions block in its instructions file. `undefined` for an agent that
 * gets neither. A pure filesystem check.
 */
export function integrationStatus(agentName: AgentName, packageRoot = PACKAGE_ROOT): IntegrationStatus | undefined {
  const config = AGENTS.find(a => a.name === agentName);
  if (config?.plugin) {
    const root = pluginRootOf(config) ?? resolve(config.installDir);
    const files = pluginEntriesOf(config, packageRoot).map(([, rel]) => join(root, rel));
    return { kind: 'plugin', installed: files.every(file => existsSync(file)) };
  }
  if (config?.instructionsFile !== undefined) {
    return { kind: 'instructions-block', installed: fileHasBlock(config.instructionsFile) };
  }
  return undefined;
}

/**
 * Removes the skill from `targetAgents` (or every configured agent): its
 * files, the harness plugin where that sits in a folder of its own, and the
 * instructions block.
 *
 * With a record, removes exactly the files it names and the record itself.
 * Without one, the install predates the record: it removes `SKILL.md`, every
 * reference file `packageRoot` ships and `PRE_RECORD_REFERENCES`, and each
 * plugin file that holds exactly what this skill ships. Either way it then prunes only the
 * directories those files leave empty, and the install directory itself once
 * nothing is left in it. Of an instructions file it removes the marked block
 * alone.
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
    if (!config) continue;

    const root = resolve(config.installDir);
    const hasInstallDir = existsSync(root);
    const record = hasInstallDir ? readInstallRecord(root) : undefined;
    const removed: string[] = [];

    if (hasInstallDir) {
      for (const rel of record?.files ?? filesWithoutRecord(packageRoot)) {
        const path = removeRecordedFile(root, rel);
        if (path !== undefined) removed.push(path);
      }
    }

    // The plugin, where the harness loads it from a folder of its own. It is
    // removed even when the skill folder is already gone: a plugin left behind
    // would go on running in every session.
    const pluginRoot = pluginRootOf(config);
    if (pluginRoot !== undefined && existsSync(pluginRoot)) {
      // With no record there is no list of what was installed. A file is then
      // ours only when it holds exactly what this skill ships: the folder may
      // be one the user keeps plugins of their own in, under any name.
      const shipped = pluginEntriesOf(config, packageRoot);
      const files = record?.pluginFiles ??
        shipped.filter(([src, rel]) => sameBytes(src, resolve(pluginRoot, rel))).map(([, rel]) => rel);
      if (!record) {
        // Said, not passed over: a plugin left in place runs in every session.
        for (const [, rel] of shipped) {
          const left = resolve(pluginRoot, rel);
          if (!files.includes(rel) && existsSync(left)) {
            process.stderr.write(
              `WARNING: ${left} was left in place: nothing records that NodeSpace installed it, ` +
              'and it is not the file this version ships. Remove it by hand if it is not yours.\n',
            );
          }
        }
      }
      for (const rel of files) {
        const path = removeRecordedFile(pluginRoot, rel);
        if (path !== undefined) removed.push(path);
      }
      removeIfEmpty(pluginRoot, config.detectionDir);
    }

    // The block, in the file the record names and the one the harness reads
    // now: they differ when the harness's home was moved since the install.
    const instructionsFiles = new Set(
      [record?.instructionsFile, config.instructionsFile]
        .filter((file): file is string => file !== undefined)
        .map(file => resolve(file)),
    );
    for (const file of instructionsFiles) {
      if (removeInstructionsBlock(file)) removed.push(file);
    }

    if (!hasInstallDir) {
      if (removed.length > 0) results.push({ agent: agentName, removed });
      continue;
    }

    // Last, so an uninstall interrupted part-way can be run again.
    removeInstalledFile(root, INSTALL_RECORD);
    removeIfEmpty(root, config.pruneRoot ?? config.detectionDir);

    results.push({ agent: agentName, removed });
  }

  return results;
}

/**
 * Removes `dir` once nothing is left in it, and then the directories between
 * it and the harness's own (`detectionDir`) that this leaves empty.
 *
 * Install makes any missing directory on the way (`skills/`,
 * `extensions/`). One left empty goes too, whoever made it: nothing records
 * which were ours, and an empty one holds nothing to lose. The harness's own
 * directory is never touched.
 */
function removeIfEmpty(dir: string, detectionDir: string): void {
  try {
    if (readdirSync(dir).length > 0) return;
    rmdirSync(dir);
    pruneEmptyParents(resolve(detectionDir), dir);
  } catch {
    // A symlinked directory (a dotfile manager's link) cannot be rmdir'd.
    // Leaving the empty directory is the smaller failure than aborting the
    // agents after this one.
  }
}
