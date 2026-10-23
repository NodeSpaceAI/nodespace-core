// The .pkg installs one LaunchAgent plist machine-wide, shared by every user of
// the Mac. launchd does not expand "~" or $HOME in StandardOutPath /
// StandardErrorPath, so a path there makes the job fail to start (EX_CONFIG)
// and the daemon never runs. The plist therefore starts the daemon through a
// small shell that resolves the log per user at run time.
//
// These tests read the shipped plist (macOS `plutil`), assert it holds no
// unexpanded path, and run its actual shell command against a stub daemon.
import { describe, expect, setDefaultTimeout, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

setDefaultTimeout(30_000);

const PLIST = join(dirname(new URL(import.meta.url).pathname), "pkg-resources", "app.nodespace.daemon.plist");
const DAEMON = "/usr/local/bin/nodespaced";

const onMac = process.platform === "darwin";
const macTest = onMac ? test : test.skip;

type Plist = {
  Label: string;
  ProgramArguments: string[];
  [key: string]: unknown;
};

function readPlist(): Plist {
  const out = Bun.spawnSync(["plutil", "-convert", "json", "-o", "-", PLIST], { stdout: "pipe", stderr: "pipe" });
  if (out.exitCode !== 0) throw new Error(`plutil failed: ${out.stderr.toString()}`);
  return JSON.parse(out.stdout.toString()) as Plist;
}

function stringValues(value: unknown): string[] {
  if (typeof value === "string") return [value];
  if (Array.isArray(value)) return value.flatMap(stringValues);
  if (value && typeof value === "object") return Object.values(value).flatMap(stringValues);
  return [];
}

describe("pkg LaunchAgent plist", () => {
  macTest("is a valid property list", () => {
    const lint = Bun.spawnSync(["plutil", "-lint", PLIST], { stdout: "pipe", stderr: "pipe" });
    expect(lint.exitCode).toBe(0);
  });

  macTest("keeps the label the app registers under", () => {
    expect(readPlist().Label).toBe("app.nodespace.daemon");
  });

  macTest("holds no value launchd would take literally that starts with a tilde", () => {
    const plist = readPlist();
    for (const value of stringValues(plist)) {
      expect(value.startsWith("~")).toBe(false);
    }
    for (const key of ["StandardOutPath", "StandardErrorPath"]) {
      const value = plist[key];
      // launchd expands nothing in these, so the plist must not set them at all
      // (the shell redirects instead).
      expect(value).toBeUndefined();
    }
  });

  macTest("runs the daemon through a shell that resolves the log under the user's home", () => {
    const [shell, flag, script] = readPlist().ProgramArguments;
    expect(shell).toBe("/bin/sh");
    expect(flag).toBe("-c");
    expect(script).toContain('"$HOME/.nodespace/logs"');
    expect(script).toContain('"$HOME/.nodespace/logs/nodespaced.log"');
    expect(script).toContain(`exec ${DAEMON} --tray`);
  });
});

describe("pkg LaunchAgent shell command", () => {
  function runCommand(home: string, stubDir: string): { exitCode: number | null; stub: string } {
    const script = readPlist().ProgramArguments[2];
    expect(script).toContain(DAEMON);
    const stub = join(stubDir, "nodespaced");
    writeFileSync(
      stub,
      `#!/bin/sh
echo "stdout line"
echo "stderr line" >&2
echo "argv: $*" >> "${stubDir}/argv"
echo "pid: $$" >> "${stubDir}/argv"
`,
    );
    chmodSync(stub, 0o755);
    const result = Bun.spawnSync(["/bin/sh", "-c", script.replace(DAEMON, stub)], {
      env: { PATH: "/usr/bin:/bin", HOME: home },
      stdout: "pipe",
      stderr: "pipe",
    });
    // Anything the daemon writes must go to the log file, not to launchd.
    expect(result.stdout.toString()).toBe("");
    expect(result.stderr.toString()).toBe("");
    return { exitCode: result.exitCode, stub };
  }

  for (const [name, sub] of [
    ["a plain home", "home"],
    ["a home containing a space", "Mary Ann"],
  ] as const) {
    macTest(`creates the log directory and sends stdout and stderr to the log: ${name}`, () => {
      const root = mkdtempSync(join(tmpdir(), "pkg-agent-"));
      try {
        const home = join(root, sub);
        mkdirSync(home);
        const stubDir = join(root, "stub");
        mkdirSync(stubDir);
        const { exitCode } = runCommand(home, stubDir);
        expect(exitCode).toBe(0);
        const log = join(home, ".nodespace", "logs", "nodespaced.log");
        expect(existsSync(log)).toBe(true);
        const text = readFileSync(log, "utf8");
        expect(text).toContain("stdout line");
        expect(text).toContain("stderr line");
        // The daemon was started with the tray flag.
        expect(readFileSync(join(stubDir, "argv"), "utf8")).toContain("argv: --tray");
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    });
  }

  macTest("appends to an existing log and runs the daemon in place of the shell", () => {
    const root = mkdtempSync(join(tmpdir(), "pkg-agent-"));
    try {
      const home = join(root, "home");
      mkdirSync(join(home, ".nodespace", "logs"), { recursive: true });
      const log = join(home, ".nodespace", "logs", "nodespaced.log");
      writeFileSync(log, "earlier run\n");
      const stubDir = join(root, "stub");
      mkdirSync(stubDir);
      // The stub reports its own pid and its parent's: after exec the shell is
      // gone, so the daemon's parent is this test process, not a shell.
      const script = readPlist().ProgramArguments[2];
      const stub = join(stubDir, "nodespaced");
      writeFileSync(stub, `#!/bin/sh\necho "pid $$ ppid $PPID"\n`);
      chmodSync(stub, 0o755);
      const proc = Bun.spawnSync(["/bin/sh", "-c", script.replace(DAEMON, stub)], {
        env: { PATH: "/usr/bin:/bin", HOME: home },
      });
      expect(proc.exitCode).toBe(0);
      const text = readFileSync(log, "utf8");
      expect(text.startsWith("earlier run\n")).toBe(true);
      const match = /pid (\d+) ppid (\d+)/.exec(text);
      expect(match).not.toBeNull();
      expect(Number(match![2])).toBe(process.pid);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
