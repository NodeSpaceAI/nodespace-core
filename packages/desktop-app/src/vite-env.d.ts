/// <reference types="vite/client" />

// Compile-time constant injected by Vite's `define` (see vite.config.js). Holds
// this package's own version (from package.json) so the app can report its
// frontend version without importing package.json at runtime.
declare const __APP_VERSION__: string;

/**
 * The extensions a build injects, from the module `NODESPACE_EXTENSIONS` names
 * (see `vite-plugins/nodespace-extensions.js`). The build provides them, and
 * the list is empty in core (ADR-082 §3.1).
 */
declare module 'virtual:nodespace-extensions' {
  const extensions: readonly import('@nodespace/extension-api').NodespaceExtension[];
  export default extensions;
}
