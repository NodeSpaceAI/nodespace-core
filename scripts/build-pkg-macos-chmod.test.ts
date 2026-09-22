// Regression guard for build-pkg.sh's Contents/MacOS/* chmod line — the
// build-time defense-in-depth (alongside the release workflow's tar/untar
// hardening) for a lost Unix executable bit on the .app bundle's own
// binaries across the upload-artifact/download-artifact handoff between CI
// jobs. No dedicated test previously existed for the chmod itself, and the
// glob it runs against had no nullglob guard — not reachable today given the
// current externalBin config (always exactly 3 external binaries plus the
// app's own executable), but would have failed with a confusing raw
// BSD-glob "No such file or directory" instead of a clear diagnostic if
// Contents/MacOS were ever empty.
//
// This test extracts the *actual* chmod snippet out of build-pkg.sh (between
// sentinel comments), matching the technique
// scripts/build-pkg-version.test.ts already uses for the version-derivation
// block, and executes it under bash against fixture trees — so a future edit
// that drops the chmod, the nullglob guard, or the empty-check regresses
// this test under `bun test scripts/` (part of `test:all`, enforced by the
// pre-push gate).
import { describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const BUILD_PKG_SH = join(REPO, "scripts", "build-pkg.sh");

const BEGIN_MARKER = "# --- BEGIN macos-chmod";
const END_MARKER = "# --- END macos-chmod ---";

function extractChmodSnippet(): string {
  const src = readFileSync(BUILD_PKG_SH, "utf8");
  const begin = src.indexOf(BEGIN_MARKER);
  const end = src.indexOf(END_MARKER);
  if (begin === -1 || end === -1 || end < begin) {
    throw new Error(
      `could not find ${BEGIN_MARKER} / ${END_MARKER} sentinels in ${BUILD_PKG_SH} — ` +
        "did the macOS chmod block move or get renamed?",
    );
  }
  return src.slice(begin, end);
}

interface RunResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
}

function runSnippet(payloadRoot: string): RunResult {
  const snippet = extractChmodSnippet();
  const script = ["set -euo pipefail", snippet].join("\n");

  const result = Bun.spawnSync(["bash", "-c", script], {
    env: { PATH: process.env.PATH ?? "", PAYLOAD_ROOT: payloadRoot } as Record<string, string>,
    stdout: "pipe",
    stderr: "pipe",
  });

  return {
    exitCode: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

/** Builds a throwaway `<root>/Applications/NodeSpace.app/Contents/MacOS` tree. */
function makeMacOSDir(root: string): string {
  const macosDir = join(root, "Applications", "NodeSpace.app", "Contents", "MacOS");
  mkdirSync(macosDir, { recursive: true });
  return macosDir;
}

describe("build-pkg.sh Contents/MacOS/* chmod", () => {
  test("chmods every file in Contents/MacOS to 755", () => {
    const root = mkdtempSync(join(tmpdir(), "build-pkg-chmod-test-"));
    try {
      const macosDir = makeMacOSDir(root);
      const bin1 = join(macosDir, "NodeSpace");
      const bin2 = join(macosDir, "nodespace-skill-installer");
      writeFileSync(bin1, "#!/bin/sh\necho hi\n");
      writeFileSync(bin2, "#!/bin/sh\necho hi\n");
      chmodSync(bin1, 0o644);
      chmodSync(bin2, 0o644);

      const result = runSnippet(root);

      expect(result.exitCode).toBe(0);
      expect(statSync(bin1).mode & 0o777).toBe(0o755);
      expect(statSync(bin2).mode & 0o777).toBe(0o755);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("fails with a clear diagnostic instead of a raw glob error when Contents/MacOS is empty", () => {
    const root = mkdtempSync(join(tmpdir(), "build-pkg-chmod-test-"));
    try {
      makeMacOSDir(root); // created, but left empty
      const result = runSnippet(root);

      expect(result.exitCode).not.toBe(0);
      expect(result.stderr).toContain("Contents/MacOS has no files to chmod");
      // The bug this guards against: an unmatched glob passed literally to
      // chmod, which BSD chmod reports as "No such file or directory" for
      // the literal asterisk-containing path — not a message this repo
      // wrote, and not diagnostic of the real problem.
      expect(result.stderr).not.toContain("No such file or directory");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("fails with the same clear diagnostic when Contents/MacOS does not exist at all", () => {
    const root = mkdtempSync(join(tmpdir(), "build-pkg-chmod-test-"));
    try {
      // Applications/NodeSpace.app/Contents/MacOS is never created.
      const result = runSnippet(root);

      expect(result.exitCode).not.toBe(0);
      expect(result.stderr).toContain("Contents/MacOS has no files to chmod");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
