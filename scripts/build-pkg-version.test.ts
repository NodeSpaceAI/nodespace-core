// Regression guard for the .pkg version-derivation logic in scripts/build-pkg.sh.
// That script's `PKG_VERSION` default previously read
// packages/daemon/Cargo.toml — a file `release:bump` (scripts/release.ts) never
// touches — so the .pkg's filename and its own pkgbuild/productbuild version
// metadata silently drifted to a stale hardcoded value while every other release
// artifact tracked the real bumped version.
//
// The fix points the derivation at packages/desktop-app/src-tauri/tauri.conf.json,
// the same canonical source scripts/check-version-sync.ts already enforces and
// `release:bump` already keeps in sync. This test extracts the *actual* derivation
// snippet out of build-pkg.sh (between sentinel comments) and executes it under bash
// against fixture trees, so a future edit that reintroduces a stale/hardcoded/wrong-file
// read fails this test under `bun test scripts/` (part of `test:all`, enforced by the
// merge gate) — without needing Apple certs or a real signed .pkg build.
import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const BUILD_PKG_SH = join(REPO, "scripts", "build-pkg.sh");

const BEGIN_MARKER = "# --- BEGIN pkg-version-derivation";
const END_MARKER = "# --- END pkg-version-derivation";

function extractDerivationSnippet(): string {
  const src = readFileSync(BUILD_PKG_SH, "utf8");
  const begin = src.indexOf(BEGIN_MARKER);
  const end = src.indexOf(END_MARKER);
  if (begin === -1 || end === -1 || end < begin) {
    throw new Error(
      `could not find ${BEGIN_MARKER} / ${END_MARKER} sentinels in ${BUILD_PKG_SH} — ` +
        "did the version-derivation block move or get renamed?",
    );
  }
  return src.slice(begin, end);
}

interface RunResult {
  pkgVersion: string;
  pkgName: string;
}

function runDerivation(env: Record<string, string | undefined>): RunResult {
  const snippet = extractDerivationSnippet();
  const script = [
    "set -euo pipefail",
    snippet,
    'echo "TEST_PKG_VERSION=${PKG_VERSION}"',
    'echo "TEST_PKG_NAME=${PKG_NAME}"',
  ].join("\n");

  const result = Bun.spawnSync(["bash", "-c", script], {
    env: env as Record<string, string>,
    stdout: "pipe",
    stderr: "pipe",
  });

  if (result.exitCode !== 0) {
    throw new Error(
      `derivation snippet exited ${result.exitCode}\nstdout: ${result.stdout}\nstderr: ${result.stderr}`,
    );
  }

  const stdout = result.stdout.toString();
  const pkgVersion = /TEST_PKG_VERSION=(.*)/.exec(stdout)?.[1];
  const pkgName = /TEST_PKG_NAME=(.*)/.exec(stdout)?.[1];
  if (!pkgVersion || !pkgName) {
    throw new Error(`could not parse derivation output:\n${stdout}`);
  }
  return { pkgVersion, pkgName };
}

/** Builds a throwaway repo-shaped tree with just the files the snippet reads. */
function makeFixtureRepo(opts: { tauriVersion: string; daemonCargoVersion?: string }): string {
  const root = mkdtempSync(join(tmpdir(), "build-pkg-version-test-"));
  const tauriDir = join(root, "packages", "desktop-app", "src-tauri");
  mkdirSync(tauriDir, { recursive: true });
  writeFileSync(
    join(tauriDir, "tauri.conf.json"),
    JSON.stringify({ productName: "NodeSpace", version: opts.tauriVersion }),
  );

  if (opts.daemonCargoVersion) {
    const daemonDir = join(root, "packages", "daemon");
    mkdirSync(daemonDir, { recursive: true });
    writeFileSync(
      join(daemonDir, "Cargo.toml"),
      `[package]\nname = "nodespace-daemon"\nversion = "${opts.daemonCargoVersion}"\n`,
    );
  }

  return root;
}

describe("build-pkg.sh version derivation", () => {
  test("derives PKG_VERSION from the real repo's canonical tauri.conf.json", () => {
    const tauriConfig = JSON.parse(
      readFileSync(
        join(REPO, "packages", "desktop-app", "src-tauri", "tauri.conf.json"),
        "utf8",
      ),
    );
    const { pkgVersion } = runDerivation({
      PATH: process.env.PATH,
      REPO_ROOT: REPO,
      TRIPLE: "aarch64-apple-darwin",
    });
    expect(pkgVersion).toBe(tauriConfig.version);
  });

  test("derives PKG_VERSION and PKG_NAME from a mocked tauri.conf.json input", () => {
    const fixture = makeFixtureRepo({ tauriVersion: "9.9.9-mocked" });
    try {
      const { pkgVersion, pkgName } = runDerivation({
        PATH: process.env.PATH,
        REPO_ROOT: fixture,
        TRIPLE: "test-triple",
      });
      expect(pkgVersion).toBe("9.9.9-mocked");
      expect(pkgName).toBe("NodeSpace_9.9.9-mocked_test-triple.pkg");
    } finally {
      rmSync(fixture, { recursive: true, force: true });
    }
  });

  test("regression: ignores packages/daemon/Cargo.toml even when it disagrees with tauri.conf.json", () => {
    // This is the exact shape of the original bug: packages/daemon/Cargo.toml stuck at
    // "0.2.0" while tauri.conf.json (and every other release:bump target) had moved on.
    const fixture = makeFixtureRepo({
      tauriVersion: "3.4.5",
      daemonCargoVersion: "0.2.0",
    });
    try {
      const { pkgVersion } = runDerivation({
        PATH: process.env.PATH,
        REPO_ROOT: fixture,
        TRIPLE: "aarch64-apple-darwin",
      });
      expect(pkgVersion).toBe("3.4.5");
      expect(pkgVersion).not.toBe("0.2.0");
    } finally {
      rmSync(fixture, { recursive: true, force: true });
    }
  });

  test("respects an explicit PKG_VERSION override without reading tauri.conf.json", () => {
    const { pkgVersion, pkgName } = runDerivation({
      PATH: process.env.PATH,
      REPO_ROOT: "/nonexistent/repo/root",
      TRIPLE: "aarch64-apple-darwin",
      PKG_VERSION: "1.2.3-override",
    });
    expect(pkgVersion).toBe("1.2.3-override");
    expect(pkgName).toBe("NodeSpace_1.2.3-override_aarch64-apple-darwin.pkg");
  });
});
