#!/usr/bin/env bun
// Sets up sccache as this machine's shared Rust compiler cache.
//
// Every worktree has its own target/, so without a shared cache each new
// worktree compiles every crates.io dependency from scratch — minutes of the
// pre-push gate (ADR-047) on its first push. sccache sits in front of rustc
// (and the C/C++ compilers behind libsql and llama.cpp) and serves those
// compiles from one machine-wide cache, so only the first worktree pays.
// It does not cache incremental compiles, so the workspace's own crates
// still build per worktree.
//
// Runs from the root `prepare` script, i.e. on every `bun install`, because
// that is the one step every dev machine is guaranteed to run. That makes
// three rules non-negotiable:
//   - Near-free when already set up: no network, no writes.
//   - Never fails the install. Offline, unsupported platform, no cargo — warn
//     and move on; a missing cache only costs build time.
//   - Never clobbers a hand-edited ~/.cargo/config.toml. When a safe append
//     isn't possible, print the lines to add instead.
//
// The cache is local to each machine. A shared remote cache only hits when
// rustc and absolute paths match byte-for-byte across machines, and the
// per-machine win (many worktrees sharing one cache) is most of the benefit.
//
// Opt out with NODESPACE_SKIP_BUILD_CACHE=1.

import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { $ } from "bun";

export const SKIP_ENV_VAR = "NODESPACE_SKIP_BUILD_CACHE";

export const SCCACHE_VERSION = "0.18.0";

// Pinned here rather than fetched alongside the archive: a checksum served
// from the same place as the binary proves only that the download finished.
export const SCCACHE_RELEASES: Record<string, { asset: string; sha256: string }> = {
  "darwin-arm64": {
    asset: `sccache-v${SCCACHE_VERSION}-aarch64-apple-darwin`,
    sha256: "308184519b646f5125289e8515b36f6ca65a13a041923994aebe702348674e8e",
  },
};

export const CACHE_SIZE_BYTES = 40 * 1024 ** 3;

export type CargoConfigPlan =
  | { action: "write"; content: string }
  | { action: "skip"; reason: string }
  | { action: "manual"; reason: string; lines: string };

export function cargoConfigBlock(sccachePath: string): string {
  // JSON string escaping is a valid TOML basic string for any path.
  const wrapper = JSON.stringify(sccachePath);
  return [
    "# Machine-wide compiler cache shared by every checkout and worktree —",
    "# written by nodespace-core scripts/setup-build-cache.ts.",
    "[build]",
    `rustc-wrapper = ${wrapper}`,
    "",
    "[env]",
    `CMAKE_C_COMPILER_LAUNCHER = ${wrapper}`,
    `CMAKE_CXX_COMPILER_LAUNCHER = ${wrapper}`,
    "",
  ].join("\n");
}

/**
 * Decides what to do with ~/.cargo/config.toml. `existing` is its contents,
 * or null when the file doesn't exist.
 */
export function planCargoConfig(existing: string | null, sccachePath: string): CargoConfigPlan {
  const block = cargoConfigBlock(sccachePath);
  if (existing === null || existing.trim() === "") {
    return { action: "write", content: block };
  }

  let parsed: Record<string, unknown>;
  try {
    parsed = Bun.TOML.parse(existing) as Record<string, unknown>;
  } catch {
    return { action: "manual", reason: "~/.cargo/config.toml doesn't parse as TOML", lines: block };
  }

  const build = parsed.build as Record<string, unknown> | undefined;
  if (build?.["rustc-wrapper"] !== undefined) {
    return { action: "skip", reason: `rustc-wrapper already set (${String(build["rustc-wrapper"])})` };
  }

  // Appending a second [build] or [env] table is a TOML error that would
  // break every cargo invocation on the machine — hand those to the user.
  if (build !== undefined || parsed.env !== undefined) {
    return {
      action: "manual",
      reason: "~/.cargo/config.toml already has a [build] or [env] table",
      lines: block,
    };
  }

  const separator = existing.endsWith("\n") ? "\n" : "\n\n";
  return { action: "write", content: `${existing}${separator}${block}` };
}

export function sccacheConfigContent(): string {
  return `[cache.disk]\nsize = ${CACHE_SIZE_BYTES} # 40 GiB\n`;
}

// SCCACHE_CONF is sccache's own override for where it reads its config.
function sccacheConfigPath(): string {
  return process.env.SCCACHE_CONF ?? join(homedir(), "Library", "Application Support", "Mozilla.sccache", "config");
}

function cargoConfigPath(): string {
  return join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "config.toml");
}

function cargoBinDir(): string {
  return join(process.env.CARGO_HOME ?? join(homedir(), ".cargo"), "bin");
}

function findOnPath(name: string): string | null {
  return Bun.which(name) ?? (existsSync(join(cargoBinDir(), name)) ? join(cargoBinDir(), name) : null);
}

// Write-then-rename so a concurrent `bun install` in another worktree never
// sees a half-written config.
function writeAtomically(path: string, content: string): void {
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, content);
  renameSync(tmp, path);
}

export function sha256Hex(bytes: Uint8Array): string {
  return new Bun.CryptoHasher("sha256").update(bytes).digest("hex");
}

async function installSccache(): Promise<string> {
  const release = SCCACHE_RELEASES[`${process.platform}-${process.arch}`];
  if (release === undefined) {
    throw new Error(`no pinned sccache build for ${process.platform}-${process.arch} — install sccache manually`);
  }

  const url = `https://github.com/mozilla/sccache/releases/download/v${SCCACHE_VERSION}/${release.asset}.tar.gz`;
  const response = await fetch(url, { signal: AbortSignal.timeout(60_000) });
  if (!response.ok) {
    throw new Error(`download failed: HTTP ${response.status} for ${url}`);
  }
  const archive = new Uint8Array(await response.arrayBuffer());
  const actual = sha256Hex(archive);
  if (actual !== release.sha256) {
    throw new Error(`checksum mismatch for ${release.asset} (expected ${release.sha256}, got ${actual})`);
  }

  const work = mkdtempSync(join(tmpdir(), "sccache-install-"));
  try {
    const archivePath = join(work, "sccache.tar.gz");
    writeFileSync(archivePath, archive);
    await $`tar -xzf ${archivePath} -C ${work}`.quiet();
    const target = join(cargoBinDir(), "sccache");
    mkdirSync(cargoBinDir(), { recursive: true });
    copyFileSync(join(work, release.asset, "sccache"), target);
    chmodSync(target, 0o755);
    return target;
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

async function main(): Promise<void> {
  if (process.env[SKIP_ENV_VAR] === "1") return;
  if (process.platform !== "darwin") return;
  // No Rust toolchain means nothing to cache — a frontend-only machine.
  if (findOnPath("cargo") === null) return;
  // Someone already routes rustc through a wrapper of their own choosing.
  if (process.env.RUSTC_WRAPPER) return;

  const configPath = cargoConfigPath();
  const existing = existsSync(configPath) ? readFileSync(configPath, "utf8") : null;

  // The fast path: every `bun install` after the first lands here.
  let sccachePath = findOnPath("sccache");
  if (sccachePath !== null) {
    const plan = planCargoConfig(existing, sccachePath);
    if (plan.action === "skip" && existsSync(sccacheConfigPath())) return;
  }

  if (sccachePath === null) {
    console.log(`▶ Installing sccache ${SCCACHE_VERSION} (shared Rust compiler cache) into ${cargoBinDir()}`);
    sccachePath = await installSccache();
  }

  if (!existsSync(sccacheConfigPath())) {
    writeAtomically(sccacheConfigPath(), sccacheConfigContent());
  }

  // Only now, with the binary known to exist: a rustc-wrapper pointing at a
  // missing binary would fail every cargo build on the machine.
  const plan = planCargoConfig(existing, sccachePath);
  switch (plan.action) {
    case "write":
      writeAtomically(configPath, plan.content);
      console.log(`✓ Rust builds now use sccache (${configPath})`);
      break;
    case "manual":
      console.warn(`⚠ sccache is installed but not enabled: ${plan.reason}.`);
      console.warn(`  Merge these keys into ${configPath} (into its existing tables, not as duplicates):\n\n${plan.lines}`);
      break;
    case "skip":
      break;
  }
}

if (import.meta.main) {
  try {
    await main();
  } catch (err) {
    console.warn(`⚠ Skipped shared Rust compiler cache setup: ${err instanceof Error ? err.message : String(err)}`);
    console.warn(`  Builds still work, just without the cache. Set ${SKIP_ENV_VAR}=1 to silence this.`);
  }
}
