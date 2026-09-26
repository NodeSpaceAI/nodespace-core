#!/usr/bin/env bun
// Installs the Rust tooling every dev machine needs, and sets up sccache as
// the machine's shared compiler cache.
//
// - cargo-nextest runs the Rust test suites (`rust:test`). It runs each test
//   in its own process, which is what lets the libsql-linked crates run at
//   full parallelism — see the race described in
//   packages/core/src/db/sqlite_store/mod.rs.
// - sccache sits in front of rustc (and the C/C++ compilers behind libsql and
//   llama.cpp). Every worktree has its own target/, so without it each new
//   worktree compiles every crates.io dependency from scratch; with it, only
//   the first worktree on the machine pays. It does not cache incremental
//   compiles, so the workspace's own crates still build per worktree.
//
// Runs from the root `prepare` script, i.e. on every `bun install`, because
// that is the one step every dev machine is guaranteed to run. That makes
// three rules non-negotiable:
//   - Near-free when already set up: no network, no writes.
//   - Never fails the install. Offline, unsupported platform, no cargo — warn
//     and move on. (A missing nextest does fail `rust:test`, loudly, naming
//     `bun install` as the fix.)
//   - Never clobbers a hand-edited ~/.cargo/config.toml. When a safe append
//     isn't possible, print the lines to add instead.
//
// The cache is local to each machine. A shared remote cache only hits when
// rustc and absolute paths match byte-for-byte across machines, and the
// per-machine win (many worktrees sharing one cache) is most of the benefit.
//
// Opt out with NODESPACE_SKIP_RUST_TOOLING=1.

import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { $ } from "bun";

export const SKIP_ENV_VAR = "NODESPACE_SKIP_RUST_TOOLING";

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

export const CACHE_SIZE_BYTES = 40 * 1024 ** 3;

export type CargoConfigPlan =
  | { action: "write"; content: string }
  | { action: "skip"; reason: string; wrapper: string }
  | { action: "manual"; reason: string; lines: string };

export function cargoConfigBlock(sccachePath: string): string {
  // JSON string escaping is a valid TOML basic string for any path.
  const wrapper = JSON.stringify(sccachePath);
  return [
    "# Machine-wide compiler cache shared by every checkout and worktree —",
    "# written by nodespace-core scripts/setup-rust-tooling.ts.",
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
export function planCargoConfig(
  existing: string | null,
  sccachePath: string,
  legacyConfigExists = false,
): CargoConfigPlan {
  const block = cargoConfigBlock(sccachePath);
  // Cargo reads the extensionless legacy file in preference to config.toml,
  // so anything appended to config.toml would be silently ignored.
  if (legacyConfigExists) {
    return {
      action: "manual",
      reason: "~/.cargo/config (legacy, no extension) takes precedence over config.toml — merge into it, or rename it to config.toml first",
      lines: block,
    };
  }
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
    const wrapper = String(build["rustc-wrapper"]);
    return { action: "skip", reason: `rustc-wrapper already set (${wrapper})`, wrapper };
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

function cargoHome(): string {
  return process.env.CARGO_HOME ?? join(homedir(), ".cargo");
}

function cargoConfigPath(): string {
  return join(cargoHome(), "config.toml");
}

function legacyCargoConfigPath(): string {
  return join(cargoHome(), "config");
}

function cargoBinDir(): string {
  return join(cargoHome(), "bin");
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

/** This platform's pinned release of `tool`, or undefined when there is none. */
export function releaseFor(tool: Tool, platform: string = process.platform, arch: string = process.arch): ToolRelease | undefined {
  return tool.releases[`${platform}-${arch}`];
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
    // Copy-then-rename: another worktree's `bun install` or gate may be
    // running this binary right now, and rewriting a running binary in place
    // on macOS gets it SIGKILLed for an invalid code signature.
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

/** Installs cargo-nextest if it's missing and this platform has a pinned build. */
async function ensureNextest(): Promise<void> {
  if (findOnPath(NEXTEST.name) !== null) return;
  const release = releaseFor(NEXTEST);
  if (release === undefined) return;
  await install(NEXTEST, release, join(cargoBinDir(), NEXTEST.name));
}

async function ensureSccache(): Promise<void> {
  // Someone already routes rustc through a wrapper of their own choosing.
  if (process.env.RUSTC_WRAPPER) return;

  const existingPath = findOnPath(SCCACHE.name);
  const release = releaseFor(SCCACHE);
  // Not installed, and no pinned build for this Mac (Intel): nothing this
  // script can do, and nothing worth a warning on every install.
  if (existingPath === null && release === undefined) return;
  const sccachePath = existingPath ?? join(cargoBinDir(), SCCACHE.name);

  const configPath = cargoConfigPath();
  const existing = existsSync(configPath) ? readFileSync(configPath, "utf8") : null;
  const plan = planCargoConfig(existing, sccachePath, existsSync(legacyCargoConfigPath()));

  // A wrapper is already configured — this script's own earlier run (the
  // fast path every later `bun install` takes) or the user's own choice.
  // Either way nothing to enable, so nothing to download.
  if (plan.action === "skip") {
    const wrapsWithSccache = plan.wrapper === SCCACHE.name || plan.wrapper === existingPath;
    if (wrapsWithSccache && existingPath !== null && !existsSync(sccacheConfigPath())) {
      writeAtomically(sccacheConfigPath(), sccacheConfigContent());
    }
    return;
  }

  // Installed even when the config needs a manual merge, so the printed
  // lines work as-is once merged.
  if (existingPath === null && release !== undefined) {
    await install(SCCACHE, release, sccachePath);
  }

  if (!existsSync(sccacheConfigPath())) {
    writeAtomically(sccacheConfigPath(), sccacheConfigContent());
  }

  // Only now, with the binary known to exist: a rustc-wrapper pointing at a
  // missing binary would fail every cargo build on the machine.
  switch (plan.action) {
    case "write":
      writeAtomically(configPath, plan.content);
      console.log(`✓ Rust builds now use sccache (${configPath})`);
      break;
    case "manual": {
      console.warn(`⚠ sccache is installed but not enabled: ${plan.reason}.`);
      const target = existsSync(legacyCargoConfigPath()) ? legacyCargoConfigPath() : configPath;
      console.warn(`  Merge these keys into ${target} (into its existing tables, not as duplicates):\n\n${plan.lines}`);
      console.warn(`  Or set ${SKIP_ENV_VAR}=1 to stop this notice.`);
      break;
    }
  }
}

async function main(): Promise<void> {
  if (process.env[SKIP_ENV_VAR] === "1") return;
  if (process.platform !== "darwin") return;
  // No Rust toolchain means nothing to set up — a frontend-only machine.
  if (findOnPath("cargo") === null) return;

  // Each independently: one failing (offline, say) mustn't stop the other.
  for (const [name, step] of [
    ["cargo-nextest", ensureNextest],
    ["sccache", ensureSccache],
  ] as const) {
    try {
      await step();
    } catch (err) {
      console.warn(`⚠ Skipped ${name} setup: ${err instanceof Error ? err.message : String(err)}`);
      console.warn(`  Re-run \`bun install\` once it's fixed. Set ${SKIP_ENV_VAR}=1 to silence this.`);
    }
  }
}

if (import.meta.main) {
  await main();
}
