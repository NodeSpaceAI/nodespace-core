#!/usr/bin/env bun
// Installs the Rust tooling the test gate needs into the repository itself.
//
// - cargo-nextest runs the Rust test suites (`rust:test`). It runs each test
//   in its own process, which is what lets the libsql-linked crates run at
//   full parallelism — see the race described in
//   packages/core/src/db/sqlite_store/mod.rs.
// - sccache is the compiler cache the gate puts in front of its own cargo
//   builds (scripts/test-gate.ts), so a new worktree reuses the dependency
//   builds of the ones before it.
//
// Everything lives in `.tools/` in the primary checkout (gitignored), and
// each worktree gets a `.tools` link to it: one download per machine, shared
// by every worktree. Nothing is written outside the repository — no
// ~/.cargo/bin, no cargo config, no sccache config. The gate hands sccache
// its settings through environment variables, so only gate builds use it and
// a developer's own cargo builds are untouched.
//
// Runs from the root `prepare` script, i.e. on every `bun install`. Two rules
// follow: near-free when already set up (no network, no writes), and never
// fail the install — offline, unsupported platform, no cargo: warn and move
// on. (A missing nextest does fail `rust:test`, loudly, naming `bun install`
// as the fix.)
//
// Opt out with NODESPACE_SKIP_RUST_TOOLING=1.

import { chmodSync, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";

export const SKIP_ENV_VAR = "NODESPACE_SKIP_RUST_TOOLING";

/** The tools directory, relative to any checkout's root. */
export const TOOLS_DIR = ".tools";

/** A pinned release of a tool, per platform. */
export interface ToolRelease {
  url: string;
  /**
   * Pinned here rather than fetched alongside the archive: a checksum served
   * from the same place as the binary proves only that the download finished.
   */
  sha256: string;
  /** Path of the binary inside the extracted archive. */
  binary: string;
}

export interface Tool {
  name: string;
  version: string;
  releases: Record<string, ToolRelease>;
}

const SCCACHE_VERSION = "0.18.0";
const NEXTEST_VERSION = "0.9.146";
const NEXTEST_MAC: ToolRelease = {
  url: `https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${NEXTEST_VERSION}/cargo-nextest-${NEXTEST_VERSION}-universal-apple-darwin.tar.gz`,
  sha256: "39785160b3c2f6ed9a765049cf4fa79f3b39aa02eb7598a5a0e2a1a0b9ffb9a8",
  binary: "cargo-nextest",
};

export const SCCACHE: Tool = {
  name: "sccache",
  version: SCCACHE_VERSION,
  releases: {
    "darwin-arm64": {
      url: `https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/sccache-v${SCCACHE_VERSION}-aarch64-apple-darwin.tar.gz`,
      sha256: "308184519b646f5125289e8515b36f6ca65a13a041923994aebe702348674e8e",
      binary: `sccache-v${SCCACHE_VERSION}-aarch64-apple-darwin/sccache`,
    },
  },
};

// A universal binary, so both Apple Silicon and Intel Macs get it.
export const NEXTEST: Tool = {
  name: "cargo-nextest",
  version: NEXTEST_VERSION,
  releases: { "darwin-arm64": NEXTEST_MAC, "darwin-x64": NEXTEST_MAC },
};

export function sha256Hex(bytes: Uint8Array): string {
  return new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
}

/** This platform's pinned release of `tool`, or undefined when there is none. */
export function releaseFor(tool: Tool, platform: string = process.platform, arch: string = process.arch): ToolRelease | undefined {
  return tool.releases[`${platform}-${arch}`];
}

/** The primary checkout's root, from `git rev-parse --git-common-dir` (its `.git`). */
export function primaryRootFromCommonDir(commonDir: string): string {
  return dirname(resolve(commonDir));
}

async function install(tool: Tool, release: ToolRelease, target: string): Promise<void> {
  console.log(`▶ Installing ${tool.name} ${tool.version} into ${dirname(target)}`);
  const response = await fetch(release.url, { signal: AbortSignal.timeout(60_000) });
  if (!response.ok) {
    throw new Error(`${tool.name} download failed: HTTP ${response.status} for ${release.url}`);
  }
  const archive = new Uint8Array(await response.arrayBuffer());
  const actual = sha256Hex(archive);
  if (actual !== release.sha256) {
    throw new Error(`checksum mismatch for ${tool.name} (expected ${release.sha256}, got ${actual})`);
  }

  const work = mkdtempSync(join(tmpdir(), `${tool.name}-install-`));
  try {
    const archivePath = join(work, "archive.tar.gz");
    writeFileSync(archivePath, archive);
    await $`tar -xzf ${archivePath} -C ${work}`.quiet();
    // Copy-then-rename: another worktree's gate may be running this binary
    // right now, and rewriting a running binary in place on macOS gets it
    // SIGKILLed for an invalid code signature.
    mkdirSync(dirname(target), { recursive: true });
    const tmp = `${target}.${process.pid}.tmp`;
    try {
      copyFileSync(join(work, release.binary), tmp);
      chmodSync(tmp, 0o755);
      renameSync(tmp, target);
    } catch (err) {
      rmSync(tmp, { force: true });
      throw err;
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

/**
 * Points this checkout's `.tools` at the primary checkout's, so every
 * worktree shares one set of tools and one compiler cache.
 */
function linkTools(checkoutRoot: string, primaryTools: string): void {
  const link = join(checkoutRoot, TOOLS_DIR);
  try {
    lstatSync(link);
    return; // Already there — a link from an earlier install, or the primary's own directory.
  } catch {
    symlinkSync(primaryTools, link);
  }
}

async function main(): Promise<void> {
  if (process.env[SKIP_ENV_VAR] === "1") return;
  if (process.platform !== "darwin") return;

  const checkoutRoot = (await $`git rev-parse --show-toplevel`.quiet().text()).trim();
  const commonDir = (await $`git rev-parse --path-format=absolute --git-common-dir`.quiet().text()).trim();
  const primaryTools = join(primaryRootFromCommonDir(commonDir), TOOLS_DIR);
  mkdirSync(join(primaryTools, "bin"), { recursive: true });
  if (resolve(checkoutRoot) !== primaryRootFromCommonDir(commonDir)) linkTools(checkoutRoot, primaryTools);

  // Each independently: one failing (offline, say) mustn't stop the other.
  for (const tool of [NEXTEST, SCCACHE]) {
    const target = join(primaryTools, "bin", tool.name);
    const release = releaseFor(tool);
    if (existsSync(target) || release === undefined) continue;
    try {
      await install(tool, release, target);
    } catch (err) {
      console.warn(`⚠ Skipped ${tool.name} setup: ${err instanceof Error ? err.message : String(err)}`);
      console.warn(`  Re-run \`bun install\` once it's fixed. Set ${SKIP_ENV_VAR}=1 to silence this.`);
    }
  }
}

if (import.meta.main) {
  try {
    await main();
  } catch (err) {
    console.warn(`⚠ Skipped Rust tooling setup: ${err instanceof Error ? err.message : String(err)}`);
  }
}
