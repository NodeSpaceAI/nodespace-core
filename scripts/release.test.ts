// Covers how scripts/release.ts reports a failed pre-release `test:perf` run:
// the releaser is told which benchmarks failed, not just that the run did.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { failingBenchmarks } from "./release";

describe("failingBenchmarks", () => {
  test("names each test from vitest's Failed Tests summary", () => {
    const output = [
      "   ✓ Suite > passing benchmark 12ms",
      "   × Suite > bulk ops complete in <100ms 212ms",
      "⎯⎯⎯⎯⎯⎯⎯ Failed Tests 2 ⎯⎯⎯⎯⎯⎯⎯",
      "",
      " FAIL  src/tests/performance/a.test.ts > Suite > bulk ops complete in <100ms",
      "AssertionError: expected 212 to be less than 100",
      " FAIL  src/tests/performance/b.test.ts > Other > render scales linearly",
      " Test Files  2 failed (2)"
    ].join("\n");

    expect(failingBenchmarks(output)).toEqual([
      "src/tests/performance/a.test.ts > Suite > bulk ops complete in <100ms",
      "src/tests/performance/b.test.ts > Other > render scales linearly"
    ]);
  });

  test("returns nothing when the output names no failing test", () => {
    // e.g. the run died before collecting tests; the caller still prints the tail.
    expect(failingBenchmarks("error: script not found\n")).toEqual([]);
  });
});
