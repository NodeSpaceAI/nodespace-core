#!/usr/bin/env bun
/**
 * Build nodespaced and nodespace CLI sidecar binaries for local development.
 *
 * CI runs this step automatically (see .github/workflows). Locally, the
 * binaries in src-tauri/binaries/ are 0-byte git stubs and must be built
 * before `bun run dev:tauri` or `bun run tauri:build`.
 *
 * Usage:
 *   bun run build:sidecars          # release build (required for dev:tauri)
 *   bun run build:sidecars --debug  # debug build (faster, larger binary)
 */

import { $ } from 'bun';
import { mkdirSync } from 'fs';
import { join } from 'path';
import { arch } from 'os';

/**
 * `cargo build` flags for the requested profile, as an array rather than a
 * bare string -- an empty-string element still becomes a literal argument
 * under Bun's `$` templating (`cargo build '' --bin ...`), which cargo
 * rejects outright. An empty array contributes zero arguments to a `$`
 * template; a non-empty one flattens into separate args.
 */
export function cargoProfileFlags(isDebug: boolean): string[] {
  return isDebug ? [] : ['--release'];
}

if (import.meta.main) {
  const isDebug = process.argv.includes('--debug');
  const profile = isDebug ? 'debug' : 'release';
  const profileFlags = cargoProfileFlags(isDebug);

  // Detect host triple
  const hostArch = arch() === 'arm64' ? 'aarch64' : 'x86_64';
  const triple = `${hostArch}-apple-darwin`;

  const BIN_DIR = join(import.meta.dir, '..', 'packages', 'desktop-app', 'src-tauri', 'binaries');
  const TARGET_DIR = join(import.meta.dir, '..', 'target', profile);

  console.log(`Building sidecars (${profile}) for ${triple}...`);

  // binaries/ is .gitignore'd (never committed) so a fresh checkout/worktree
  // won't have the directory at all — create it before cp fails on a missing parent.
  mkdirSync(BIN_DIR, { recursive: true });

  await $`cargo build ${profileFlags} --bin nodespaced --bin nodespace`.quiet();

  for (const bin of ['nodespaced', 'nodespace']) {
    const src = join(TARGET_DIR, bin);
    const dest = join(BIN_DIR, `${bin}-${triple}`);
    await $`cp ${src} ${dest}`;
    await $`chmod +x ${dest}`;
    const size = (await $`du -sh ${dest}`.text()).split('\t')[0];
    console.log(`  ✓ ${dest} (${size})`);
  }

  console.log('Done. Restart nodespaced to pick up the new binary:');
  console.log('  launchctl kickstart -k gui/$(id -u)/app.nodespace.daemon');
}
