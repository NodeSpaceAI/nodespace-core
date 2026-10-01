/**
 * Every change to the extension host API is deliberate and versioned (ADR-082 §7).
 *
 * The snapshot (`extension-api-surface.json`) records `EXTENSION_API_VERSION`,
 * each entry's export names, and a hash of the API's type declarations. A
 * surface change without a version change fails; so does a removal with only a
 * minor bump, and a bump that was not re-recorded. To re-record after bumping:
 *
 *   UPDATE_EXTENSION_API_SURFACE=1 bun run --cwd packages/desktop-app test src/tests/extension-api
 *
 * Re-recording refuses a missing or too-small bump. The hash covers types
 * declared in the registry and in the host API's files; signatures that reach the
 * API through other core modules (`DatabaseInfo`, a component's props) are left
 * to review against the policy in `src/lib/extension-api/index.ts`.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { EXTENSION_API_VERSION } from '$lib/plugins/ui-extensions';
import * as indexEntry from '@nodespace/extension-api';
import * as uiEntry from '@nodespace/extension-api/ui';
import * as testingEntry from '@nodespace/extension-api/testing';
import {
  ENTRY_FILES,
  ENTRY_NAMES,
  currentSurface,
  readSnapshot,
  surfaceProblems,
  writeSnapshot,
  type ApiVersion,
  type ProblemKind,
  type Surface,
  type SurfaceSnapshot
} from './extension-api-surface';
import { exportedNames, hashDeclarations, typeDeclarations } from './source-scan';

const version: ApiVersion = {
  major: EXTENSION_API_VERSION.major,
  minor: EXTENSION_API_VERSION.minor
};

describe('extension API surface', () => {
  it('matches the snapshot, or EXTENSION_API_VERSION accounts for the change', () => {
    const current = currentSurface();
    const problems = surfaceProblems(readSnapshot(), current, version);

    if (process.env.UPDATE_EXTENSION_API_SURFACE === '1') {
      const blocking = problems.filter((p) => p.kind !== 'not-recorded');
      expect(blocking.map((p) => p.message)).toEqual([]);
      if (problems.length > 0) writeSnapshot({ version, ...current });
      return;
    }
    expect(problems.map((p) => p.message)).toEqual([]);
  });

  // Checks the source parsing against what each entry really exports. The entries are
  // imported statically: loading the UI components inside a test could outlast its timeout.
  it.each(ENTRY_NAMES)('reads the %s entry’s runtime exports correctly', (entry) => {
    const modules = { index: indexEntry, ui: uiEntry, testing: testingEntry };
    const runtime = Object.keys(modules[entry]).sort();
    const parsed = exportedNames(ENTRY_FILES[entry])
      .filter((e) => !e.isType && !e.name.includes('.'))
      .map((e) => e.name)
      .sort();
    expect(runtime).toEqual(parsed);
  });

  it('records each Dialog part under the namespace', () => {
    expect(currentSurface().entries.ui).toEqual(
      expect.arrayContaining(['Dialog', 'Dialog.Root', 'Dialog.Content', 'Dialog.Title'])
    );
  });
});

describe('surfaceProblems', () => {
  const base: SurfaceSnapshot = {
    version: { major: 1, minor: 0 },
    entries: { index: ['a', 'b'], ui: ['Button'], testing: ['render'] },
    typesHash: 'h1'
  };
  const withEntries = (entries: Partial<Surface['entries']>, typesHash = 'h1'): Surface => ({
    entries: { ...base.entries, ...entries },
    typesHash
  });
  const kinds = (
    recorded: SurfaceSnapshot | null,
    current: Surface,
    v: ApiVersion
  ): ProblemKind[] => surfaceProblems(recorded, current, v).map((p) => p.kind);

  it('passes when the surface and the version both match', () => {
    expect(kinds(base, withEntries({}), { major: 1, minor: 0 })).toEqual([]);
  });

  it('fails an added export without a bump', () => {
    expect(kinds(base, withEntries({ index: ['a', 'b', 'c'] }), { major: 1, minor: 0 })).toEqual([
      'unversioned-change'
    ]);
  });

  it('fails a removed export without a bump', () => {
    expect(kinds(base, withEntries({ ui: [] }), { major: 1, minor: 0 })).toEqual([
      'unversioned-change'
    ]);
  });

  it('fails a changed type declaration without a bump', () => {
    expect(kinds(base, withEntries({}, 'h2'), { major: 1, minor: 0 })).toEqual([
      'unversioned-change'
    ]);
  });

  it('asks for a re-record once an addition is bumped', () => {
    const problems = surfaceProblems(base, withEntries({ index: ['a', 'b', 'c'] }), {
      major: 1,
      minor: 1
    });
    expect(problems.map((p) => p.kind)).toEqual(['not-recorded']);
    expect(problems[0].message).toContain('UPDATE_EXTENSION_API_SURFACE=1');
  });

  it('fails a removal that moved only the minor version', () => {
    expect(kinds(base, withEntries({ index: ['a'] }), { major: 1, minor: 1 })).toEqual([
      'removal-needs-major',
      'not-recorded'
    ]);
  });

  it('fails a rename that moved only the minor version', () => {
    expect(kinds(base, withEntries({ index: ['a', 'c'] }), { major: 1, minor: 1 })).toEqual([
      'removal-needs-major',
      'not-recorded'
    ]);
  });

  it('accepts a removal with a major bump, once re-recorded', () => {
    expect(kinds(base, withEntries({ index: ['a'] }), { major: 2, minor: 0 })).toEqual([
      'not-recorded'
    ]);
  });

  it('fails a version that went backwards', () => {
    expect(kinds({ ...base, version: { major: 1, minor: 2 } }, withEntries({}), version)).toEqual([
      'version-regressed'
    ]);
  });

  it('asks for a first recording when there is no snapshot', () => {
    expect(kinds(null, withEntries({}), version)).toEqual(['not-recorded']);
  });
});

describe('source parsing', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'extension-api-surface-'));
  let count = 0;
  const file = (source: string): string => {
    const target = path.join(dir, `module-${count++}.ts`);
    fs.writeFileSync(target, source);
    return target;
  };
  afterAll(() => fs.rmSync(dir, { recursive: true, force: true }));

  it('refuses `export *`, with or without a namespace name', () => {
    expect(() => exportedNames(file("export * from './a';"))).toThrow(/export \*/);
    expect(() => exportedNames(file("export * as A from './a';"))).toThrow(/export \*/);
  });

  it('reads aliases, type-only exports and declarations, ignoring comments', () => {
    const names = exportedNames(
      file(
        [
          "export { a, b as c, type D } from './x';",
          "export type { E } from './y';",
          '// export const commented = 1;',
          'export const f = 1;',
          'export function g() {}',
          'export interface H {}',
          'export type I = string;'
        ].join('\n')
      )
    );
    expect(names).toEqual([
      { name: 'a', isType: false },
      { name: 'c', isType: false },
      { name: 'D', isType: true },
      { name: 'E', isType: true },
      { name: 'f', isType: false },
      { name: 'g', isType: false },
      { name: 'H', isType: true },
      { name: 'I', isType: true }
    ]);
  });

  it('lists a namespace re-export’s members', () => {
    const members = file(
      'const Root = 1;\nconst Title = 2;\nexport { Root, Title as DialogTitle };'
    );
    const entry = file(
      `import * as Dialog from './${path.basename(members, '.ts')}';\nexport { Dialog };`
    );
    expect(exportedNames(entry).map((e) => e.name)).toEqual([
      'Dialog',
      'Dialog.Root',
      'Dialog.DialogTitle'
    ]);
  });

  describe('the types hash', () => {
    const hashOf = (source: string): string =>
      hashDeclarations(typeDeclarations(file(source), true));
    const original = [
      '/** Doc. */',
      "export type Slot = 'a' | 'b';",
      'export interface Thing {',
      '  // Its id.',
      '  id: string;',
      '  load: () => Promise<{ default: number }>;',
      '}',
      'interface Internal { x: number }'
    ].join('\n');

    it('ignores comments, whitespace, line breaks and trailing separators', () => {
      const reformatted = [
        'export type Slot =',
        "  | 'a'",
        "  | 'b';",
        '/* a block comment */',
        'export interface Thing { id: string; load: () => Promise<{',
        '  default: number;',
        '}> }'
      ].join('\n');
      expect(hashOf(reformatted)).toBe(hashOf(original));
    });

    it('ignores declarations that are not exported, when asked to', () => {
      expect(hashOf(original.replace('x: number', 'y: string'))).toBe(hashOf(original));
    });

    it('changes when a member is added, retyped or removed', () => {
      const before = hashOf(original);
      expect(hashOf(original.replace('id: string;', 'id: string;\n  label?: string;'))).not.toBe(
        before
      );
      expect(hashOf(original.replace('id: string;', 'id: number;'))).not.toBe(before);
      expect(hashOf(original.replace("'a' | 'b'", "'a'"))).not.toBe(before);
    });

    it('ignores an import item that looks like a declaration', () => {
      expect(hashOf(`import {\n  type Foo\n} from './x';\n${original}`)).toBe(hashOf(original));
    });
  });
});
