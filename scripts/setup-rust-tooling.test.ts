// Covers scripts/setup-rust-tooling.ts: the pinned releases it installs into
// the repository's `.tools/`, and how it finds the primary checkout that every
// worktree shares them from.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { GENERATED_MARKER } from "./gate-sccache";
import { needsInstall, NEXTEST, primaryRootFromCommonDir, releaseFor, SCCACHE, setUpDevCache, sha256Hex } from "./setup-rust-tooling";

describe("release pins", () => {
  test("every pinned release has a full SHA-256 and a versioned URL", () => {
    for (const tool of [SCCACHE, NEXTEST]) {
      for (const release of Object.values(tool.releases)) {
        expect(release.sha256).toMatch(/^[0-9a-f]{64}$/);
        expect(release.url).toContain(tool.version);
      }
    }
  });

  test("nextest covers both Mac architectures; sccache only Apple Silicon", () => {
    expect(releaseFor(NEXTEST, "darwin", "arm64")).toBeDefined();
    expect(releaseFor(NEXTEST, "darwin", "x64")).toBeDefined();
    expect(releaseFor(SCCACHE, "darwin", "arm64")).toBeDefined();
    expect(releaseFor(SCCACHE, "darwin", "x64")).toBeUndefined();
    expect(releaseFor(NEXTEST, "linux", "x64")).toBeUndefined();
  });

  test("sha256Hex matches a known digest", () => {
    expect(sha256Hex(new TextEncoder().encode("abc"))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });
});

describe("primaryRootFromCommonDir", () => {
  test("a worktree's common dir is the primary checkout's .git", () => {
    expect(primaryRootFromCommonDir("/Users/dev/nodespace-core/.git")).toBe("/Users/dev/nodespace-core");
  });

  test("a trailing slash doesn't change the answer", () => {
    expect(primaryRootFromCommonDir("/Users/dev/nodespace-core/.git/")).toBe("/Users/dev/nodespace-core");
  });
});

describe("needsInstall", () => {
  test("installs a missing tool", () => {
    expect(needsInstall(false, null, "0.9.146")).toBe(true);
  });

  test("reinstalls when the pinned version moved past the stamped one", () => {
    expect(needsInstall(true, "0.9.145", "0.9.146")).toBe(true);
  });

  test("reinstalls a binary with no stamp (installed before stamps existed)", () => {
    expect(needsInstall(true, null, "0.9.146")).toBe(true);
  });

  test("leaves an up-to-date tool alone, trailing newline or not", () => {
    expect(needsInstall(true, "0.9.146", "0.9.146")).toBe(false);
    expect(needsInstall(true, "0.9.146\n", "0.9.146")).toBe(false);
  });
});

describe("setUpDevCache", () => {
  let checkout: string;
  beforeEach(() => {
    checkout = mkdtempSync(join(tmpdir(), "setup-dev-cache-test-"));
  });
  afterEach(() => {
    rmSync(checkout, { recursive: true, force: true });
  });

  test("writes an executable wrapper into the checkout and a config naming it", () => {
    expect(setUpDevCache(checkout, "/repo/.tools")).toBe(true);
    const wrapper = join(checkout, ".cargo", "rustc-wrapper");
    expect(statSync(wrapper).mode & 0o111).not.toBe(0);
    expect(readFileSync(join(checkout, ".cargo", "config.toml"), "utf8")).toContain(`rustc-wrapper = "${wrapper}"`);
  });

  test("rewrites its own config on the next install", () => {
    setUpDevCache(checkout, "/repo/.tools");
    setUpDevCache(checkout, "/elsewhere/.tools");
    expect(readFileSync(join(checkout, ".cargo", "rustc-wrapper"), "utf8")).toContain("/elsewhere/.tools/bin/sccache");
  });

  test("leaves a hand-written config alone and writes no wrapper", () => {
    mkdirSync(join(checkout, ".cargo"));
    const handWritten = '[build]\nrustc-wrapper = "my-own"\n';
    writeFileSync(join(checkout, ".cargo", "config.toml"), handWritten);
    expect(setUpDevCache(checkout, "/repo/.tools")).toBe(false);
    expect(readFileSync(join(checkout, ".cargo", "config.toml"), "utf8")).toBe(handWritten);
    expect(readFileSync(join(checkout, ".cargo", "config.toml"), "utf8")).not.toContain(GENERATED_MARKER);
    expect(existsSync(join(checkout, ".cargo", "rustc-wrapper"))).toBe(false);
  });
});
