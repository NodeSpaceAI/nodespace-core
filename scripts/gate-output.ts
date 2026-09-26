#!/usr/bin/env bun
// Output handling for the test gate (scripts/test-gate.ts): each stage's full
// output goes to a log file, and the gate prints a line per stage plus the
// log's tail on failure. See test-gate.ts for why the gate must never stream
// test output into the terminal of the session that pushed.

/** A stage label as a log file name. */
export function stageLogName(label: string): string {
  const slug = label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 80);
  return `${slug}.log`;
}

/** The last `lines` non-empty lines of `text`. */
export function tail(text: string, lines: number): string {
  return text
    .split("\n")
    .filter((l) => l.trim() !== "")
    .slice(-lines)
    .join("\n");
}

/**
 * Free space in GiB from `df -k <path>` output (the "Available" column of
 * the first data row), or null when it can't be read.
 */
export function freeGiBFromDf(dfOutput: string): number | null {
  const row = dfOutput.trim().split("\n")[1];
  const availableKiB = Number(row?.trim().split(/\s+/)[3]);
  return Number.isFinite(availableKiB) ? availableKiB / 1024 ** 2 : null;
}
