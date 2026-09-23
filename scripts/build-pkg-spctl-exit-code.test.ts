// Regression guard for build-pkg.sh's Gatekeeper-verification step. Under
// `set -euo pipefail`, a bare `spctl ... && echo OK` treats ANY non-zero
// `spctl` exit as a genuine Gatekeeper rejection -- but spctl only documents
// exit 3 as "assessment denied"; every other non-zero exit (a transient
// network/OCSP hiccup, a bad invocation, spctl itself crashing) is a check
// failure, not a rejection verdict, and must not be misreported as one.
// scripts/verify-pkg-gatekeeper.ts's assessGatekeeperInstall already draws
// this distinction for the separate post-release verification tool; this
// guards the release-time check inside build-pkg.sh itself.
//
// This test extracts the *actual* spctl-handling snippet out of build-pkg.sh
// (between sentinel comments), matching the technique
// scripts/build-pkg-macos-chmod.test.ts already uses for the chmod block,
// and executes it under bash with a fake `spctl` on PATH that exits with a
// controlled code -- so a future edit that reintroduces the "any non-zero =
// rejected" bug regresses this test under `bun test scripts/` (part of
// `test:all`, enforced by the pre-push gate).
import { describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const BUILD_PKG_SH = join(REPO, "scripts", "build-pkg.sh");

const BEGIN_MARKER = "# --- BEGIN spctl-exit-code";
const END_MARKER = "# --- END spctl-exit-code ---";

function extractSpctlSnippet(): string {
  const src = readFileSync(BUILD_PKG_SH, "utf8");
  const begin = src.indexOf(BEGIN_MARKER);
  const end = src.indexOf(END_MARKER);
  if (begin === -1 || end === -1 || end < begin) {
    throw new Error(
      `could not find ${BEGIN_MARKER} / ${END_MARKER} sentinels in ${BUILD_PKG_SH} — ` +
        "did the spctl exit-code handling move or get renamed?",
    );
  }
  return src.slice(begin, end);
}

interface RunResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
}

/** Writes a fake `spctl` onto a throwaway PATH dir that exits with `exitCode` and prints `marker`. */
function makeFakeSpctl(binDir: string, exitCode: number, marker: string): void {
  const spctlPath = join(binDir, "spctl");
  writeFileSync(
    spctlPath,
    `#!/bin/bash\necho "${marker} stdout"\necho "${marker} stderr" >&2\nexit ${exitCode}\n`,
  );
  chmodSync(spctlPath, 0o755);
}

function runSnippet(finalPkg: string, binDir: string): RunResult {
  const snippet = extractSpctlSnippet();
  const script = ["set -euo pipefail", snippet].join("\n");

  const result = Bun.spawnSync(["bash", "-c", script], {
    env: {
      PATH: `${binDir}:${process.env.PATH ?? ""}`,
      FINAL_PKG: finalPkg,
    } as Record<string, string>,
    stdout: "pipe",
    stderr: "pipe",
  });

  return {
    exitCode: result.exitCode,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
}

describe("build-pkg.sh spctl exit-code handling", () => {
  test("exit 0 is treated as Gatekeeper acceptance", () => {
    const binDir = mkdtempSync(join(tmpdir(), "build-pkg-spctl-test-"));
    try {
      makeFakeSpctl(binDir, 0, "ACCEPTED");
      const result = runSnippet("/tmp/NodeSpace_1.0.0.pkg", binDir);

      expect(result.exitCode).toBe(0);
      expect(result.stdout).toContain("Gatekeeper: OK");
    } finally {
      rmSync(binDir, { recursive: true, force: true });
    }
  });

  test("exit 3 is reported as a genuine Gatekeeper rejection", () => {
    const binDir = mkdtempSync(join(tmpdir(), "build-pkg-spctl-test-"));
    try {
      makeFakeSpctl(binDir, 3, "REJECTED");
      const result = runSnippet("/tmp/NodeSpace_1.0.0.pkg", binDir);

      expect(result.exitCode).not.toBe(0);
      expect(result.stderr).toContain("Gatekeeper rejected");
      expect(result.stderr).toContain("REJECTED stdout");
      expect(result.stderr).toContain("REJECTED stderr");
      // Must not be conflated with the generic "not a Gatekeeper verdict" path.
      expect(result.stderr).not.toContain("not a Gatekeeper verdict");
    } finally {
      rmSync(binDir, { recursive: true, force: true });
    }
  });

  test("a non-3 non-zero exit is reported as a check error, not a rejection", () => {
    const binDir = mkdtempSync(join(tmpdir(), "build-pkg-spctl-test-"));
    try {
      makeFakeSpctl(binDir, 2, "TOOLERROR");
      const result = runSnippet("/tmp/NodeSpace_1.0.0.pkg", binDir);

      expect(result.exitCode).not.toBe(0);
      expect(result.stderr).toContain("not a Gatekeeper verdict");
      expect(result.stderr).toContain("spctl exited 2");
      expect(result.stderr).toContain("TOOLERROR stdout");
      // The bug this guards against: misreporting a tool error as a rejection.
      expect(result.stderr).not.toContain("Gatekeeper rejected");
    } finally {
      rmSync(binDir, { recursive: true, force: true });
    }
  });

  test("a high exit code (e.g. 127, command not found) is also a check error, not a rejection", () => {
    const binDir = mkdtempSync(join(tmpdir(), "build-pkg-spctl-test-"));
    try {
      makeFakeSpctl(binDir, 127, "NOTFOUND");
      const result = runSnippet("/tmp/NodeSpace_1.0.0.pkg", binDir);

      expect(result.exitCode).not.toBe(0);
      expect(result.stderr).toContain("not a Gatekeeper verdict");
      expect(result.stderr).toContain("spctl exited 127");
      expect(result.stderr).not.toContain("Gatekeeper rejected");
    } finally {
      rmSync(binDir, { recursive: true, force: true });
    }
  });
});
