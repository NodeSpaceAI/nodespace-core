// Covers the gate's per-user sccache addressing (scripts/gate-sccache.ts).
// This dev Mac (and others like it) is shared by more than one macOS
// account, each with its own clone and `.tools/`. sccache runs one server
// per address, so a single shared address let whichever account's gate
// started the server first silently own it — every other account's gate
// then compiled through a server process running as the wrong OS user,
// which can't write that account's `target/` or temp files, and compiles
// failed with permission errors. sccacheServerUds keys the server's unix
// socket path on the numeric uid so every account gets its own server.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { devCargoConfig, devRustcWrapper, GENERATED_MARKER, sccacheServerUds } from "./gate-sccache";

describe("sccacheServerUds", () => {
  test("two different uids get different socket paths", () => {
    expect(sccacheServerUds(501)).not.toBe(sccacheServerUds(502));
  });

  test("the same uid gets a stable path across calls", () => {
    expect(sccacheServerUds(502)).toBe(sccacheServerUds(502));
  });

  test("the path is scoped to the given uid", () => {
    expect(sccacheServerUds(502)).toContain("502");
    expect(sccacheServerUds(501)).toContain("501");
  });

  // uid 0 (root) is also the `process.getuid?.() ?? 0` fallback used at the
  // call site when `getuid` is unavailable (e.g. Windows). Documented here so
  // that collision is visible, even though the fallback is unreachable today
  // (the pinned sccache release is darwin-arm64 only).
  test("uid 0 gets its own distinct, stable path", () => {
    expect(sccacheServerUds(0)).not.toBe(sccacheServerUds(502));
    expect(sccacheServerUds(0)).toBe(sccacheServerUds(0));
  });
});

// Each of these writes a shell script and runs it. On a machine in the middle
// of a cold build, starting a script that was written a moment ago has stalled:
// four of them timed out together at Bun's default 5 seconds in a merge gate,
// about 20 seconds in all. The limit here is sized to catch a hang.
const SPAWN_TIMEOUT_MS = 60_000;

describe("the development builds' wrapper", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "gate-sccache-test-"));
  });
  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  /** Runs a generated wrapper with `args`, under exactly `env`. */
  function runWrapper(script: string, args: string[], env: Record<string, string> = {}): string {
    const path = join(dir, "rustc-wrapper");
    writeFileSync(path, script, { mode: 0o755 });
    const run = Bun.spawnSync([path, ...args], { env: { PATH: process.env.PATH ?? "", ...env } });
    // Says how the script ended. Killed at a test's time limit it printed
    // nothing, and an empty string alone reads as wrong output.
    if (run.exitCode !== 0) {
      throw new Error(
        `the wrapper exited ${run.exitCode} (signal ${run.signalCode ?? "none"}): ${run.stderr.toString().trim()}`
      );
    }
    return run.stdout.toString().trim();
  }

  /** A stand-in sccache that reports the settings and arguments it was given. */
  function fakeTools(): string {
    const tools = join(dir, "tools");
    mkdirSync(join(tools, "bin"), { recursive: true });
    writeFileSync(
      join(tools, "bin", "sccache"),
      '#!/bin/sh\necho "$SCCACHE_DIR|$SCCACHE_SERVER_UDS|$SCCACHE_IGNORE_SERVER_IO_ERROR|$*"\n',
      { mode: 0o755 }
    );
    return tools;
  }

  test("hands sccache the gate's cache, a server of its own and every argument", () => {
    const tools = fakeTools();
    const [cache, socket, ignoreIoErrors, args] = runWrapper(devRustcWrapper(tools, "/repo/wt-a"), ["rustc", "--crate-name", "x"], {
      TMPDIR: "/private/var/folders/ab/T/",
    }).split("|");
    expect(cache).toBe(join(tools, "sccache-cache"));
    // Beside the gate's own socket, in the private per-user temp directory.
    expect(dirname(socket)).toBe(dirname(join("/private/var/folders/ab/T", basename(sccacheServerUds(0)))));
    expect(ignoreIoErrors).toBe("1");
    expect(args).toBe("rustc --crate-name x");
  }, SPAWN_TIMEOUT_MS);

  test("each checkout gets its own server, and keeps it", () => {
    const tools = fakeTools();
    const socketOf = (checkout: string) => runWrapper(devRustcWrapper(tools, checkout), ["rustc"]).split("|")[1];
    expect(socketOf("/repo/wt-a")).not.toBe(socketOf("/repo/wt-b"));
    expect(socketOf("/repo/wt-a")).toBe(socketOf("/repo/wt-a"));
  }, SPAWN_TIMEOUT_MS);

  test("with a TMPDIR too deep for a unix socket, it compiles uncached rather than failing the build", () => {
    const tools = fakeTools();
    const deep = `/private/var/folders/${"x".repeat(90)}/T/`;
    expect(runWrapper(devRustcWrapper(tools, "/repo/wt-a"), ["echo", "compiled", "directly"], { TMPDIR: deep })).toBe(
      "compiled directly"
    );
  }, SPAWN_TIMEOUT_MS);

  test("with sccache gone, it runs the compiler itself rather than failing the build", () => {
    const script = devRustcWrapper(join(dir, "no-such-tools"), "/repo/wt-a");
    expect(runWrapper(script, ["echo", "compiled", "directly"])).toBe("compiled directly");
  }, SPAWN_TIMEOUT_MS);

  test("the cargo config routes rustc and CMake through the wrapper, and is marked as generated", () => {
    const config = devCargoConfig("/repo/wt-a/.cargo/rustc-wrapper");
    expect(config).toContain(GENERATED_MARKER);
    expect(config).toContain('rustc-wrapper = "/repo/wt-a/.cargo/rustc-wrapper"');
    expect(config).toContain('CMAKE_CXX_COMPILER_LAUNCHER = "/repo/wt-a/.cargo/rustc-wrapper"');
  });
});
