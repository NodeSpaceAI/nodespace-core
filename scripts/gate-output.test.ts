// Covers the test gate's output helpers (scripts/gate-output.ts). The gate
// prints only these summaries; full stage output goes to log files, so an
// unread terminal can't fill the pipe and stall a gate that holds the lock.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { commandOutput, stageLogName, tail } from "./gate-output";

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

describe("commandOutput", () => {
  test("decodes stdout and stderr buffers", () => {
    const enc = new TextEncoder();
    expect(commandOutput({ stdout: enc.encode("out"), stderr: enc.encode("err") })).toBe("out\nerr");
  });

  test("is empty for anything that isn't a command result", () => {
    expect(commandOutput(undefined)).toBe("");
    expect(commandOutput("text")).toBe("");
  });
});
