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
import { describe, expect, test } from "bun:test";
import { devCargoConfig, devRustcWrapper, GENERATED_MARKER, SCCACHE_CACHE_SIZE, sccacheServerUds } from "./gate-sccache";

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

describe("the development builds' wrapper", () => {
  // The wrapper computes its socket in shell; if it ever drifted from
  // sccacheServerUds, development builds would start a second server on the
  // same cache instead of sharing the gate's.
  test.each([
    ["with a trailing slash", "/private/var/folders/ab/T/"],
    ["without one", "/private/var/folders/ab/T"],
    ["unset", undefined],
  ])("reaches the gate's server when TMPDIR is %s", (_label, tmp) => {
    const script = devRustcWrapper("/repo/.tools").replace(/^exec .*$/m, 'echo "$SCCACHE_SERVER_UDS"');
    const env: Record<string, string> = { PATH: process.env.PATH ?? "" };
    if (tmp !== undefined) env.TMPDIR = tmp;
    const shell = Bun.spawnSync(["sh", "-c", script], { env }).stdout.toString().trim();

    const previous = process.env.TMPDIR;
    try {
      if (tmp === undefined) delete process.env.TMPDIR;
      else process.env.TMPDIR = tmp;
      expect(shell).toBe(sccacheServerUds(process.getuid?.() ?? 0));
    } finally {
      if (previous === undefined) delete process.env.TMPDIR;
      else process.env.TMPDIR = previous;
    }
  });

  test("hands sccache the gate's cache and size, and passes every argument through", () => {
    const script = devRustcWrapper("/repo/.tools");
    expect(script).toContain('SCCACHE_DIR="/repo/.tools/sccache-cache"');
    expect(script).toContain(`SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE}"`);
    expect(script).toMatch(/^exec "\/repo\/.tools\/bin\/sccache" "\$@"$/m);
  });

  test("the cargo config routes rustc and CMake through the wrapper, and is marked as generated", () => {
    const config = devCargoConfig("/repo/.tools/bin/rustc-wrapper");
    expect(config).toContain(GENERATED_MARKER);
    expect(config).toContain('rustc-wrapper = "/repo/.tools/bin/rustc-wrapper"');
    expect(config).toContain('CMAKE_CXX_COMPILER_LAUNCHER = "/repo/.tools/bin/rustc-wrapper"');
  });
});
