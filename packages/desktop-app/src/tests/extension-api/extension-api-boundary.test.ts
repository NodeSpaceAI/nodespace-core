/**
 * The host API is a one-way boundary (ADR-082 §3.6):
 *   - core modules never import it, so it stays something core offers rather than
 *     something core depends on;
 *   - it never imports the edition-specific modules core does not ship (the
 *     sync variant machine and its stores, the membership store and service, and
 *     the built-in extension that renders them);
 *   - only its `/testing` entry reaches test code, so the other entries never
 *     pull Vitest into a bundle;
 *   - the fixture extension imports core only through it, like an out-of-tree
 *     extension.
 *
 * Specifiers come from each file's syntax tree (see `importSpecifiers`), so
 * comments and strings never count. The edition-specific needles are built from
 * fragments so this file adds no boundary-check markers.
 */
import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  HOST_API_DIR,
  LIB_ROOT,
  SRC_ROOT,
  importSpecifiers,
  resolveSpecifier,
  sourceFiles
} from './source-scan';

const EDITION = ['p', 'ro'].join('');

/** True when `specifier` names the host API, by alias or by a path into its directory. */
function isHostApi(specifier: string, fromFile: string): boolean {
  if (/^(?:@nodespace\/extension-api|\$lib\/extension-api)(?:\/|$)/.test(specifier)) return true;
  if (!specifier.startsWith('.')) return false;
  const target = resolveSpecifier(specifier, fromFile) ?? '';
  return target === HOST_API_DIR || target.startsWith(`${HOST_API_DIR}${path.sep}`);
}

const EDITION_SEGMENT = new RegExp(`(?:^|[-_.])${EDITION}(?:[-_.]|$)`);
const MEMBERSHIP_MODULE = new RegExp(`^${['member', 'ship'].join('')}(?:[-_.]|$)`);

/** True when `specifier`'s last segment names an edition-specific module. */
function isEditionModule(specifier: string): boolean {
  const basename = specifier.split('/').pop() ?? '';
  return EDITION_SEGMENT.test(basename) || MEMBERSHIP_MODULE.test(basename);
}

/** True when `specifier` reaches test code: the test runner, testing-library or `src/tests`. */
function isTestCode(specifier: string, fromFile: string): boolean {
  if (/^(?:vitest|@vitest\/|@testing-library\/)/.test(specifier)) return true;
  const target = resolveSpecifier(specifier, fromFile);
  return target !== null && target.startsWith(path.join(SRC_ROOT, 'tests') + path.sep);
}

interface Import {
  file: string;
  specifier: string;
}

/**
 * The imports of `files`. With `mentioning`, a file whose text does not contain
 * it is skipped unparsed, which keeps a scan of all of `src/lib` fast.
 */
function importsOf(files: string[], mentioning?: string): Import[] {
  return files.flatMap((file) => {
    const source = fs.readFileSync(file, 'utf8');
    if (mentioning !== undefined && !source.includes(mentioning)) return [];
    return importSpecifiers(file, source).map((specifier) => ({ file, specifier }));
  });
}

function describeImports(imports: Import[]): string[] {
  return imports.map(({ file, specifier }) => `${path.relative(SRC_ROOT, file)} -> ${specifier}`);
}

const hostApiFiles = sourceFiles(HOST_API_DIR);

describe('extension API boundary', () => {
  it('is imported by no core module under src/lib or src/routes', () => {
    const coreFiles = [
      ...sourceFiles(LIB_ROOT).filter((file) => !file.startsWith(`${HOST_API_DIR}${path.sep}`)),
      ...sourceFiles(path.join(SRC_ROOT, 'routes'))
    ];
    // Every way to name the host API, by alias or by path, contains its directory name.
    const offenders = importsOf(coreFiles, 'extension-api').filter(({ file, specifier }) =>
      isHostApi(specifier, file)
    );
    expect(
      describeImports(offenders),
      'Core modules never import the host API; it is the surface core offers extensions.'
    ).toEqual([]);
  });

  it('imports no edition-specific module', () => {
    expect(hostApiFiles.length).toBeGreaterThan(0);
    const offenders = importsOf(hostApiFiles).filter(({ specifier }) => isEditionModule(specifier));
    expect(describeImports(offenders)).toEqual([]);
  });

  it('reaches test code only from its /testing entry', () => {
    const testing = path.join(HOST_API_DIR, 'testing.ts');
    const offenders = importsOf(hostApiFiles.filter((file) => file !== testing)).filter(
      ({ file, specifier }) =>
        isTestCode(specifier, file) || resolveSpecifier(specifier, file) === testing
    );
    expect(describeImports(offenders)).toEqual([]);
  });

  it('is the only way the fixture extension reaches core', () => {
    const fixtureDir = path.join(SRC_ROOT, 'tests/fixtures/test-extension');
    const offenders = importsOf(sourceFiles(fixtureDir)).filter(
      ({ specifier }) =>
        !specifier.startsWith('./') &&
        !/^(?:svelte|@tauri-apps\/api|@nodespace\/extension-api)(?:\/|$)/.test(specifier)
    );
    expect(describeImports(offenders)).toEqual([]);
  });
});

describe('the boundary predicates', () => {
  const here = path.join(LIB_ROOT, 'stores/example.ts');

  it('read imports from the syntax tree, not from comments or strings', () => {
    const source = [
      "import a from 'a';",
      "// import b from 'b';",
      'const s = "from \'c\'";',
      "const re = /'/; import d from 'd';",
      "export { e } from 'e';",
      "const f = () => import('f');",
      "type G = import('g').G;"
    ].join('\n');
    expect(importSpecifiers(here, source)).toEqual(['a', 'd', 'e', 'f', 'g']);
  });

  it('read only the script blocks of a .svelte file', () => {
    const source = [
      '<script lang="ts">',
      "  import a from 'a';",
      '</script>',
      "<!-- import b from 'b' -->",
      "<p>import c from 'c'</p>"
    ].join('\n');
    expect(importSpecifiers(path.join(LIB_ROOT, 'example.svelte'), source)).toEqual(['a']);
  });

  it('recognize the host API by alias and by relative path', () => {
    for (const specifier of [
      '@nodespace/extension-api',
      '@nodespace/extension-api/ui',
      '$lib/extension-api',
      '$lib/extension-api/testing',
      '../extension-api',
      '../extension-api/index'
    ]) {
      expect(isHostApi(specifier, here), specifier).toBe(true);
    }
    for (const specifier of ['$lib/extension-apis', '../plugins/ui-extensions', 'svelte']) {
      expect(isHostApi(specifier, here), specifier).toBe(false);
    }
  });

  it('recognize the edition-specific modules by name, and nothing else', () => {
    for (const specifier of [
      `$lib/plugins/${EDITION}-plugin`,
      `$lib/plugins/${EDITION}-${'sync'}-variant.svelte`,
      `$lib/stores/${EDITION}-${'sync'}.svelte`,
      '$lib/stores/membership.svelte',
      `$lib/services/${['membership', 'service'].join('-')}`,
      `$lib/components/first-${EDITION}-consent-slot.svelte`
    ]) {
      expect(isEditionModule(specifier), specifier).toBe(true);
    }
    for (const specifier of [
      '$lib/stores/database.svelte',
      '$lib/components/property-forms/task-schema-form.svelte',
      '$lib/types/project-node',
      '$lib/plugins/ui-extensions'
    ]) {
      expect(isEditionModule(specifier), specifier).toBe(false);
    }
  });

  it('recognize test code', () => {
    const entry = path.join(HOST_API_DIR, 'index.ts');
    for (const specifier of [
      'vitest',
      '@testing-library/svelte',
      '../../tests/helpers/mock-tauri-core'
    ]) {
      expect(isTestCode(specifier, entry), specifier).toBe(true);
    }
    expect(isTestCode('$lib/utils/logger', entry)).toBe(false);
  });
});
