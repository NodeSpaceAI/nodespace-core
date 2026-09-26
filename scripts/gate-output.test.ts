// Covers the test gate's output helpers (scripts/gate-output.ts). The gate
// prints only these summaries; full stage output goes to log files, so an
// unread terminal can't fill the pipe and stall a gate that holds the lock.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { freeGiBFromDf, stageLogName, tail } from "./gate-output";

describe("freeGiBFromDf", () => {
  test("reads the Available column of macOS df -k output", () => {
    const df =
      "Filesystem 1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n" +
      "/dev/disk3s1 482797652 409052356  38797296    92% 2700000 4000000   1%   /System/Volumes/Data\n";
    expect(freeGiBFromDf(df)).toBeCloseTo(37.0, 1);
  });

  test("is null for output it can't read, so the gate doesn't refuse on a parse failure", () => {
    expect(freeGiBFromDf("")).toBeNull();
    expect(freeGiBFromDf("garbage")).toBeNull();
  });
});

describe("stageLogName", () => {
  test("turns a stage label into a safe file name", () => {
    expect(stageLogName("test:browser (Chromium)")).toBe("test-browser-chromium.log");
    expect(stageLogName("Tauri-seam integration tests (ADR-048)")).toBe("tauri-seam-integration-tests-adr-048.log");
  });

  test("bounds the length", () => {
    expect(stageLogName("x".repeat(500)).length).toBeLessThanOrEqual(84);
  });
});

describe("tail", () => {
  test("keeps the last N non-empty lines", () => {
    expect(tail("a\n\nb\nc\n\nd\n", 2)).toBe("c\nd");
  });

  test("returns everything when there are fewer lines", () => {
    expect(tail("only\n", 40)).toBe("only");
  });
});
