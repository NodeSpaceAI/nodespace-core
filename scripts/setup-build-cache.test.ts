// Covers the decisions scripts/setup-build-cache.ts makes about a machine's
// ~/.cargo/config.toml. The rule that matters most is "never break cargo":
// a duplicated [build]/[env] table or a wrapper pointing at nothing fails
// every build on the machine, so those cases must hand off to the user.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import {
  cargoConfigBlock,
  CACHE_SIZE_BYTES,
  planCargoConfig,
  SCCACHE_RELEASES,
  sccacheConfigContent,
  sha256Hex,
} from "./setup-build-cache";

const SCCACHE = "/Users/dev/.cargo/bin/sccache";

describe("planCargoConfig", () => {
  test("writes the block when the file does not exist", () => {
    const plan = planCargoConfig(null, SCCACHE);
    expect(plan).toEqual({ action: "write", content: cargoConfigBlock(SCCACHE) });
  });

  test("writes the block when the file is empty", () => {
    expect(planCargoConfig("  \n", SCCACHE).action).toBe("write");
  });

  test("the written block is valid TOML that routes rustc and CMake through sccache", () => {
    const plan = planCargoConfig(null, SCCACHE);
    if (plan.action !== "write") throw new Error("expected write");
    const parsed = Bun.TOML.parse(plan.content) as {
      build: Record<string, string>;
      env: Record<string, string>;
    };
    expect(parsed.build["rustc-wrapper"]).toBe(SCCACHE);
    expect(parsed.env.CMAKE_C_COMPILER_LAUNCHER).toBe(SCCACHE);
    expect(parsed.env.CMAKE_CXX_COMPILER_LAUNCHER).toBe(SCCACHE);
  });

  test("appends to a file with unrelated tables, keeping its content", () => {
    const existing = '[net]\ngit-fetch-with-cli = true';
    const plan = planCargoConfig(existing, SCCACHE);
    if (plan.action !== "write") throw new Error("expected write");
    expect(plan.content.startsWith(existing)).toBe(true);
    const parsed = Bun.TOML.parse(plan.content) as Record<string, Record<string, unknown>>;
    expect(parsed.net["git-fetch-with-cli"]).toBe(true);
    expect(parsed.build["rustc-wrapper"]).toBe(SCCACHE);
  });

  test("skips when a rustc-wrapper is already configured", () => {
    const plan = planCargoConfig('[build]\nrustc-wrapper = "sccache"\n', SCCACHE);
    expect(plan.action).toBe("skip");
  });

  test("skips when a different wrapper is configured, leaving the user's choice alone", () => {
    const plan = planCargoConfig('[build]\nrustc-wrapper = "/opt/cachepot"\n', SCCACHE);
    expect(plan).toEqual({ action: "skip", reason: "rustc-wrapper already set (/opt/cachepot)" });
  });

  test("hands off when [build] exists without a wrapper — appending would duplicate the table", () => {
    const plan = planCargoConfig("[build]\njobs = 8\n", SCCACHE);
    expect(plan.action).toBe("manual");
  });

  test("hands off when [env] exists — appending would duplicate the table", () => {
    const plan = planCargoConfig('[env]\nFOO = "bar"\n', SCCACHE);
    expect(plan.action).toBe("manual");
  });

  test("hands off when a legacy ~/.cargo/config exists — cargo would ignore config.toml", () => {
    const plan = planCargoConfig(null, SCCACHE, true);
    expect(plan.action).toBe("manual");
  });

  test("hands off when the file is not valid TOML", () => {
    const plan = planCargoConfig("[build\nnot toml", SCCACHE);
    expect(plan.action).toBe("manual");
  });
});

describe("sccache release pin", () => {
  test("pins a full SHA-256 for Apple Silicon", () => {
    expect(SCCACHE_RELEASES["darwin-arm64"].sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  test("sha256Hex matches a known digest", () => {
    expect(sha256Hex(new TextEncoder().encode("abc"))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });
});

test("sccache config sets a 40 GiB disk cache", () => {
  const parsed = Bun.TOML.parse(sccacheConfigContent()) as { cache: { disk: { size: number } } };
  expect(parsed.cache.disk.size).toBe(CACHE_SIZE_BYTES);
});
