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
// `bun test scripts/` (part of `test:all`, enforced by the merge gate)
// — without needing a real macOS install or root privileges.
import { describe, expect, setDefaultTimeout, test } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

// These tests write a fake executable and spawn it through bash: correctness
// checks, not performance ones. The 5s default measured how busy the machine
// was — they timed out at a load average of ~43 while passing in about a
// second each on a quiet one.
setDefaultTimeout(30_000);

const REPO = join(dirname(new URL(import.meta.url).pathname), "..");
const POSTINSTALL = join(REPO, "scripts", "pkg-resources", "postinstall");

function extractBlock(name: string): string {
  const beginMarker = `# --- BEGIN ${name}`;
  const endMarker = `# --- END ${name} ---`;
  const src = readFileSync(POSTINSTALL, "utf8");
  const begin = src.indexOf(beginMarker);
  const end = src.indexOf(endMarker);
  if (begin === -1 || end === -1 || end < begin) {
    throw new Error(
      `could not find ${beginMarker} / ${endMarker} sentinels in ${POSTINSTALL} — ` +
        `did the ${name} block move or get renamed?`,
    );
  }
  return src.slice(begin, end);
}

function extractBootstrapSnippet(): string {
  return extractBlock("launchd-bootstrap");
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
  const stubs: Record<string, string> = {
    launchctl: script,
    // Console owner, uid/gid and home directory come from env vars so each
    // test picks the "logged-in user" without touching real directory services.
    stat: `#!/bin/bash
printf '%s\\n' "\${MOCK_CONSOLE_USER-}"
`,
    id: `#!/bin/bash
[[ "$1" == "-u" ]] && { printf '%s\\n' "\${MOCK_UID-501}"; exit 0; }
[[ "$1" == "-g" ]] && { printf '%s\\n' "\${MOCK_GID-20}"; exit 0; }
exit 1
`,
    dscl: `#!/bin/bash
if [[ -n "\${MOCK_HOME_ON_NEXT_LINE-}" ]]; then
  printf 'NFSHomeDirectory:\\n %s\\n' "\${MOCK_HOME-}"
else
  printf 'NFSHomeDirectory: %s\\n' "\${MOCK_HOME-}"
fi
`,
    // Records the argv it was given (one per line) instead of running anything.
    sudo: `#!/bin/bash
printf '%s\\n' "$@" > "\${MOCK_SUDO_LOG}"
`,
  };
  for (const [name, body] of Object.entries(stubs)) {
    const file = join(dir, name);
    writeFileSync(file, body);
    chmodSync(file, 0o755);
  }
  return dir;
}

interface RunResult {
  exitCode: number | null;
  stderr: string;
}

function runSnippet(env: Record<string, string | undefined>): RunResult {
  const snippet = extractBootstrapSnippet();
  const script = [
    "set -euo pipefail",
    extractBlock("console-user"),
    extractBlock("log-dir"),
    "resolve_installing_user",
    "create_user_log_dir",
    snippet,
    "exit 0",
  ].join("\n");

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
    // $USER is deliberately hostile: it must never decide who gets the install.
    USER: "root",
    MOCK_CONSOLE_USER: "alice",
    MOCK_UID: "501",
    MOCK_GID: "20",
    MOCK_HOME: mockDir,
    MOCK_SUDO_LOG: join(mockDir, "sudo.log"),
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

  test("skips bootstrap entirely when the console owner is root (login window)", () => {
    const mockDir = makeMockBin();
    const counterFile = join(mockDir, "count");
    try {
      const { exitCode } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_CONSOLE_USER: "root",
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

describe("postinstall installing-user resolution and log directory", () => {
  function withMock(fn: (mockDir: string, counterFile: string) => void) {
    const mockDir = makeMockBin();
    try {
      fn(mockDir, join(mockDir, "count"));
    } finally {
      rmSync(mockDir, { recursive: true, force: true });
    }
  }

  test("USER=root with a real console user still bootstraps for the console user", () => {
    withMock((mockDir, counterFile) => {
      const { exitCode } = runSnippet({ ...baseEnv(mockDir, counterFile), USER: "root" });
      expect(exitCode).toBe(0);
      expect(readFileSync(counterFile, "utf8").trim()).toBe("1");
    });
  });

  test("USER unset does not matter", () => {
    withMock((mockDir, counterFile) => {
      const env: Record<string, string | undefined> = baseEnv(mockDir, counterFile);
      delete env.USER;
      const { exitCode } = runSnippet(env);
      expect(exitCode).toBe(0);
      expect(readFileSync(counterFile, "utf8").trim()).toBe("1");
    });
  });

  test("the log directory is created as the user (privileges dropped), not as root", () => {
    withMock((mockDir, counterFile) => {
      const { exitCode } = runSnippet(baseEnv(mockDir, counterFile));
      expect(exitCode).toBe(0);
      const argv = readFileSync(join(mockDir, "sudo.log"), "utf8").trimEnd().split("\n");
      expect(argv).toEqual([
        "-n",
        "-u",
        "#501",
        "mkdir",
        "-p",
        "--",
        `${mockDir}/Library/Logs/com.nodespace`,
      ]);
    });
  });

  test("a home directory containing a space (dscl wraps it onto the next line) is kept whole", () => {
    withMock((mockDir, counterFile) => {
      const home = join(mockDir, "Mary Ann");
      mkdirSync(home);
      const { exitCode } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_CONSOLE_USER: "Mary Ann",
        MOCK_HOME: home,
        MOCK_HOME_ON_NEXT_LINE: "1",
      });
      expect(exitCode).toBe(0);
      const argv = readFileSync(join(mockDir, "sudo.log"), "utf8").trimEnd().split("\n");
      expect(argv[argv.length - 1]).toBe(`${home}/Library/Logs/com.nodespace`);
    });
  });

  test("a user name made of shell metacharacters is never evaluated", () => {
    withMock((mockDir, counterFile) => {
      const pwned = join(mockDir, "pwned");
      const { exitCode } = runSnippet({
        ...baseEnv(mockDir, counterFile),
        MOCK_CONSOLE_USER: `x; touch ${pwned}; $(touch ${pwned}) \`touch ${pwned}\``,
      });
      expect(exitCode).toBe(0);
      expect(existsSync(pwned)).toBe(false);
    });
  });

  test("a name starting with a dash, a system uid, or a missing home is refused", () => {
    for (const override of [
      { MOCK_CONSOLE_USER: "-n" },
      { MOCK_UID: "89" },
      { MOCK_UID: "abc" },
      { MOCK_HOME: "relative/path" },
    ]) {
      withMock((mockDir, counterFile) => {
        const { exitCode } = runSnippet({ ...baseEnv(mockDir, counterFile), ...override });
        expect(exitCode).toBe(0);
        expect(existsSync(counterFile)).toBe(false);
        expect(existsSync(join(mockDir, "sudo.log"))).toBe(false);
      });
    }
  });

  test("the script contains no eval and does not read $USER or logname", () => {
    const code = readFileSync(POSTINSTALL, "utf8")
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("#"))
      .join("\n");
    expect(code).not.toMatch(/\beval\b/);
    expect(code).not.toMatch(/\$\{?USER\b/);
    expect(code).not.toMatch(/\blogname\b/);
  });
});
