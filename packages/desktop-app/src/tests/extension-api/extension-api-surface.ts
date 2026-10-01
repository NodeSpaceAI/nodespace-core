/**
 * The extension API's recorded surface and the version rule that guards it
 * (ADR-082 §7). `extension-api-surface.test.ts` runs the check;
 * `extension-api-surface.json` holds the snapshot.
 */
import fs from 'node:fs';
import path from 'node:path';
import {
  APP_ROOT,
  HOST_API_DIR,
  LIB_ROOT,
  exportedNames,
  hashDeclarations,
  typeDeclarations,
  type TypeDeclaration
} from './source-scan';

export const SNAPSHOT_FILE = path.join(
  APP_ROOT,
  'src/tests/extension-api/extension-api-surface.json'
);

export const RERECORD_COMMAND =
  'UPDATE_EXTENSION_API_SURFACE=1 bun run --cwd packages/desktop-app test src/tests/extension-api';

/** The registry module whose types the API re-exports. */
export const REGISTRY_FILE = path.join(LIB_ROOT, 'plugins/ui-extensions.ts');

/**
 * Each entry of the host API, by name, and the file that implements it. The
 * alias makes every file in the host API's directory importable
 * (`@nodespace/extension-api/<name>`, with `index` the bare package), so every
 * file there is an entry; anything other than a `.ts` file is refused. Dotfiles
 * (an editor's swap file, `.DS_Store`) are skipped.
 */
export function hostApiEntries(dir = HOST_API_DIR): Record<string, string> {
  const entries: Record<string, string> = {};
  const dirents = fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((dirent) => !dirent.name.startsWith('.'));
  // Sorted, so a re-recorded snapshot is byte-identical whatever the filesystem's order.
  dirents.sort((a, b) => a.name.localeCompare(b.name));
  for (const dirent of dirents) {
    if (!dirent.isFile() || !dirent.name.endsWith('.ts')) {
      throw new Error(
        `${path.relative(APP_ROOT, path.join(dir, dirent.name))}: the host API directory ` +
          'holds only .ts entries, each one importable as @nodespace/extension-api/<name>'
      );
    }
    entries[dirent.name.slice(0, -'.ts'.length)] = path.join(dir, dirent.name);
  }
  return entries;
}

export interface ApiVersion {
  major: number;
  minor: number;
}

export interface Surface {
  /** Each entry's export names, sorted; `Dialog.Root` names a namespace member. */
  entries: Record<string, string[]>;
  /** SHA-256 over the API's type declarations; see {@link apiTypeDeclarations}. */
  typesHash: string;
}

export interface SurfaceSnapshot extends Surface {
  version: ApiVersion;
}

/**
 * The type declarations the hash covers:
 *   - every one in the host API's own files, exported or not, since they exist
 *     only to type the API (the shape of the `nodes` facade, for one);
 *   - each registry declaration the API reaches: one an entry exports, or one
 *     that a covered declaration mentions, transitively. A registry type only the
 *     hosts use is left out.
 * Types the API re-exports from other core modules, such as `DatabaseInfo`, are
 * not covered.
 */
export function apiTypeDeclarations(): Map<string, TypeDeclaration> {
  const covered = new Map<string, TypeDeclaration>();
  const reachable = new Set<string>();
  for (const file of Object.values(hostApiEntries())) {
    for (const { name } of exportedNames(file)) reachable.add(name);
    for (const [name, declaration] of typeDeclarations(file)) {
      if (covered.has(name)) throw new Error(`Type ${name} is declared twice in the host API`);
      covered.set(name, declaration);
      for (const ref of declaration.refs) reachable.add(ref);
    }
  }

  const registry = typeDeclarations(REGISTRY_FILE);
  const pending = [...reachable];
  while (pending.length > 0) {
    const name = pending.pop() as string;
    const declaration = registry.get(name);
    if (declaration === undefined || covered.has(name)) continue;
    covered.set(name, declaration);
    pending.push(...declaration.refs);
  }
  return covered;
}

export function currentSurface(): Surface {
  const entries: Record<string, string[]> = {};
  for (const [entry, file] of Object.entries(hostApiEntries())) {
    entries[entry] = [...new Set(exportedNames(file).map((e) => e.name))].sort();
  }
  return { entries, typesHash: hashDeclarations(apiTypeDeclarations()) };
}

export function readSnapshot(): SurfaceSnapshot | null {
  if (!fs.existsSync(SNAPSHOT_FILE)) return null;
  return JSON.parse(fs.readFileSync(SNAPSHOT_FILE, 'utf8')) as SurfaceSnapshot;
}

export function writeSnapshot(snapshot: SurfaceSnapshot): void {
  fs.writeFileSync(SNAPSHOT_FILE, `${JSON.stringify(snapshot, null, 2)}\n`);
}

export type ProblemKind =
  /** The surface changed and the version did not. */
  | 'unversioned-change'
  /** A name was removed or renamed and only the minor version moved. */
  | 'removal-needs-major'
  /** The version is lower than the recorded one. */
  | 'version-regressed'
  /** The version differs from the recorded one, or nothing is recorded: re-record. */
  | 'not-recorded';

export interface Problem {
  kind: ProblemKind;
  message: string;
}

function formatVersion(v: ApiVersion): string {
  return `${v.major}.${v.minor}`;
}

function compareVersions(a: ApiVersion, b: ApiVersion): number {
  return a.major !== b.major ? a.major - b.major : a.minor - b.minor;
}

interface SurfaceDiff {
  added: string[];
  removed: string[];
  typesChanged: boolean;
}

function diffSurfaces(recorded: Surface, current: Surface): SurfaceDiff {
  const added: string[] = [];
  const removed: string[] = [];
  const entryNames = new Set([...Object.keys(recorded.entries), ...Object.keys(current.entries)]);
  for (const entry of [...entryNames].sort()) {
    const before = new Set(recorded.entries[entry] ?? []);
    const after = new Set(current.entries[entry] ?? []);
    for (const name of after) if (!before.has(name)) added.push(`${entry}: ${name}`);
    for (const name of before) if (!after.has(name)) removed.push(`${entry}: ${name}`);
  }
  return { added, removed, typesChanged: recorded.typesHash !== current.typesHash };
}

function describeDiff(diff: SurfaceDiff): string {
  const parts: string[] = [];
  if (diff.added.length > 0) parts.push(`added ${diff.added.join(', ')}`);
  if (diff.removed.length > 0) parts.push(`removed ${diff.removed.join(', ')}`);
  if (diff.typesChanged) parts.push('a type declaration changed');
  return parts.join('; ');
}

/**
 * What stops `current` (at `version`) from matching `recorded`; empty when they
 * match. It enforces that a change comes with a bump, and that a removed or
 * renamed export comes with a major one. Whether a changed type declaration
 * needs a major or a minor bump is left to review: the hash records only that
 * some declaration changed. Every kind but `not-recorded` also blocks
 * re-recording, so a re-record cannot hide a missing bump or a minor bump for a
 * removal.
 */
export function surfaceProblems(
  recorded: SurfaceSnapshot | null,
  current: Surface,
  version: ApiVersion
): Problem[] {
  if (recorded === null) {
    return [
      {
        kind: 'not-recorded',
        message: `No extension API surface snapshot at ${path.relative(APP_ROOT, SNAPSHOT_FILE)}. Record it: ${RERECORD_COMMAND}`
      }
    ];
  }

  const diff = diffSurfaces(recorded, current);
  const changed = diff.added.length > 0 || diff.removed.length > 0 || diff.typesChanged;
  const order = compareVersions(version, recorded.version);
  const was = formatVersion(recorded.version);
  const now = formatVersion(version);

  if (order === 0) {
    if (!changed) return [];
    return [
      {
        kind: 'unversioned-change',
        message:
          `The extension API changed (${describeDiff(diff)}) but EXTENSION_API_VERSION is still ${now}. ` +
          'Bump it in src/lib/plugins/ui-extensions.ts (minor for an addition; major for a removal, ' +
          `rename or type change), then re-record: ${RERECORD_COMMAND}`
      }
    ];
  }

  if (order < 0) {
    return [
      {
        kind: 'version-regressed',
        message: `EXTENSION_API_VERSION went from ${was} back to ${now}; it only moves forward.`
      }
    ];
  }

  const problems: Problem[] = [];
  if (diff.removed.length > 0 && version.major === recorded.version.major) {
    problems.push({
      kind: 'removal-needs-major',
      message:
        `${diff.removed.join(', ')} left the extension API, but only the minor version moved ` +
        `(${was} to ${now}). A removal or rename needs a major bump.`
    });
  }
  problems.push({
    kind: 'not-recorded',
    message: `EXTENSION_API_VERSION is ${now} but the snapshot records ${was}. Re-record it: ${RERECORD_COMMAND}`
  });
  return problems;
}
