// The build-time seam for extensions (ADR-082 §2.1).
//
// `virtual:nodespace-extensions` default-exports the extension list a build
// injects. In core there is no entry and the list is empty. With an entry, the
// virtual module re-exports the entry's default export, so core never imports
// an extension by name.

import { dirname, isAbsolute, relative, sep } from 'node:path';
import { normalizePath, searchForWorkspaceRoot } from 'vite';

const VIRTUAL_ID = 'virtual:nodespace-extensions';
// Vite's convention: a NUL prefix keeps other plugins from treating the id as a file path.
const RESOLVED_VIRTUAL_ID = `\0${VIRTUAL_ID}`;

/**
 * Whether `entry` lies outside `root`.
 *
 * @param {string} root
 * @param {string} entry
 * @returns {boolean}
 */
function isOutside(root, entry) {
  const rel = relative(root, entry);
  return rel === '..' || rel.startsWith(`..${sep}`) || isAbsolute(rel);
}

/**
 * @param {{ root: string, entry: string | null }} options `root` is the app
 *   directory; `entry` is the absolute extensions entry from
 *   `resolveExtensionsEntry`, or null for none
 * @returns {import('vite').Plugin}
 */
export function nodespaceExtensions({ root, entry }) {
  let announced = false;

  return {
    name: 'nodespace-extensions',
    enforce: 'pre',

    config() {
      /** @type {import('vite').UserConfig} */
      const config = {
        // An entry outside `root` must not bring its own copy of these: a second
        // Svelte runtime breaks components, a second Tauri API breaks IPC.
        resolve: { dedupe: ['svelte', '@tauri-apps/api'] }
      };
      if (entry !== null && isOutside(root, entry)) {
        // Setting `allow` replaces Vite's default of the workspace root, so restate it.
        config.server = { fs: { allow: [searchForWorkspaceRoot(root), dirname(entry)] } };
      }
      return config;
    },

    configResolved(config) {
      if (entry === null || announced) return;
      announced = true;
      config.logger.info(`injecting extensions from ${entry}`);
    },

    resolveId(id) {
      return id === VIRTUAL_ID ? RESOLVED_VIRTUAL_ID : undefined;
    },

    load(id) {
      if (id !== RESOLVED_VIRTUAL_ID) return undefined;
      if (entry === null) return 'export default [];';
      return `export { default } from ${JSON.stringify(normalizePath(entry))};`;
    }
  };
}
