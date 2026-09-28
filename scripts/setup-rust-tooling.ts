#!/usr/bin/env bun
// Installs the Rust tooling the test gate needs into the repository itself.
//
// - cargo-nextest runs the Rust test suites (`rust:test`). It runs each test
//   in its own process, which is what lets the libsql-linked crates run at
//   full parallelism — see the race described in
//   packages/core/src/db/sqlite_store/mod.rs.
// - sccache is the compiler cache in front of every cargo build on the
//   machine: the gate's (scripts/test-gate.ts), and development builds, which
//   reach it through a generated, gitignored `.cargo/config.toml` in each
//   checkout whose rustc wrapper (in `.tools/bin`) hands sccache the gate's
//   settings — one server, one cache (see ./gate-sccache.ts). What it shares
//   across checkouts is llama.cpp's C/C++ build (measured: 97% hits, ~50s off
//   a fresh worktree's first build). Rust crates hit only within one target
//   dir — rustc's arguments carry the checkout's own target path, so every
//   checkout hashes differently (measured: 0 of 374 hit across two fresh
//   target dirs) — which is why the gate keeps one persistent checkout.
//
// Everything lives in `.tools/` in the primary checkout (gitignored), and
// each worktree gets a `.tools` link to it: one download per machine, shared
// by every worktree. Nothing is written outside the repository — no
// ~/.cargo/bin, no user-level cargo or sccache config.
//
// Runs from the root `prepare` script, i.e. on every `bun install`. Two rules
// follow: near-free when already set up (no network, no writes), and never
// fail the install — offline, unsupported platform, no cargo: warn and move
// on. (A missing nextest does fail `rust:test`, loudly, naming `bun install`
// as the fix.)
//
// Opt out with NODESPACE_SKIP_RUST_TOOLING=1.

import { chmodSync, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { $ } from "bun";
import { devCargoConfig, devRustcWrapper, GENERATED_MARKER } from "./gate-sccache";

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

/**
 * Whether a tool needs (re)installing: it is missing, or the version stamped
 * beside it isn't the pinned one. The stamp is what makes a version bump in
 * this file reach machines that already have an older binary.
 */
export function needsInstall(binaryExists: boolean, stampedVersion: string | null, pinnedVersion: string): boolean {
  return !binaryExists || stampedVersion?.trim() !== pinnedVersion;
}

/** The stamp file recording which version of a tool is installed. */
function stampPath(target: string): string {
  return `${target}.version`;
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

  // Staged inside .tools itself (gitignored), so even the download never
  // lands outside the repository.
  const work = mkdtempSync(join(dirname(dirname(target)), `.install-${tool.name}-`));
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
      writeFileSync(stampPath(target), tool.version);
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
  let present = false;
  try {
    lstatSync(link);
    present = true;
  } catch {
    // Nothing there yet.
  }
  // A link whose target is gone (the primary's .tools was cleaned) is
  // replaced; anything that resolves is left as it is.
  if (present && existsSync(link)) return;
  if (present) unlinkSync(link);
  symlinkSync(primaryTools, link);
}

/**
 * Writes `content` to `path` unless it already holds exactly that — the
 * near-free rule. Write-then-rename, because a build may be running the
 * wrapper right now.
 */
function writeIfChanged(path: string, content: string, mode: number): void {
  if (existsSync(path) && readFileSync(path, "utf8") === content) return;
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, content, { mode });
  renameSync(tmp, path);
}

/**
 * Points this checkout's cargo builds at the shared sccache: the wrapper in
 * the primary's `.tools/bin`, and a `.cargo/config.toml` naming it. A config
 * file this script didn't write is left alone, with a warning.
 */
function setUpDevCache(checkoutRoot: string, primaryTools: string): void {
  const wrapper = join(primaryTools, "bin", "rustc-wrapper");
  writeIfChanged(wrapper, devRustcWrapper(primaryTools), 0o755);
  const config = join(checkoutRoot, ".cargo", "config.toml");
  if (existsSync(config) && !readFileSync(config, "utf8").includes(GENERATED_MARKER)) {
    console.warn(`⚠ ${config} wasn't written by bun install; development builds won't share the compiler cache.`);
    return;
  }
  mkdirSync(dirname(config), { recursive: true });
  writeIfChanged(config, devCargoConfig(wrapper), 0o644);
}

async function main(): Promise<void> {
  if (process.env[SKIP_ENV_VAR] === "1") return;
  if (process.platform !== "darwin") return;
  // No Rust toolchain means nothing to use these tools with — a frontend-only
  // machine. (Looking for cargo is a read, not a write.)
  if (!Bun.which("cargo") && !existsSync(join(homedir(), ".cargo", "bin", "cargo"))) return;

  const checkoutRoot = (await $`git rev-parse --show-toplevel`.quiet().text()).trim();
  const commonDir = (await $`git rev-parse --path-format=absolute --git-common-dir`.quiet().text()).trim();
  const primaryTools = join(primaryRootFromCommonDir(commonDir), TOOLS_DIR);
  mkdirSync(join(primaryTools, "bin"), { recursive: true });
  if (resolve(checkoutRoot) !== primaryRootFromCommonDir(commonDir)) linkTools(checkoutRoot, primaryTools);

  // Each independently: one failing (offline, say) mustn't stop the other.
  for (const tool of [NEXTEST, SCCACHE]) {
    const target = join(primaryTools, "bin", tool.name);
    const release = releaseFor(tool);
    if (release === undefined) continue;
    const stamped = existsSync(stampPath(target)) ? readFileSync(stampPath(target), "utf8") : null;
    if (!needsInstall(existsSync(target), stamped, tool.version)) continue;
    try {
      await install(tool, release, target);
    } catch (err) {
      console.warn(`⚠ Skipped ${tool.name} setup: ${err instanceof Error ? err.message : String(err)}`);
      console.warn(`  Re-run \`bun install\` once it's fixed. Set ${SKIP_ENV_VAR}=1 to silence this.`);
    }
  }
  if (existsSync(join(primaryTools, "bin", SCCACHE.name))) setUpDevCache(checkoutRoot, primaryTools);
}

if (import.meta.main) {
  try {
    await main();
  } catch (err) {
    console.warn(`⚠ Skipped Rust tooling setup: ${err instanceof Error ? err.message : String(err)}`);
  }
}
