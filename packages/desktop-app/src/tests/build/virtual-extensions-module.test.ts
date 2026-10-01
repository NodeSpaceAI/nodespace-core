// @vitest-environment node
/**
 * `virtual:nodespace-extensions` loaded through a real Vite server, so the
 * plugin's `resolveId` / `load` pair is exercised together with Vite's own
 * resolution rather than called by hand.
 */
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer, type ViteDevServer } from 'vite';
import { afterAll, describe, expect, it } from 'vitest';
import { nodespaceExtensions } from '../../../vite-plugins/nodespace-extensions.js';

const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const FIXTURE_ENTRY = join(APP_ROOT, 'src/tests/fixtures/sample-extension-entry.ts');
const VIRTUAL_ID = 'virtual:nodespace-extensions';

describe('virtual:nodespace-extensions', () => {
  const servers: ViteDevServer[] = [];
  const scratchDirs: string[] = [];

  /** A module server with only the plugin: no file watcher and no HMR socket to leak or collide. */
  async function serverFor(entry: string | null): Promise<ViteDevServer> {
    const server = await createServer({
      configFile: false,
      root: APP_ROOT,
      appType: 'custom',
      logLevel: 'silent',
      server: { middlewareMode: true, hmr: false, watch: null },
      plugins: [nodespaceExtensions({ root: APP_ROOT, entry })]
    });
    servers.push(server);
    return server;
  }

  afterAll(async () => {
    await Promise.all(servers.map((server) => server.close()));
    for (const dir of scratchDirs) rmSync(dir, { recursive: true, force: true });
  });

  it('default-exports the entry’s array when a build injects one', async () => {
    const server = await serverFor(FIXTURE_ENTRY);
    const module = await server.ssrLoadModule(VIRTUAL_ID);
    expect(module.default).toEqual([{ id: 'sample-extension', apiVersion: 2 }]);
  });

  it('default-exports an empty list when nothing is injected, as in core', async () => {
    const server = await serverFor(null);
    const module = await server.ssrLoadModule(VIRTUAL_ID);
    expect(module.default).toEqual([]);
  });

  it('loads an entry outside the root whose imports only the root can resolve', async () => {
    // A temporary directory has no node_modules, so these bare imports resolve only
    // because the plugin dedupes them to the root's copies.
    const outside = mkdtempSync(join(tmpdir(), 'ns-virtual-extensions-'));
    scratchDirs.push(outside);
    const entry = join(outside, 'index.ts');
    writeFileSync(
      entry,
      [
        "import { mount } from 'svelte';",
        "import { invoke } from '@tauri-apps/api/core';",
        "export default [{ id: 'outside-extension', apiVersion: 2, imports: [typeof mount, typeof invoke] }];"
      ].join('\n')
    );

    const server = await serverFor(entry);
    const module = await server.ssrLoadModule(VIRTUAL_ID);
    expect(module.default).toEqual([
      { id: 'outside-extension', apiVersion: 2, imports: ['function', 'function'] }
    ]);
  });
});
