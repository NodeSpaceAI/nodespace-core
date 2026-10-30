import { describe, expect, test } from "bun:test";
import { renderDump } from "./prompt-dump";

// One file, written by one writer with one counter: a local turn, a remote
// turn, then a local turn again — the shape the daemon produces when the
// provider is swapped mid-conversation.
const MIXED = [
  { kind: "prompt", engine: "native", seq: 0, target: null, sent: "LOCAL PROMPT ONE", sent_len: 16 },
  { kind: "response", engine: "native", seq: 0, raw_response: "local answer one", raw_response_len: 16 },
  {
    kind: "prompt",
    engine: "openai_compat",
    seq: 1,
    target: "http://host/v1",
    sent: {
      model: "remote-model",
      messages: [
        { role: "system", content: "be brief" },
        { role: "user", content: "REMOTE QUESTION" },
      ],
      tools: [{ function: { name: "search_nodes" } }],
    },
    sent_len: 120,
  },
  { kind: "response", engine: "openai_compat", seq: 1, raw_response: "remote answer", raw_response_len: 13 },
  { kind: "prompt", engine: "native", seq: 2, target: null, sent: "LOCAL PROMPT TWO", sent_len: 16 },
  { kind: "response", engine: "native", seq: 2, raw_response: "local answer two", raw_response_len: 16 },
].map((r) => JSON.stringify(r));

const render = (...flags: string[]) => renderDump(MIXED, new Set(flags)).join("\n");

describe("prompt-dump reader", () => {
  test("shows local and remote turns of one file in timeline order", () => {
    const text = render();
    const at = (needle: string) => text.indexOf(needle);

    expect(at("PROMPT #0 [native]")).toBeGreaterThanOrEqual(0);
    expect(at("PROMPT #1 [openai_compat]")).toBeGreaterThan(at("PROMPT #0 [native]"));
    expect(at("PROMPT #2 [native]")).toBeGreaterThan(at("PROMPT #1 [openai_compat]"));
    expect(at("RESPONSE #1 [openai_compat]")).toBeGreaterThan(at("PROMPT #1 [openai_compat]"));
    expect(at("RESPONSE #1 [openai_compat]")).toBeLessThan(at("PROMPT #2 [native]"));
  });

  test("shows what each engine put on the wire", () => {
    const text = render();
    expect(text).toContain("LOCAL PROMPT ONE");
    expect(text).toContain("→ http://host/v1");
    expect(text).toContain("model: remote-model");
    expect(text).toContain("REMOTE QUESTION");
  });

  test("--prompt prints the sent payload verbatim and skips responses", () => {
    const text = render("--prompt");
    expect(text).toContain("LOCAL PROMPT TWO");
    expect(text).toContain('"content": "REMOTE QUESTION"');
    expect(text).not.toContain("RESPONSE");
  });

  test("--tools lists what a remote request offered and notes the local one renders them in", () => {
    const text = render("--tools");
    expect(text).toContain("• search_nodes");
    expect(text).toContain("rendered into the prompt text");
  });

  test("--last keeps only the most recent call and its response", () => {
    const text = render("--last");
    expect(text).toContain("LOCAL PROMPT TWO");
    expect(text).toContain("local answer two");
    expect(text).not.toContain("LOCAL PROMPT ONE");
    expect(text).not.toContain("REMOTE QUESTION");
  });

  test("--raw returns the lines untouched", () => {
    expect(renderDump(MIXED, new Set(["--raw"]))).toEqual(MIXED);
  });

  test("--last ignores calls from an earlier run whose seq restarted in the same file", () => {
    const run = (n: number, tag: string) =>
      Array.from({ length: n }, (_, seq) => [
        { kind: "prompt", engine: "native", seq, target: null, sent: `${tag} PROMPT ${seq}`, sent_len: 1 },
        { kind: "response", engine: "native", seq, raw_response: `${tag} answer ${seq}`, raw_response_len: 1 },
      ]).flat();
    const appended = [...run(3, "OLD"), ...run(2, "NEW")].map((r) => JSON.stringify(r));
    const text = renderDump(appended, new Set(["--last"])).join("\n");

    expect(text).toContain("NEW PROMPT 1");
    expect(text).toContain("NEW answer 1");
    expect(text).not.toContain("OLD");
    expect(text.match(/PROMPT #1/g)).toHaveLength(1);
  });
});
