// Locating the module a build injects extensions from (ADR-082 §2.1).
//
// Plain ESM with JSDoc types, importing only `node:fs` and `node:path`, so that
// every config loader that needs it (Vite, Vitest, Tailwind's jiti) can load it
// without a TypeScript step.

import { statSync } from 'node:fs';
import { dirname, resolve, sep } from 'node:path';

/** The environment variable naming the extensions entry module. */
export const EXTENSIONS_ENV = 'NODESPACE_EXTENSIONS';

/**
 * Forward-slash form of a path; globs and Vite ids use it on every platform.
 *
 * @param {string} path
 * @returns {string}
 */
function toPosix(path) {
  return path.split(sep).join('/');
}

/**
 * Escape glob syntax in a literal path, so a directory named `ext (1)` matches
 * itself instead of being read as a pattern (which would silently match
 * nothing).
 *
 * @param {string} path a forward-slash path
 * @returns {string}
 */
function escapeGlob(path) {
  return path.replace(/[()[\]{}*?!+@|\\]/g, '\\$&');
}

/**
 * Resolve the extensions entry from the raw variable value.
 *
 * @param {string | undefined} raw the value of {@link EXTENSIONS_ENV}
 * @param {string} root the directory a relative path resolves against
 * @returns {string | null} the absolute path of the entry, or null when the
 *   variable is unset or blank
 * @throws {Error} when the path is not an existing file
 */
export function resolveExtensionsEntry(raw, root) {
  if (raw === undefined || raw.trim() === '') return null;
  const entry = resolve(root, raw.trim());
  let isFile = false;
  try {
    isFile = statSync(entry).isFile();
  } catch {
    // Missing or unreadable: reported below.
  }
  if (!isFile) {
    throw new Error(`${EXTENSIONS_ENV} does not name an existing file: ${entry}`);
  }
  return entry;
}

/**
 * The Tailwind `content` globs for the entry's directory, so classes used only
 * by injected components are not purged.
 *
 * @param {string | null} entry absolute entry path from {@link resolveExtensionsEntry}
 * @returns {string[]}
 */
export function extensionContentGlobs(entry) {
  if (entry === null) return [];
  // Not escaped, unlike the test glob: Tailwind splits a glob into its base
  // directory and the rest and escapes the base itself, and its path
  // normalization would turn a backslash escape added here into a separator.
  return [`${toPosix(dirname(entry))}/**/*.{html,js,svelte,ts}`];
}

/**
 * The Vitest `include` globs for the entry's directory: its `*.test.ts` files.
 * Vitest's file scanner accepts an absolute glob, so no root is needed.
 *
 * @param {string | null} entry absolute entry path from {@link resolveExtensionsEntry}
 * @returns {string[]}
 */
export function extensionTestGlobs(entry) {
  if (entry === null) return [];
  return [`${escapeGlob(toPosix(dirname(entry)))}/**/*.test.ts`];
}
