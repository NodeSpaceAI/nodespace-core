#!/usr/bin/env bun
/**
 * Thin wrapper around the existing `tauri:build` pipeline, for
 * naming/discoverability consistency with `build:windows:quick` and
 * `build:windows` (see the local-builds development doc).
 *
 * `bun run tauri:build` (packages/desktop-app/package.json) already does
 * everything the macOS leg of .github/workflows/release.yml's
 * `build-tauri-macos-arm` job does, minus what only CI has: a Homebrew
 * Protobuf step this repo's dev setup already expects to be installed, the
 * embedding-model download (a separate one-time step -- see
 * `bun run download:models`), and Apple code signing/notarization (no
 * `APPLE_*` secrets locally, so the build produces an unsigned, unnotarized
 * .app/.dmg -- fine for local inspection, not for distribution).
 *
 * Usage:
 *   bun run build:macos
 */

import { $ } from 'bun';
import { arch } from 'node:os';
import { join } from 'node:path';

const WORKSPACE_ROOT = join(import.meta.dir, '..');

async function main(): Promise<void> {
  if (process.platform !== 'darwin') {
    console.error(`error: build:macos only runs on macOS (this machine reports "${process.platform}").`);
    process.exit(1);
  }

  if (arch() !== 'arm64') {
    console.warn(
      'warning: this machine is not Apple Silicon (arm64) -- `tauri:build` will build for the host ' +
        'architecture instead of the "macOS ARM" build the CI release pipeline ships. Pass an explicit ' +
        '--target to `bunx tauri build` directly if you specifically need an aarch64-apple-darwin build ' +
        'cross-compiled from an Intel host.',
    );
  }

  console.log('Building macOS app (.app + .dmg) via tauri:build...');
  await $`bun run tauri:build`.cwd(WORKSPACE_ROOT);
  console.log(
    '\nDone. Bundle output: target/<host-triple>/release/bundle/macos/ (.app) and .../dmg/ (.dmg).',
  );
}

if (import.meta.main) {
  main().catch((err) => {
    console.error('\nbuild-macos failed:', err.message ?? err);
    process.exit(1);
  });
}
