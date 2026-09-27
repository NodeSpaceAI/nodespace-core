// Covers scripts/setup-rust-tooling.ts: the pinned releases it installs into
// the repository's `.tools/`, and how it finds the primary checkout that every
// worktree shares them from.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { NEXTEST, primaryRootFromCommonDir, releaseFor, SCCACHE, sha256Hex } from "./setup-rust-tooling";

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
