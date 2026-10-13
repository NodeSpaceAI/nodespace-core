import { describe, expect, test } from "bun:test";
import { records } from "./pi.ts";

/** A byte stream that delivers `chunks` as they are given, however they cut the text. */
async function* chunked(...chunks: string[]): AsyncGenerator<Uint8Array> {
  const encoder = new TextEncoder();

  for (const chunk of chunks) {
    yield encoder.encode(chunk);
  }
}

async function all(stream: AsyncIterable<Uint8Array>): Promise<string[]> {
  const out: string[] = [];

  for await (const line of records(stream)) {
    out.push(line);
  }

  return out;
}

describe("Pi's JSONL framing", () => {
  test("splits on LF only, across chunks, and strips a CR", async () => {
    expect(await all(chunked('{"a":1}\r\n{"b"', ':2}\n{"c":3}\n'))).toEqual(['{"a":1}', '{"b":2}', '{"c":3}']);
  });

  test("keeps a Unicode line or paragraph separator inside its record", async () => {
    const record = JSON.stringify({ text: "one two three" });

    expect(await all(chunked(`${record}\n`))).toEqual([record]);
  });

  test("keeps a multi-byte character cut between chunks", async () => {
    const bytes = new TextEncoder().encode('{"t":"é"}\n');
    const cut = bytes.indexOf(0xc3) + 1;
    const stream = (async function* () {
      yield bytes.slice(0, cut);
      yield bytes.slice(cut);
    })();

    expect(await all(stream)).toEqual(['{"t":"é"}']);
  });

  test("yields a last record that has no newline", async () => {
    expect(await all(chunked('{"a":1}\n{"b":2}'))).toEqual(['{"a":1}', '{"b":2}']);
  });
});
