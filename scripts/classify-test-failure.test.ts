// Covers the abort-vs-failure classifier for the gate's stage reporting
// (scripts/gate-stage.ts, ADR-047): a load-induced process abort (SIGSEGV
// under parallel-test resource contention) and a genuine assertion failure
// currently look identical in the gate's output, wasting time re-running a
// multi-minute suite to tell them apart. These tests exercise the pattern
// matching directly against captured cargo/test-harness output strings.
import { describe, expect, test } from "bun:test";
import { classifyFailure, formatAbortNote } from "./classify-test-failure";

describe("classifyFailure", () => {
  test("classifies real cargo SIGSEGV output as an abort", () => {
    const output = `
error: test failed, to rerun pass \`-p nodespace-core --lib\`
Caused by:
  process didn't exit successfully: \`.../nodespace_core-abc\` (signal: 11, SIGSEGV: invalid memory reference)
`;
    expect(classifyFailure(output)).toBe("abort");
  });

  test("classifies a genuine assertion failure as a failure, not an abort", () => {
    const output = `
test services::node_service::tests::update_task_node_status_only_does_not_change_title ... FAILED

failures:

---- services::node_service::tests::update_task_node_status_only_does_not_change_title stdout ----
thread 'services::node_service::tests::update_task_node_status_only_does_not_change_title' panicked at packages/core/src/services/node_service.rs:42:
assertion \`left == right\` failed
  left: "done"
 right: "open"
`;
    expect(classifyFailure(output)).toBe("failure");
  });

  test("classifies empty output as a failure (silence is the safe default)", () => {
    expect(classifyFailure("")).toBe("failure");
  });

  test("recognizes a bare SIGABRT mention", () => {
    expect(classifyFailure("thread panicked, SIGABRT")).toBe("abort");
  });

  test("recognizes a segmentation fault message case-insensitively", () => {
    // "core dumped" deliberately omitted -- that phrase has its own pattern
    // and would let this test pass even if the segmentation-fault pattern's
    // case-insensitivity broke.
    expect(classifyFailure("Segmentation fault at address 0x0")).toBe("abort");
  });

  test("does not misclassify prose that merely mentions 'signal' without a number", () => {
    expect(classifyFailure("the function returns a signal to the caller")).toBe("failure");
  });

  test("recognizes the bare \"process didn't exit successfully ... (signal\" wording", () => {
    // No "signal:" + digits (so /signal:\s*\d+/i can't be the one carrying
    // this), no SIG* name, no "segmentation fault"/"core dumped" — isolates
    // the dedicated /process didn't exit successfully.*\(signal/i pattern.
    expect(classifyFailure("process didn't exit successfully: `foo` (signal 9)")).toBe("abort");
  });

  test("recognizes STATUS_ACCESS_VIOLATION (the Windows equivalent) case-insensitively", () => {
    expect(classifyFailure("Process exited with status_access_violation")).toBe("abort");
  });
});


describe("formatAbortNote", () => {
  test("names the label and states the suite is known to hit resource contention", () => {
    const note = formatAbortNote("rust:test");
    expect(note).toContain("rust:test");
    expect(note).toContain("Rerun before assuming the code broke");
  });

  test("does not claim the push proceeds anyway (it does not — this is a note, not a bypass)", () => {
    const note = formatAbortNote("rust:test");
    expect(note.toLowerCase()).not.toContain("push blocked");
    expect(note.toLowerCase()).not.toContain("pushing");
  });
});
