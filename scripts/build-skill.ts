#!/usr/bin/env bun
/**
 * Build packages/skill and stage its runtime output where the desktop app
 * expects to find it.
 *
 * packages/skill ships two ways, both produced here (see
 * packages/desktop-app/app-lib/src/skill_setup.rs's `Installer` enum for
 * which one a given launch actually uses):
 *
 *   1. **The compiled standalone binary** (`bun build --compile`) — the
 *      preferred path: no external bun/node dependency at all, so a packaged
 *      app's end user never hits "runtime not found". Staged as
 *      `binaries/nodespace-skill-installer-{triple}`, declared as an
 *      `externalBin` sidecar in tauri.conf.json (same mechanism as
 *      `nodespaced`/`nodespace`).
 *   2. **The plain JS build** (`tsc` compiles src/ -> dist/, packages/skill/
 *      package.json's own `build` script) — the fallback for a platform this
 *      hasn't been wired up for yet, or a dev/source checkout. Needs `bun` or
 *      `node` on $PATH to run. Staged (dist/, plugins/, SKILL.md, references/,
 *      package.json for its `"type": "module"` marker) into
 *      packages/desktop-app/src-tauri/resources/skill/, a `resources` entry
 *      in tauri.conf.json — also where the compiled binary's
 *      `--resource-root` points, since it can't infer that from its own
 *      compiled-executable location the way dist/install.js can from
 *      `import.meta.url`.
 *
 * Tauri copies declared resources/externalBin into the build cache for
 * `tauri dev` too, so this single staging step serves both dev and packaged
 * builds. Run automatically before `dev:tauri` and `tauri:build` (see
 * packages/desktop-app/package.json) so neither is ever stale.
 *
 * The compiled binary is only produced for the CURRENT host here — this
 * script is for local development, same scope as `build-sidecars.ts`
 * (real cross-platform release builds compile natively on each platform's
 * own CI runner; see `.github/workflows/release.yml`).
 *
 * ## Why both halves are idempotent
 *
 * Everything this script writes is an input to `nodespace-app`'s build
 * script: `tauri-build` emits a `cargo:rerun-if-changed` for every
 * `externalBin` and every `resources` entry (`copy_binaries` /
 * `copy_resources` in `tauri-build-2.6.2/src/lib.rs`). So a single rewritten
 * byte — or merely a fresher mtime on an otherwise identical file —
 * invalidates the crate and forces a full rebuild. Since `build:skill` runs
 * automatically before `dev:tauri` and `tauri:build`, an unconditional
 * rewrite taxes every pass through the desktop dev loop even when nothing
 * under `packages/skill/` changed.
 *
 * Both halves therefore no-op when they would reproduce what is already
 * staged, with each check matched to what that half actually costs:
 *
 *   - The **compile** (`bun build --compile`, ~58MB) is guarded by mtime —
 *     the same comparison shape as `nodespace-app-build`'s `sync_stale_sidecar`
 *     — so the expensive step is skipped outright, not run and discarded.
 *   - The **resource staging** is guarded by content. `tsc` runs without
 *     `--incremental` here, so it rewrites every `dist/*.js` on each run with
 *     byte-identical output and a fresh mtime; an mtime check would see churn
 *     that isn't really there. Comparing bytes and writing only what actually
 *     differs leaves unchanged files' mtimes — and the crate — untouched.
 *
 * ## Adding guidance at build time (`NODESPACE_SKILL_EXTENSIONS`)
 *
 * A build that ships more agent guidance than core's own names a directory in
 * `NODESPACE_SKILL_EXTENSIONS` (ADR-082). With the variable unset or empty,
 * which is every core build and test run, the staged skill is exactly core's.
 * Set, the directory may hold a `SKILL.md` fragment, appended to the staged
 * `SKILL.md`, and `references/*.md`, staged beside core's references; anything
 * else in it, a reference named like one of core's, or a directory that does
 * not exist fails the build (`readSkillExtensions`). Dot-entries such as
 * `.DS_Store` are ignored. A relative path resolves against the directory the
 * build runs from, the repository root under `bun run build:skill`. The
 * installer installs and uninstalls an added reference like any of core's,
 * because it takes its list from what is staged.
 *
 * The additions are merged into what each entry stages, not copied after it.
 * Both halves of the staging above compare bytes and delete what the source no
 * longer holds, so a plain run after a run with additions removes them again,
 * and a repeated run of either kind rewrites nothing.
 *
 * ## Cross-compiling the binary (`--target <rust-triple>`)
 *
 * Every caller except one wants the compiled binary for `hostTriple()`
 * (the default). The exception is scripts/build-windows-quick.ts, which
 * cross-compiles nodespaced/nodespace for x86_64-pc-windows-msvc from a
 * macOS host via cargo-xwin — Tauri's externalBin lookup then needs a
 * matching `nodespace-skill-installer-x86_64-pc-windows-msvc.exe`, not the
 * macOS one `hostTriple()` would otherwise produce. `--target <rust-triple>`
 * overrides which triple this script stages for and, when it differs from
 * `hostTriple()`, adds Bun's own `--target=<bun-target>` flag (see
 * `bunCompileTarget`) to cross-compile the binary itself.
 */

import { $ } from 'bun';
import {
  chmodSync,
  copyFileSync,
  type Dirent,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, extname, join, relative, resolve, sep } from 'node:path';
import { arch, platform } from 'node:os';

const WORKSPACE_ROOT = join(import.meta.dir, '..');
const SKILL_DIR = join(WORKSPACE_ROOT, 'packages', 'skill');
const RESOURCE_DIR = join(
  WORKSPACE_ROOT,
  'packages',
  'desktop-app',
  'src-tauri',
  'resources',
  'skill',
);
const BIN_DIR = join(WORKSPACE_ROOT, 'packages', 'desktop-app', 'src-tauri', 'binaries');

/**
 * Every file under `root`, as paths relative to it. A `root` that does not
 * exist yields no files; a `root` that is itself a file yields the single
 * empty relative path, so a plain file and a directory tree can be fed to
 * the same sync loop. Directories themselves are never yielded, only the
 * files inside them.
 *
 * Symlinks are reported as files (the link is classified, not its target),
 * which is fine for the staged entries because none of them contain any —
 * a dangling one would go on to throw ENOENT from the caller's `statSync` or
 * `copyFileSync`. Supporting them is out of scope rather than overlooked.
 */
export function listFilesRecursive(root: string): string[] {
  if (!existsSync(root)) return [];
  if (!statSync(root).isDirectory()) return [''];
  const files: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const abs = join(dir, entry.name);
      if (entry.isDirectory()) walk(abs);
      else files.push(relative(root, abs));
    }
  };
  walk(root);
  return files;
}

/**
 * The most recent mtime across `paths` (files and/or directory trees), or
 * `null` if none of them exist. A missing path contributes nothing rather
 * than erroring: an input that isn't there can't have superseded anything.
 * `ignore` drops any relative path for which it returns true, so a caller can
 * exclude subtrees that don't actually feed the output.
 */
export function newestMtimeMs(
  paths: string[],
  ignore: (relativePath: string) => boolean = () => false,
): number | null {
  let newest: number | null = null;
  for (const path of paths) {
    if (!existsSync(path)) continue;
    for (const rel of listFilesRecursive(path)) {
      if (ignore(rel)) continue;
      const { mtimeMs } = statSync(join(path, rel));
      if (newest === null || mtimeMs > newest) newest = mtimeMs;
    }
  }
  return newest;
}

/**
 * Whether `outputPath` is up to date with respect to every input in
 * `inputPaths` — it exists and is at least as new as the newest of them. A
 * missing output is never fresh; an output whose inputs have all vanished is
 * (nothing remains that could have superseded it).
 *
 * Deliberately mtime-based rather than content-hashed, matching
 * `nodespace-app-build`'s `sync_stale_sidecar`. The known residual failure mode
 * is that git stamps checked-out files with checkout time, so a branch switch
 * that lands on the same coarse tick as an existing binary can read as fresh
 * when it isn't. What keeps that tolerable here: the outputs (`binaries/`,
 * `resources/skill/`) are gitignored, so a checkout only ever moves the
 * tracked `src/` side, and almost always forward. Hashing the inputs would
 * close the gap outright, but that is a real cost for a local dev script
 * whose worst case is one stale `bun run build:skill` away from correct —
 * this is a considered tradeoff, not an oversight. (A *partially written*
 * output, the one failure this can't self-correct from, is prevented at the
 * source instead — see `compileInstaller`.)
 */
export function isOutputFresh(
  outputPath: string,
  inputPaths: string[],
  ignore?: (relativePath: string) => boolean,
): boolean {
  if (!existsSync(outputPath)) return false;
  const newestInput = newestMtimeMs(inputPaths, ignore);
  if (newestInput === null) return true;
  return statSync(outputPath).mtimeMs >= newestInput;
}

/**
 * Where the bytes of one staged file come from: a file to copy, or the bytes
 * themselves (for a file that exists only in the merged form, such as a
 * `SKILL.md` with a fragment appended).
 */
export type StagedSource = { from: string } | { bytes: Uint8Array };

/**
 * Every file under `root` as a staging map, keyed by path relative to `root`
 * (see `listFilesRecursive` for what a plain file or a missing root yields).
 */
export function treeSources(root: string): Map<string, StagedSource> {
  return new Map(listFilesRecursive(root).map((rel) => [rel, { from: join(root, rel) }]));
}

/**
 * Mirrors `files` onto `destRoot` writing only what actually differs: files
 * whose bytes changed are written, files under `destRoot` that `files` does
 * not hold are deleted, and byte-identical files are left completely alone,
 * mtime included (see the module doc on why that matters). Returns the count
 * of files written or removed, so the caller can report a real no-op.
 *
 * The keys are paths relative to `destRoot`; the empty path stages a plain
 * file at `destRoot` itself, the shape `listFilesRecursive` gives a plain file.
 *
 * Mirrors *files*, not the directory structure as such: an empty source
 * directory has nothing to copy and so never appears in the destination,
 * which is the right shape for a Tauri bundle (it globs files). Symlinks are
 * not supported anywhere under the staged entries and are not handled.
 */
export function syncFilesByContent(files: Map<string, StagedSource>, destRoot: string): number {
  // A path that flipped kind between runs (a file where a directory now
  // stands, or the reverse) can't be reconciled entry-by-entry — reading a
  // directory as bytes just throws EISDIR. Clear it and let the copy below
  // rebuild it, so the staging self-heals instead of wedging on an error
  // whose remedy ("delete resources/skill/ and re-run") isn't obvious.
  if (files.size > 0 && existsSync(destRoot) && statSync(destRoot).isDirectory() === files.has('')) {
    rmSync(destRoot, { recursive: true, force: true });
  }

  let changed = 0;

  for (const rel of listFilesRecursive(destRoot)) {
    if (files.has(rel)) continue;
    rmSync(join(destRoot, rel));
    changed += 1;
  }

  for (const [rel, source] of files) {
    const dest = join(destRoot, rel);
    if (existsSync(dest)) {
      // Same self-healing as above, one level down: a nested path that is now
      // a file but was staged as a directory can't be byte-compared.
      if (statSync(dest).isDirectory()) {
        rmSync(dest, { recursive: true, force: true });
      } else if (readFileSync(dest).equals('from' in source ? readFileSync(source.from) : source.bytes)) {
        continue;
      }
    }
    mkdirSync(dirname(dest), { recursive: true });
    if ('from' in source) copyFileSync(source.from, dest);
    else writeFileSync(dest, source.bytes);
    changed += 1;
  }

  return changed;
}

/**
 * Removes directories under `keepRoot` that no longer hold any file, deepest
 * first, leaving `keepRoot` itself in place. `syncFilesByContent` deletes
 * files but leaves their parents behind; an emptied directory is harmless to
 * the bundle but confusing to find on disk.
 */
export function pruneEmptyDirs(dir: string, keepRoot: string = dir): void {
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) pruneEmptyDirs(join(dir, entry.name), keepRoot);
  }
  if (dir !== keepRoot && readdirSync(dir).length === 0) {
    rmSync(dir, { recursive: true });
  }
}

// Host triple: matches whatever Tauri/rustc expects on this platform for its
// externalBin lookup, same triples the sidecar cargo builds already use
// (release.yml's per-platform steps, build-sidecars.ts locally). Compiled
// natively on each CI runner's own platform — no cross-compilation. Only
// macOS and Windows ship a Tauri desktop app at all (Linux is CLI+daemon
// binaries only, no packaged GUI app, so no skill installer to run there).
export function hostTriple(): string | null {
  if (platform() === 'darwin') {
    return `${arch() === 'arm64' ? 'aarch64' : 'x86_64'}-apple-darwin`;
  }
  if (platform() === 'win32') {
    return 'x86_64-pc-windows-msvc';
  }
  return null;
}

/**
 * Maps a Rust target triple to the value `bun build --compile --target=`
 * expects for that platform. Only used when cross-compiling the skill
 * installer for a triple other than `hostTriple()` — e.g. the Tier 1 local
 * Windows cross-compile (scripts/build-windows-quick.ts), which builds the
 * Rust sidecars for x86_64-pc-windows-msvc from a macOS host via cargo-xwin,
 * and needs the skill installer's compiled binary to match. CI never needs
 * this: every release.yml leg compiles natively on a runner whose host
 * already IS the target, so `bun build --compile` with no `--target` flag
 * (i.e. `hostTriple()` unchanged) is correct there.
 */
export function bunCompileTarget(rustTriple: string): string {
  switch (rustTriple) {
    case 'x86_64-pc-windows-msvc':
      return 'bun-windows-x64';
    case 'aarch64-apple-darwin':
      return 'bun-darwin-arm64';
    case 'x86_64-apple-darwin':
      return 'bun-darwin-x64';
    default:
      throw new Error(
        `bunCompileTarget: no Bun cross-compile target known for Rust triple '${rustTriple}'`,
      );
  }
}

/**
 * What `bun build --compile` actually bundles into the standalone installer:
 * everything reachable from `src/install.ts`, plus the manifests that shape
 * how those get resolved. Deliberately NOT SKILL.md, references/ or plugins/ —
 * the binary reads those at runtime from `--resource-root`, so they are
 * staged resources, never compile inputs, and counting them here would
 * recompile 58MB every time a doc line moved.
 */
export function compileInputs(skillDir: string): string[] {
  return [join(skillDir, 'src'), join(skillDir, 'package.json'), join(skillDir, 'tsconfig.json')];
}

/**
 * Paths under `compileInputs` that don't actually reach the bundle:
 * `src/tests/` is unreachable from `src/install.ts` and excluded by
 * packages/skill's own tsconfig, so editing a skill test should not cost a
 * 58MB recompile.
 */
export function isNotACompileInput(relativePath: string): boolean {
  return relativePath === 'tests' || relativePath.startsWith(`tests${sep}`);
}

/**
 * Compiles `entrypoint` into a standalone binary at `outfile`, via a
 * temporary sibling that is renamed into place only once the compile has
 * succeeded.
 *
 * The indirection is what makes the mtime guard above safe to trust.
 * `bun build --compile` writes its output incrementally, so a run killed
 * partway (Ctrl-C, OOM, a full disk) would otherwise leave a truncated
 * `outfile` carrying a brand-new mtime — newer than every input, and so
 * reported fresh by `isOutputFresh` forever after. Every later `build:skill`
 * would then skip the compile and ship the corrupt binary, with the only
 * recovery a manual `rm` of a file most developers don't know exists, and the
 * symptom surfacing far downstream as a broken skill install at app runtime.
 *
 * Renaming within a directory is atomic, so the guard can only ever observe a
 * complete binary: either the rename happened and `outfile` is whole, or it
 * didn't and `outfile` is left exactly as it was (stale, but honestly stale —
 * still older than its inputs, so the next run retries the compile). Temp
 * files are removed on failure, and any orphaned by a hard kill are swept on
 * the next call.
 */
export async function compileInstaller(
  entrypoint: string,
  outfile: string,
  // Injectable so a test can drive the write-then-fail case directly. `bun
  // build --compile` resolves and bundles everything before it opens its
  // output, so no malformed entrypoint reproduces a partial write from the
  // outside — the guarantee has to be pinned at this seam instead.
  compile: (entry: string, target: string) => Promise<unknown> = (entry, target) =>
    $`bun build --compile ${entry} --outfile ${target}`.quiet(),
): Promise<void> {
  // A hard kill (SIGKILL) skips the catch below, orphaning a ~58MB temp file
  // that nothing would otherwise reclaim. Sweeping them here — before adding
  // one — keeps that bounded at one per interrupted run rather than
  // accumulating silently. They are never bundled regardless: tauri-build's
  // copy_binaries iterates the exact `externalBin` names, not a glob.
  // The tmp marker goes *before* the extension, not after: `bun build
  // --compile` on Windows silently appends `.exe` to an --outfile that
  // doesn't already end in it, so a tempfile named `foo.exe.tmp-1234`
  // actually lands on disk as `foo.exe.tmp-1234.exe` and the rename below
  // then fails with ENOENT looking for the un-suffixed name. Keeping the
  // real extension trailing (`foo.tmp-1234.exe`) means bun sees it's already
  // there and leaves the name alone, on every platform.
  const ext = extname(outfile);
  const stem = ext ? outfile.slice(0, -ext.length) : outfile;

  for (const stale of readdirSync(dirname(outfile))) {
    if (stale.startsWith(`${basename(stem)}.tmp-`)) {
      rmSync(join(dirname(outfile), stale), { recursive: true, force: true });
    }
  }

  const tempfile = `${stem}.tmp-${process.pid}${ext}`;
  try {
    await compile(entrypoint, tempfile);
    if (platform() !== 'win32') {
      chmodSync(tempfile, 0o755);
    }
    renameSync(tempfile, outfile);
  } catch (error) {
    rmSync(tempfile, { force: true });
    throw error;
  }
}

/**
 * Entries staged into `resources/skill/`. Anything else found there is a
 * leftover from an earlier layout and gets dropped.
 *
 * `references/` carries the on-demand tier of the skill (the full CLI
 * reference). It is part of the shipped artifact, not a dev-only doc: SKILL.md
 * points at it by relative path, so omitting it leaves the body referring to a
 * file that isn't there.
 */
export const STAGED_ENTRIES = ['dist', 'plugins', 'SKILL.md', 'references', 'package.json'];

/**
 * `--target <rust-triple>` override for `main()`'s compiled-binary triple,
 * used by scripts/build-windows-quick.ts to cross-compile the skill
 * installer for x86_64-pc-windows-msvc from a macOS host. `undefined` when
 * the flag isn't present (the normal case — every other caller wants
 * `hostTriple()`); throws on `--target` with no value following it, rather
 * than silently falling back to the host triple for what was clearly meant
 * to be an explicit override.
 */
export function parseTargetArg(argv: string[]): string | undefined {
  const i = argv.indexOf('--target');
  if (i === -1) return undefined;
  const value = argv[i + 1];
  if (!value) throw new Error('--target requires a Rust target triple argument (e.g. --target x86_64-pc-windows-msvc)');
  return value;
}

/** The environment variable naming the directory of build-time additions to the skill. */
export const SKILL_EXTENSIONS_VAR = 'NODESPACE_SKILL_EXTENSIONS';

/** What a skill-extension directory adds to the staged skill. */
export interface SkillExtensions {
  /** The directory it was read from, as an absolute path. */
  dir: string;
  /** Each added `references/*.md`, by file name, as an absolute path. */
  references: Map<string, string>;
  /** The directory's `SKILL.md`, to be appended to the staged one. Absent when it has none. */
  fragment?: string;
}

const CORE_REFERENCES_DIR = join(SKILL_DIR, 'references');

/** Whether an entry is a dot-entry (`.DS_Store`, `.gitkeep`), which a skill extension skips. */
function isDotEntry(entry: Dirent): boolean {
  return entry.name.startsWith('.');
}

/** A directory's entries, in name order, so a build's log and first error do not depend on the filesystem. */
function sortedEntries(dir: string): Dirent[] {
  return readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
    a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
  );
}

/**
 * Reads the skill extension named by `NODESPACE_SKILL_EXTENSIONS` in `env`:
 * `undefined` when it is unset or empty, otherwise what the directory adds.
 * A relative path resolves against the directory the build runs from, which is
 * the repository root under `bun run build:skill`.
 *
 * The directory may hold only a `SKILL.md` (non-empty) and a flat `references/`
 * of `*.md` files, the same files the installer installs from a package root.
 * Anything else, a name already taken by one of core's references
 * (`coreReferencesDir`, compared without regard to case so it cannot clobber
 * one on a case-insensitive filesystem), or a directory that does not exist
 * throws an error that names the problem; a build that quietly ignored a
 * misplaced file would ship less guidance than its author meant.
 *
 * Entries whose names start with a dot, at the root and under `references/`,
 * are skipped and never staged: they are filesystem noise (`.DS_Store`, which
 * Finder recreates, or a `.gitkeep`), and rejecting them would fail every dev
 * build of a checkout someone opened in Finder.
 */
export function readSkillExtensions(
  env: Record<string, string | undefined> = process.env,
  coreReferencesDir: string = CORE_REFERENCES_DIR,
): SkillExtensions | undefined {
  const value = env[SKILL_EXTENSIONS_VAR];
  if (!value) return undefined;

  const dir = resolve(value);
  const fail = (problem: string): never => {
    throw new Error(`${SKILL_EXTENSIONS_VAR} (${dir}): ${problem}`);
  };

  if (!existsSync(dir)) return fail('the directory does not exist');
  if (!statSync(dir).isDirectory()) return fail('it is not a directory');

  const extensions: SkillExtensions = { dir, references: new Map() };
  const coreNames = new Set(listFilesRecursive(coreReferencesDir).map((name) => name.toLowerCase()));

  for (const entry of sortedEntries(dir)) {
    if (isDotEntry(entry)) continue;
    if (entry.name === 'SKILL.md') {
      if (!entry.isFile()) return fail('SKILL.md is not a regular file');
      const fragment = readFileSync(join(dir, entry.name), 'utf8');
      if (fragment.trim() === '') return fail('SKILL.md is empty');
      extensions.fragment = fragment;
    } else if (entry.name === 'references') {
      if (entry.isSymbolicLink()) return fail('references is a symlink; symlinks are not accepted');
      if (!entry.isDirectory()) return fail('references is not a directory');
      const referencesDir = join(dir, entry.name);
      for (const reference of sortedEntries(referencesDir)) {
        if (isDotEntry(reference)) continue;
        const where = `references/${reference.name}`;
        if (reference.isDirectory()) return fail(`${where} is a directory; references must be flat`);
        if (!reference.isFile()) return fail(`${where} is not a regular file`);
        if (!reference.name.endsWith('.md')) return fail(`${where} is not a .md file`);
        if (coreNames.has(reference.name.toLowerCase())) {
          return fail(`${where} has the same name as one of core's references`);
        }
        extensions.references.set(reference.name, join(referencesDir, reference.name));
      }
    } else {
      return fail(`unexpected entry '${entry.name}'; only SKILL.md and references/*.md are accepted`);
    }
  }

  return extensions;
}

/** The one log line a build prints for its extension: where it came from and what it adds. */
export function describeSkillExtensions(extensions: SkillExtensions): string {
  const added = [
    ...[...extensions.references.keys()].map((name) => `references/${name}`),
    ...(extensions.fragment === undefined ? [] : ['a SKILL.md fragment']),
  ];
  return `Skill extensions from ${extensions.dir}: adding ${added.length === 0 ? 'nothing' : added.join(', ')}.`;
}

/**
 * The files to stage for one of `STAGED_ENTRIES`: core's own from `skillDir`,
 * and, when `extensions` is given, what it adds to `references` and `SKILL.md`.
 * Every other entry stages as core's alone.
 */
export function stagedSources(
  entry: string,
  skillDir: string,
  extensions?: SkillExtensions,
): Map<string, StagedSource> {
  const sources = treeSources(join(skillDir, entry));
  if (!extensions) return sources;

  if (entry === 'references') {
    for (const [name, from] of extensions.references) {
      if (sources.has(name)) throw new Error(`references/${name} would replace one of core's references`);
      sources.set(name, { from });
    }
  } else if (entry === 'SKILL.md' && extensions.fragment !== undefined) {
    sources.set('', {
      bytes: Buffer.concat([readFileSync(join(skillDir, entry)), Buffer.from(`\n${extensions.fragment}`)]),
    });
  }
  return sources;
}

/**
 * Stages `skillDir`'s runtime output into `resourceDir` (core's, merged with
 * `extensions` when given), writing only what differs and dropping anything
 * staged that the source no longer holds. Returns how many files it wrote or
 * removed, zero when everything was already current.
 */
export function stageSkillResources(
  skillDir: string,
  resourceDir: string,
  extensions?: SkillExtensions,
): number {
  mkdirSync(resourceDir, { recursive: true });

  // Drop anything already staged that is no longer one of STAGED_ENTRIES, so
  // a since-removed entry never lingers in the bundle across rebuilds.
  for (const entry of readdirSync(resourceDir)) {
    if (!STAGED_ENTRIES.includes(entry)) {
      rmSync(join(resourceDir, entry), { recursive: true, force: true });
    }
  }

  let stagedChanges = 0;
  for (const entry of STAGED_ENTRIES) {
    stagedChanges += syncFilesByContent(
      stagedSources(entry, skillDir, extensions),
      join(resourceDir, entry),
    );
  }
  pruneEmptyDirs(resourceDir);
  return stagedChanges;
}

async function main(): Promise<void> {
  // Read first: a bad extension directory should fail the build before tsc
  // and staging run.
  const extensions = readSkillExtensions();
  if (extensions) console.log(describeSkillExtensions(extensions));

  console.log('Building packages/skill...');
  await $`bun run --cwd ${SKILL_DIR} build`;

  if (!existsSync(join(SKILL_DIR, 'dist', 'install.js'))) {
    throw new Error(
      `packages/skill build did not produce dist/install.js (expected at ${join(SKILL_DIR, 'dist', 'install.js')})`,
    );
  }

  console.log(`Staging skill resources -> ${RESOURCE_DIR}`);
  const stagedChanges = stageSkillResources(SKILL_DIR, RESOURCE_DIR, extensions);
  console.log(
    stagedChanges === 0
      ? '  Staged resources already current — nothing rewritten.'
      : `  Updated ${stagedChanges} staged file(s).`,
  );

  const explicitTarget = parseTargetArg(process.argv);
  const triple = explicitTarget ?? hostTriple();

  if (!triple) {
    console.log('Skipping compiled skill-installer binary (no Tauri desktop app on this platform).');
  } else {
    mkdirSync(BIN_DIR, { recursive: true });
    // Target-triple-based, not host-`platform()`-based: with an explicit
    // --target this binary's extension must match what it's actually being
    // built FOR (e.g. a Windows .exe compiled cross-platform from macOS),
    // which host platform() cannot tell us.
    const ext = triple.includes('windows') ? '.exe' : '';
    const outfile = join(BIN_DIR, `nodespace-skill-installer-${triple}${ext}`);

    if (isOutputFresh(outfile, compileInputs(SKILL_DIR), isNotACompileInput)) {
      console.log(`Standalone skill installer is current -> ${outfile} (skipping compile).`);
    } else {
      console.log(`Compiling standalone skill installer -> ${outfile}`);
      // Cross-compiling (explicit --target different from this host's own
      // triple) needs Bun's own --target= flag on top of the default
      // compile command; same-triple (the common case) keeps using
      // compileInstaller's default compile callback unchanged.
      const compile =
        explicitTarget && explicitTarget !== hostTriple()
          ? (entry: string, target: string) =>
              $`bun build --compile --target=${bunCompileTarget(explicitTarget)} ${entry} --outfile ${target}`.quiet()
          : undefined;
      await compileInstaller(join(SKILL_DIR, 'src', 'install.ts'), outfile, compile);
    }
  }

  console.log('Done.');
}

if (import.meta.main) {
  await main();
}
