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
import { sccacheServerUds } from "./gate-sccache";

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
});
