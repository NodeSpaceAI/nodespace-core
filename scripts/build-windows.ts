#!/usr/bin/env bun
/**
 * Tier 2 local Windows build: Parallels Desktop VM automation, full CI
 * parity (both NSIS and .msi installers, real MSVC toolchain, code signing
 * if the VM has a certificate installed).
 *
 * This script only DRIVES an already-configured VM -- it never provisions
 * one. Parallels Desktop and a VM named `NodeSpace-Build-Win` (Rust MSVC
 * toolchain + WiX Toolset installed) are not present on this machine, and
 * setting either up is out of scope here (see this issue's acceptance
 * criteria: Tier 2 must "gracefully error with setup instructions if
 * Parallels or VM not found", not attempt to provision either). Every run
 * on this machine takes that path -- see `checkPrerequisites` below.
 *
 * The remainder (start VM, SSH in, build, copy artifacts back, stop VM) is
 * implemented per this issue's technical spec but is UNVERIFIED against a
 * real VM -- there is none to test against here. It is structurally sound
 * (types check, control flow matches the spec) but the SSH host/user/shell
 * assumptions below are exactly the kind of thing that needs confirming
 * against a real VM the first time one is configured -- see the
 * local-builds development doc's "Tier 2 -- one-time VM setup" section for
 * the manual verification checklist and the env vars that override these
 * defaults.
 *
 * Usage:
 *   bun run build:windows                # start VM, build, copy artifacts, stop VM
 *   bun run build:windows --keep-running  # leave the VM running afterward
 */

import { $ } from 'bun';
import { join } from 'node:path';

export const VM_NAME = 'NodeSpace-Build-Win';
const TARGET = 'x86_64-pc-windows-msvc';
const WORKSPACE_ROOT = join(import.meta.dir, '..');
const OUTPUT_DIR = join(WORKSPACE_ROOT, 'target', 'windows-release');

/**
 * `process.env.X ?? fallback` only catches null/undefined, not an
 * explicitly-empty-string override -- `NODESPACE_WIN_VM_REPO_PATH=` (a real
 * shell/CI misconfiguration shape: the var is set but empty) would silently
 * produce `''` instead of the documented default. Treat empty-string the
 * same as unset for all three of these.
 */
export function envOrDefault(name: string, fallback: string): string {
  const value = process.env[name];
  return value && value.length > 0 ? value : fallback;
}

// Overridable for a VM whose one-time setup didn't match these defaults --
// see the local-builds development doc.
const SSH_HOST = envOrDefault('NODESPACE_WIN_VM_HOST', 'nodespace-build-win.shared');
const SSH_USER = envOrDefault('NODESPACE_WIN_VM_USER', 'nodespace');
// Assumes a POSIX-ish default shell on the OpenSSH server side (e.g. Git
// Bash configured as the OpenSSH DefaultShell) -- the common setup for a
// Windows box whose whole purpose is running a Unix-flavored Rust/cargo
// toolchain. If the VM's SSH server defaults to PowerShell/cmd instead,
// override the commands run below accordingly (see local-builds.md).
const REPO_PATH = envOrDefault('NODESPACE_WIN_VM_REPO_PATH', '~/nodespace-core');

export function setupInstructions(): string {
  return (
    'Tier 2 (`bun run build:windows`) requires Parallels Desktop and a pre-configured\n' +
    `Windows VM named "${VM_NAME}" (Rust MSVC toolchain + WiX Toolset installed).\n\n` +
    'One-time setup is documented in the local-builds development doc\n' +
    '("Tier 2 -- one-time VM setup").\n\n' +
    'Until that VM exists, use Tier 1 for a fast, no-VM cross-compiled NSIS build instead:\n' +
    '  bun run build:windows:quick\n'
  );
}

async function commandExists(cmd: string): Promise<boolean> {
  try {
    await $`which ${cmd}`.quiet();
    return true;
  } catch {
    return false;
  }
}

/**
 * Both prerequisites this script needs before it may attempt anything else:
 * `prlctl` on PATH (Parallels Desktop installed) and a VM actually
 * registered under `VM_NAME`. Neither is provisioned here -- see module doc.
 */
async function checkPrerequisites(): Promise<boolean> {
  if (!(await commandExists('prlctl'))) {
    console.error('error: `prlctl` (Parallels Desktop CLI) is not installed or not on PATH.\n');
    console.error(setupInstructions());
    return false;
  }

  try {
    // `prlctl list -a` (not the running-only default `prlctl list`) so a
    // currently-stopped VM is still found.
    const listing = await $`prlctl list -a --json`.quiet().text();
    const vms = JSON.parse(listing) as Array<{ name: string }>;
    if (vms.some((vm) => vm.name === VM_NAME)) return true;
  } catch (err) {
    console.error(
      `error: \`prlctl list -a --json\` failed: ${err instanceof Error ? err.message : String(err)}\n`,
    );
    console.error(setupInstructions());
    return false;
  }

  console.error(`error: no Parallels VM named "${VM_NAME}" is registered.\n`);
  console.error(setupInstructions());
  return false;
}

async function isVmRunning(): Promise<boolean> {
  // Guarded the same way checkPrerequisites()'s `prlctl list -a --json` call
  // is: an unexpected shape here shouldn't surface as a raw JSON.parse
  // exception. Called on the common path (deciding whether to start the VM
  // at all), so a parse failure is treated as "not confirmed running" --
  // main() will then attempt `prlctl start`, which is the safe default.
  try {
    const listing = await $`prlctl list --json`.quiet().text();
    const running = JSON.parse(listing) as Array<{ name: string }>;
    return running.some((vm) => vm.name === VM_NAME);
  } catch (err) {
    console.error(
      `warning: \`prlctl list --json\` failed while checking VM state: ${err instanceof Error ? err.message : String(err)}`,
    );
    return false;
  }
}

async function waitForSsh(timeoutMs = 120_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  const sshProbe = () =>
    $`ssh -o ConnectTimeout=5 -o BatchMode=yes -o StrictHostKeyChecking=accept-new ${SSH_USER}@${SSH_HOST} exit`.quiet();

  while (Date.now() < deadline) {
    try {
      await sshProbe();
      return;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 5_000));
    }
  }
  throw new Error(
    `Timed out waiting for SSH on ${SSH_USER}@${SSH_HOST} after ${Math.round(timeoutMs / 1000)}s. ` +
      'Confirm the VM booted, Shared Networking is enabled, and the hostname/user match your VM setup ' +
      '(override with NODESPACE_WIN_VM_HOST / NODESPACE_WIN_VM_USER).',
  );
}

/**
 * The remote equivalent of release.yml's Windows `build-tauri` matrix leg:
 * pull latest, build both sidecars, build the Tauri bundle (both NSIS and
 * .msi -- unlike Tier 1, a real Windows host can produce the .msi too).
 * Skill-installer staging (`bun run build:skill`) is included since it runs
 * natively on this host, not cross-compiled -- no target-triple mismatch
 * here the way Tier 1 has.
 *
 * Pure string-building, exported for testing -- the SSH invocation itself
 * (`runRemoteBuild`) is what's actually unverified against a real VM (see
 * module doc); this at least pins down exactly which commands it would run.
 */
export function buildRemoteScript(repoPath: string): string {
  return [
    // Single-quoted: the whole joined script is sent as one command line to
    // the remote shell with no further escaping applied, so an unquoted
    // repoPath containing a space (a real shape -- NODESPACE_WIN_VM_REPO_PATH
    // is operator-configurable, and Git-Bash-style Windows paths like
    // `/c/Users/Build Machine/nodespace-core` are exactly this) would word-
    // split into an unexpected extra `cd` argument and fail the whole
    // `&&`-chained build at the very first step.
    `cd '${repoPath}'`,
    'git pull',
    'bun install --frozen-lockfile',
    'bun run --cwd packages/desktop-app sync',
    // Same step release.yml's build-tauri-macos-arm job runs before `tauri
    // build`: tauri.conf.json's `bundle.resources` includes
    // `resources/models/**/*`, which is gitignored and empty on a fresh
    // checkout, so without this the produced .msi/.exe silently ships
    // without the embedding model -- the exact bug this VM build path
    // exists to reproduce with "full CI parity" (see module doc above).
    // `--skip-existing`, not `--clobber`: unlike a CI runner, this VM's
    // checkout persists across runs, so a model already downloaded by a
    // previous build should be reused rather than re-fetched (146MB) every
    // time. Requires `gh` to be installed and authenticated on the VM --
    // add to the one-time VM setup alongside the Rust/WiX prerequisites.
    'mkdir -p packages/desktop-app/src-tauri/resources/models',
    'gh release download models-v2 --pattern "nomic-embed-text-v1.5.Q8_0.gguf" --dir packages/desktop-app/src-tauri/resources/models/ --skip-existing',
    `cargo build --release --bin nodespaced --target ${TARGET}`,
    `cargo build --release --bin nodespace --target ${TARGET}`,
    'mkdir -p packages/desktop-app/src-tauri/binaries',
    `cp target/${TARGET}/release/nodespaced.exe packages/desktop-app/src-tauri/binaries/nodespaced-${TARGET}.exe`,
    `cp target/${TARGET}/release/nodespace.exe packages/desktop-app/src-tauri/binaries/nodespace-${TARGET}.exe`,
    'bun run build:skill',
    `bunx tauri build --target ${TARGET}`,
  ].join(' && ');
}

async function runRemoteBuild(): Promise<void> {
  await $`ssh ${SSH_USER}@${SSH_HOST} ${buildRemoteScript(REPO_PATH)}`;
}

/**
 * The two `user@host:path` scp sources for the bundle directories, built as
 * complete strings rather than assembled from separate `${}` pieces in the
 * `$` template that consumes them. Pure -- exported for testing, same
 * reasoning as `buildRemoteScript` above.
 *
 * Two real bugs, both caught by adversarial review, both from letting Bun's
 * `$` see a bare `${remoteBundleDir}/...` segment instead of a complete
 * pre-built string:
 *   1. An earlier version had no `user@host:` prefix on the second (msi)
 *      source at all, so scp treated it as a LOCAL path.
 *   2. `REPO_PATH` defaults to `~/nodespace-core` (matching the documented
 *      Tier 2 VM setup), so `remoteBundleDir` starts with `~` -- and Bun's
 *      `$` tilde-expands any interpolated value that itself starts with `~`,
 *      against THIS (macOS) host, before scp ever sees it. That spliced this
 *      Mac's own home directory into what must stay a purely remote path.
 * Building the full `user@host:...` string here first avoids both: the
 * leading character `$` sees is the SSH user, never `~`.
 */
export function buildScpSources(
  sshUser: string,
  sshHost: string,
  remoteBundleDir: string,
): { nsis: string; msi: string } {
  return {
    nsis: `${sshUser}@${sshHost}:${remoteBundleDir}/nsis`,
    msi: `${sshUser}@${sshHost}:${remoteBundleDir}/msi`,
  };
}

async function copyArtifactsBack(): Promise<void> {
  await $`mkdir -p ${OUTPUT_DIR}`;
  // TARGET, not a re-typed literal -- buildRemoteScript() already builds the
  // remote paths this reads back from via ${TARGET} everywhere; a second,
  // independently-typed copy here would silently drift from it if TARGET
  // ever changes (e.g. an arm64 Windows target added).
  const remoteBundleDir = `${REPO_PATH}/target/${TARGET}/release/bundle`;
  const { nsis, msi } = buildScpSources(SSH_USER, SSH_HOST, remoteBundleDir);
  await $`scp -r ${nsis} ${msi} ${OUTPUT_DIR}/`;
}

async function main(): Promise<void> {
  if (process.platform !== 'darwin') {
    console.error(
      'error: build:windows (Tier 2) automates Parallels Desktop via `prlctl`, which only runs on macOS.',
    );
    process.exit(1);
  }

  if (!(await checkPrerequisites())) process.exit(1);

  const keepRunning = process.argv.includes('--keep-running');
  const alreadyRunning = await isVmRunning();

  // `prlctl start` runs INSIDE the try (not before it) so the `finally`
  // below always gets a chance to attempt a stop -- including the case
  // where `start` itself rejects after the VM has actually begun starting
  // (a plausible prlctl failure mode: a timeout waiting for a running-state
  // confirmation, or a post-start check failing while the VM process is
  // already alive). A `prlctl stop` against a VM that in fact never started
  // is a safe no-op either way (see its own `.nothrow()` below).
  try {
    if (!alreadyRunning) {
      console.log(`==> Starting VM "${VM_NAME}"...`);
      await $`prlctl start ${VM_NAME}`;
    } else {
      console.log(`==> VM "${VM_NAME}" is already running.`);
    }

    console.log(`==> Waiting for SSH on ${SSH_USER}@${SSH_HOST}...`);
    await waitForSsh();

    console.log('==> VM is reachable over SSH. Running remote build (this mirrors release.yml -- expect several minutes)...');
    await runRemoteBuild();

    console.log('==> Copying artifacts back...');
    await copyArtifactsBack();

    console.log(`\nDone. Artifacts: ${OUTPUT_DIR}/`);
  } finally {
    if (keepRunning || alreadyRunning) {
      console.log(`\n(VM left running. Stop it manually with: prlctl stop "${VM_NAME}")`);
    } else {
      console.log(`==> Stopping VM "${VM_NAME}"...`);
      // `.nothrow()` so a failed stop doesn't mask whatever error (if any)
      // is already propagating out of this finally -- but its exit code is
      // still checked and reported, rather than discarded outright. Without
      // this, a failed stop (wedged VM, Parallels hiccup) printed the exact
      // same "Stopping VM..." message as a successful one, silently
      // indistinguishable from success.
      const stopResult = await $`prlctl stop ${VM_NAME}`.nothrow();
      if (stopResult.exitCode !== 0) {
        console.error(
          `warning: \`prlctl stop "${VM_NAME}"\` exited with code ${stopResult.exitCode} -- ` +
            'the VM may still be running. Check with `prlctl list -a` and stop it manually if needed.',
        );
      }
    }
  }
}

if (import.meta.main) {
  main().catch((err) => {
    console.error('\nbuild-windows (Tier 2) failed:', err.message ?? err);
    process.exit(1);
  });
}
