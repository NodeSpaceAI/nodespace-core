/**
 * Test helpers for extension code (ADR-082 §2.6), imported as
 * `@nodespace/extension-api/testing`. Part of the versioned host API; see the
 * compatibility policy in `./index.ts`.
 *
 * Test-only: it imports Vitest, and app code never imports this entry, so it
 * never reaches a bundle. The other entries never import it either;
 * `extension-api-boundary.test.ts` holds both.
 *
 * The rune mocks and setup files are not exports. Core's `vitest.config.ts`
 * applies them (its setup files and the rune-mock plugin) to every test it runs,
 * and it runs the tests of the entry a build injects through
 * `NODESPACE_EXTENSIONS`.
 *
 * - `mockTauriCore` builds a `vi.mock('@tauri-apps/api/core', ...)` factory, as
 *   core's own tests use it.
 * - `render`, `screen`, `fireEvent`, `waitFor` and `cleanup` come from
 *   `@testing-library/svelte`. They are re-exported because extension code may
 *   import only the host API, `svelte` and `@tauri-apps/api`.
 */

export { mockTauriCore } from '../../tests/helpers/mock-tauri-core';
export { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
