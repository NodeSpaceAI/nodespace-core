// Regression guard for the launchd-bootstrap logic in
// scripts/pkg-resources/postinstall.
//
// Before the fix, `launchctl bootstrap` (unlike the `bootout` call right
// above it) ran unguarded under `set -euo pipefail`. `bootstrap` returns
// error 5 (EIO) both when the daemon label is already registered and when a
// preceding `bootout` is still mid-teardown (a race) — neither is a real
// failure, but the unguarded call let either one propagate through `set -e`
// and fail the ENTIRE package install, even though the binaries were
// already correctly installed and signed by that point.
//
// This test extracts the *actual* launchd-bootstrap snippet out of
// postinstall (between sentinel comments) and executes it under bash
// against a mocked `launchctl`, so a future edit that reintroduces an
// unguarded/non-resilient bootstrap call fails this test under
// `bun test scripts/` (part of `test:all`, enforced by the pre-push gate)
// — without needing a real macOS install or root privileges.
import { describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const POSTINSTALL = join(REPO, "scripts", "pkg-resources", "postinstall");

const BEGIN_MARKER = "# --- BEGIN launchd-bootstrap";
const END_MARKER = "# --- END launchd-bootstrap ---";

function extractBootstrapSnippet(): string {
  const src = readFileSync(POSTINSTALL, "utf8");
  const begin = src.indexOf(BEGIN_MARKER);
  const end = src.indexOf(END_MARKER);
  if (begin === -1 || end === -1 || end < begin) {
    throw new Error(
      `could not find ${BEGIN_MARKER} / ${END_MARKER} sentinels in ${POSTINSTALL} — ` +
        "did the launchd-bootstrap block move or get renamed?",
    );
  }
  return src.slice(begin, end);
}

/**
 * Writes a mock `launchctl` onto a throwaway PATH dir so the snippet never
 * touches the real launchd. Behavior is controlled entirely by env vars the
 * test sets, so no code needs to change per scenario:
 *   - MOCK_BOOTSTRAP_FAIL_COUNT: how many leading `bootstrap` invocations
 *     return EIO (5) before one succeeds. A huge number means "always fails".
 *   - MOCK_PRINT_EXIT: exit code for `launchctl print` (0 = service loaded).
 * Call counts persist across invocations via a counter file, since each
 * `launchctl` call is a fresh process.
 */
function makeMockBin(): string {
  const dir = mkdtempSync(join(tmpdir(), "postinstall-bootstrap-test-"));
  const script = `#!/bin/bash
case "$1" in
  bootout)
    exit "\${MOCK_BOOTOUT_EXIT:-0}"
    ;;
  bootstrap)
    count=0
    [[ -f "\${MOCK_CALL_COUNTER}" ]] && count=$(cat "\${MOCK_CALL_COUNTER}")
    count=$((count + 1))
    echo "$count" > "\${MOCK_CALL_COUNTER}"
    if [[ "$count" -le "\${MOCK_BOOTSTRAP_FAIL_COUNT:-0}" ]]; then
      exit 5
    fi
    exit 0
    ;;
  print)
    exit "\${MOCK_PRINT_EXIT:-0}"
    ;;
  *)
    exit 0
    ;;
esac
`;
  const bin = join(dir, "launchctl");
  writeFileSync(bin, script);
  chmodSync(bin, 0o755);
  return dir;
}

interface RunResult {
  exitCode: number | null;
  stderr: string;
}

function runSnippet(env: Record<string, string | undefined>): RunResult {
  const snippet = extractBootstrapSnippet();
  const script = ["set -euo pipefail", snippet, "exit 0"].join("\n");

  const result = Bun.spawnSync(["bash", "-c", script], {
    env: env as Record<string, string>,
    stdout: "pipe",
    stderr: "pipe",
  });

  return { exitCode: result.exitCode, stderr: result.stderr.toString() };
}

function baseEnv(mockDir: string, counterFile: string): Record<string, string> {
  return {
    PATH: `${mockDir}:${process.env.PATH}`,
    USER: process.env.USER ?? "testuser",
    LAUNCHD_PLIST: "/Library/LaunchAgents/app.nodespace.daemon.plist",
    MOCK_CALL_COUNTER: counterFile,
  };
}

describe("postinstall launchd-bootstrap resilience", () => {
  test("bootstrap succeeding immediately exits 0 with no warning", () => {
    const mockDir = makeMockBin();
    const counterFile = join(mockDir, "count");
    try {
      const { exitCode, stderr } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_BOOTSTRAP_FAIL_COUNT: "0",
      });
      expect(exitCode).toBe(0);
      expect(stderr).not.toContain("Warning");
      expect(stderr).not.toContain("Note:");
    } finally {
      rmSync(mockDir, { recursive: true, force: true });
    }
  });

  test("bootstrap failing once (mid-teardown race) then succeeding on retry exits 0 with no warning", () => {
    const mockDir = makeMockBin();
    const counterFile = join(mockDir, "count");
    try {
      const { exitCode, stderr } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_BOOTSTRAP_FAIL_COUNT: "1",
      });
      expect(exitCode).toBe(0);
      expect(stderr).not.toContain("Warning");
      expect(stderr).not.toContain("Note:");
      expect(readFileSync(counterFile, "utf8").trim()).toBe("2");
    } finally {
      rmSync(mockDir, { recursive: true, force: true });
    }
  });

  test("bootstrap failing every time, but service ends up loaded (already registered) — reported as a note, install not aborted", () => {
    const mockDir = makeMockBin();
    const counterFile = join(mockDir, "count");
    try {
      const { exitCode, stderr } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_BOOTSTRAP_FAIL_COUNT: "99",
        MOCK_PRINT_EXIT: "0",
      });
      expect(exitCode).toBe(0);
      expect(stderr).toContain("already registered");
      expect(stderr).not.toContain("Warning");
    } finally {
      rmSync(mockDir, { recursive: true, force: true });
    }
  });

  test("bootstrap failing every time and service NOT loaded — reported as a warning, install still not aborted", () => {
    const mockDir = makeMockBin();
    const counterFile = join(mockDir, "count");
    try {
      const { exitCode, stderr } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_BOOTSTRAP_FAIL_COUNT: "99",
        MOCK_PRINT_EXIT: "1",
      });
      // The whole point of the fix: a genuine registration failure is
      // reported to stderr but must NOT propagate as a package-install
      // failure — the binaries are already installed by this point.
      expect(exitCode).toBe(0);
      expect(stderr).toContain("Warning: failed to register");
      expect(stderr).toContain("launchctl bootstrap gui/");
    } finally {
      rmSync(mockDir, { recursive: true, force: true });
    }
  });

  test("skips bootstrap entirely for root (matches original guard)", () => {
    const mockDir = makeMockBin();
    const counterFile = join(mockDir, "count");
    try {
      const { exitCode } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        USER: "root",
        MOCK_BOOTSTRAP_FAIL_COUNT: "0",
      });
      expect(exitCode).toBe(0);
      // bootstrap/bootout never ran, so the counter file was never created.
      expect(() => readFileSync(counterFile, "utf8")).toThrow();
    } finally {
      rmSync(mockDir, { recursive: true, force: true });
    }
  });
});
