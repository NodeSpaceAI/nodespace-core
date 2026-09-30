// @vitest-environment node
/**
 * The build-time extension seam (ADR-082 §2.1): the entry helpers, the Vite
 * plugin's hooks, and the wiring that registers what the build injects.
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { searchForWorkspaceRoot, type ConfigEnv, type UserConfig } from 'vite';
import { afterAll, beforeAll, describe, expect, expectTypeOf, it } from 'vitest';
import { nodespaceExtensions } from '../../../vite-plugins/nodespace-extensions.js';
import {
  EXTENSIONS_ENV,
  extensionContentGlobs,
  extensionTestGlobs,
  resolveExtensionsEntry
} from '../../../vite-plugins/nodespace-extensions-entry.js';
import { EXTENSION_API_VERSION } from '$lib/plugins/ui-extensions';
import type * as host from '$lib/plugins/ui-extensions';
import type * as api from '@nodespace/extension-api';

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const VIRTUAL_ID = 'virtual:nodespace-extensions';
const RESOLVED_VIRTUAL_ID = `\0${VIRTUAL_ID}`;

/** The plugin's hooks with the `this` context and hook-object forms stripped, for direct calls. */
interface Hooks {
  name: string;
  enforce: string;
  config: (config: UserConfig, env: ConfigEnv) => UserConfig;
  configResolved: (config: { logger: { info: (message: string) => void } }) => void;
  resolveId: (id: string) => string | undefined;
  load: (id: string) => string | undefined;
}

function pluginFor(entry: string | null, root = APP_ROOT): Hooks {
  return nodespaceExtensions({ root, entry }) as unknown as Hooks;
}

function configOf(plugin: Hooks): UserConfig {
  return plugin.config({}, { command: 'serve', mode: 'test' });
}

describe('nodespace extensions build entry', () => {
  let scratch: string;
  let entryInRoot: string;
  let entryOutside: string;

  beforeAll(() => {
    scratch = mkdtempSync(join(tmpdir(), 'ns-extensions-plugin-'));
    entryOutside = join(scratch, 'outside', 'index.ts');
    mkdirSync(dirname(entryOutside), { recursive: true });
    writeFileSync(entryOutside, 'export default [];\n');
    entryInRoot = join(scratch, 'root', 'extensions', 'index.ts');
    mkdirSync(dirname(entryInRoot), { recursive: true });
    writeFileSync(entryInRoot, 'export default [];\n');
  });

  afterAll(() => {
    rmSync(scratch, { recursive: true, force: true });
  });

  describe('resolveExtensionsEntry', () => {
    it('names the variable the build reads', () => {
      expect(EXTENSIONS_ENV).toBe('NODESPACE_EXTENSIONS');
    });

    it('returns null for an unset or blank value', () => {
      expect(resolveExtensionsEntry(undefined, APP_ROOT)).toBeNull();
      expect(resolveExtensionsEntry('', APP_ROOT)).toBeNull();
      expect(resolveExtensionsEntry('  \t ', APP_ROOT)).toBeNull();
    });

    it('resolves a relative path against the root', () => {
      const root = join(scratch, 'root');
      expect(resolveExtensionsEntry('extensions/index.ts', root)).toBe(entryInRoot);
      expect(resolveExtensionsEntry(' ./extensions/index.ts ', root)).toBe(entryInRoot);
    });

    it('keeps an absolute path', () => {
      expect(resolveExtensionsEntry(entryOutside, join(scratch, 'root'))).toBe(entryOutside);
    });

    it('throws for a missing file, naming the variable and the absolute path', () => {
      const root = join(scratch, 'root');
      const missing = join(root, 'does-not-exist.ts');
      expect(() => resolveExtensionsEntry('does-not-exist.ts', root)).toThrow(EXTENSIONS_ENV);
      expect(() => resolveExtensionsEntry('does-not-exist.ts', root)).toThrow(missing);
    });

    it('throws for a directory, which is not a module', () => {
      expect(() => resolveExtensionsEntry(dirname(entryOutside), APP_ROOT)).toThrow(EXTENSIONS_ENV);
    });
  });

  describe('extensionContentGlobs and extensionTestGlobs', () => {
    it('are empty without an entry', () => {
      expect(extensionContentGlobs(null)).toEqual([]);
      expect(extensionTestGlobs(null)).toEqual([]);
    });

    it('scan the entry directory for source and for test files', () => {
      const dir = dirname(entryOutside);
      expect(extensionContentGlobs(entryOutside)).toEqual([`${dir}/**/*.{html,js,svelte,ts}`]);
      expect(extensionTestGlobs(entryOutside)).toEqual([`${dir}/**/*.test.ts`]);
    });

    it('escapes glob syntax in the directory of the test glob, which Vitest matches literally', () => {
      const entry = '/work/ext (1)/[team]/{a,b}/d!(x)/e+(x)/f@(x)/m|x/g*?/index.ts';
      expect(extensionTestGlobs(entry)).toEqual([
        '/work/ext \\(1\\)/\\[team\\]/\\{a,b\\}/d\\!\\(x\\)/e\\+\\(x\\)/f\\@\\(x\\)/m\\|x/g\\*\\?/**/*.test.ts'
      ]);
    });

    // A backslash is the path separator on Windows, where toPosix turns it into a slash first.
    it.skipIf(process.platform === 'win32')('escapes a backslash in the directory name', () => {
      expect(extensionTestGlobs('/work/n\\x/index.ts')).toEqual(['/work/n\\\\x/**/*.test.ts']);
    });

    it('leaves the content glob unescaped, because Tailwind escapes the base directory itself', () => {
      expect(extensionContentGlobs('/work/ext (1)/src/index.ts')).toEqual([
        '/work/ext (1)/src/**/*.{html,js,svelte,ts}'
      ]);
    });
  });

  describe('nodespaceExtensions plugin', () => {
    it('is a pre-enforced plugin named nodespace-extensions', () => {
      const plugin = pluginFor(null);
      expect(plugin.name).toBe('nodespace-extensions');
      expect(plugin.enforce).toBe('pre');
    });

    describe('resolveId', () => {
      it('claims only the virtual module id, NUL-prefixed', () => {
        expect(pluginFor(null).resolveId(VIRTUAL_ID)).toBe(RESOLVED_VIRTUAL_ID);
      });

      it('ignores every other id', () => {
        const plugin = pluginFor(entryOutside);
        for (const id of [
          'svelte',
          '@nodespace/extension-api',
          './virtual:nodespace-extensions',
          'virtual:nodespace-extensions/extra',
          'virtual:other',
          RESOLVED_VIRTUAL_ID,
          ''
        ]) {
          expect(plugin.resolveId(id)).toBeUndefined();
        }
      });
    });

    describe('load', () => {
      it('is an empty list without an entry', () => {
        expect(pluginFor(null).load(RESOLVED_VIRTUAL_ID)).toBe('export default [];');
      });

      it('re-exports the default export of the normalized, JSON-quoted entry path', () => {
        expect(pluginFor(entryOutside).load(RESOLVED_VIRTUAL_ID)).toBe(
          `export { default } from ${JSON.stringify(entryOutside)};`
        );
        // Vite's normalizePath: on Windows this turns backslashes into slashes; everywhere
        // it collapses redundant segments, which is what a POSIX run can observe.
        expect(pluginFor('/tmp/ext//nested/../index.ts').load(RESOLVED_VIRTUAL_ID)).toBe(
          'export { default } from "/tmp/ext/index.ts";'
        );
        const awkward = '/tmp/with space/and "quote"/index.ts';
        expect(pluginFor(awkward).load(RESOLVED_VIRTUAL_ID)).toBe(
          'export { default } from "/tmp/with space/and \\"quote\\"/index.ts";'
        );
      });

      it('ignores every other id', () => {
        const plugin = pluginFor(entryOutside);
        expect(plugin.load(VIRTUAL_ID)).toBeUndefined();
        expect(plugin.load(entryOutside)).toBeUndefined();
      });
    });

    describe('config', () => {
      it('always dedupes svelte and the Tauri API', () => {
        for (const entry of [null, entryInRoot, entryOutside]) {
          const root = entry === entryInRoot ? join(scratch, 'root') : APP_ROOT;
          expect(configOf(pluginFor(entry, root)).resolve?.dedupe).toEqual([
            'svelte',
            '@tauri-apps/api'
          ]);
        }
      });

      it('leaves fs.allow alone without an entry or for one inside the root', () => {
        expect(configOf(pluginFor(null)).server).toBeUndefined();
        expect(configOf(pluginFor(entryInRoot, join(scratch, 'root'))).server).toBeUndefined();
      });

      it('allows the workspace root and the entry directory for an entry outside the root', () => {
        const allow = configOf(pluginFor(entryOutside)).server?.fs?.allow;
        // Setting `allow` replaces Vite's default of the workspace root, so it is restated.
        expect(allow).toEqual([searchForWorkspaceRoot(APP_ROOT), dirname(entryOutside)]);
      });
    });

    describe('configResolved', () => {
      it('logs the injected entry once', () => {
        const messages: string[] = [];
        const plugin = pluginFor(entryOutside);
        const config = { logger: { info: (message: string) => messages.push(message) } };
        plugin.configResolved(config);
        plugin.configResolved(config);
        expect(messages).toEqual([`injecting extensions from ${entryOutside}`]);
      });

      it('logs nothing without an entry', () => {
        const messages: string[] = [];
        pluginFor(null).configResolved({ logger: { info: (m: string) => messages.push(m) } });
        expect(messages).toEqual([]);
      });
    });
  });

  describe('wiring', () => {
    it('aliases the host API for SvelteKit, which writes it into the generated tsconfig', async () => {
      // Imported by a computed path so the config stays out of the type-check program.
      const configPath = join(APP_ROOT, 'svelte.config.js');
      const { default: svelteConfig } = (await import(configPath)) as {
        default: { kit?: { alias?: Record<string, string> } };
      };
      expect(svelteConfig.kit?.alias).toEqual({
        '@nodespace/extension-api': 'src/lib/extension-api'
      });
    });

    it('resolves @nodespace/extension-api under the unit-tier config, exposing the registration API', async () => {
      const api = await import('@nodespace/extension-api');
      // Explicit export list, no `export *`: the runtime surface is exactly the version.
      // Update this list when the barrel gains a runtime export.
      expect(Object.keys(api)).toEqual(['EXTENSION_API_VERSION']);
      expect(api.EXTENSION_API_VERSION).toBe(EXTENSION_API_VERSION);
    });

    it('re-exports the contribution types an extension is written against, unchanged', () => {
      // Compile-time only, enforced by svelte-check: it fails if the barrel drops or reshapes one.
      expectTypeOf<api.NodespaceExtension>().toEqualTypeOf<host.NodespaceExtension>();
      expectTypeOf<api.Contribution>().toEqualTypeOf<host.Contribution>();
      expectTypeOf<api.ChromeSlot>().toEqualTypeOf<host.ChromeSlot>();
      expectTypeOf<api.ChromeContribution>().toEqualTypeOf<host.ChromeContribution>();
      expectTypeOf<api.ViewerTabContribution>().toEqualTypeOf<host.ViewerTabContribution>();
      expectTypeOf<api.SettingsSectionContribution>().toEqualTypeOf<host.SettingsSectionContribution>();
      expectTypeOf<api.SettingsSlot>().toEqualTypeOf<host.SettingsSlot>();
      expectTypeOf<api.SettingsSlotContribution>().toEqualTypeOf<host.SettingsSlotContribution>();
      expectTypeOf<api.SettingsSlotContributionFor<'database.row'>>().toEqualTypeOf<
        host.SettingsSlotContributionFor<'database.row'>
      >();
    });

    it('registers the injected extensions synchronously in the root layout', () => {
      const layout = readFileSync(join(APP_ROOT, 'src/routes/+layout.svelte'), 'utf8');
      const script = layout.slice(0, layout.indexOf('</script>'));

      const injected = /^\s*import (\w+) from 'virtual:nodespace-extensions';$/m.exec(script);
      expect(injected, 'the layout imports the default export of the virtual module').not.toBeNull();
      const name = injected?.[1] ?? '';

      expect(script).toMatch(
        /^\s*import \{ registerExtensions \} from '\$lib\/plugins\/ui-extensions';$/m
      );
      // A statement at the top level of the script runs when the component initialises, before
      // AppShell renders. One indented deeper would sit inside a function or callback.
      expect(script, 'the layout passes the injected list to registerExtensions').toMatch(
        new RegExp(`^  registerExtensions\\(${name}\\);$`, 'm')
      );
    });
  });
});
