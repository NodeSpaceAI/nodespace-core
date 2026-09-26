#!/usr/bin/env bun
// Output handling for the test gate (scripts/test-gate.ts): each stage's full
// output goes to a log file, and the gate prints a line per stage plus the
// log's tail on failure. See test-gate.ts for why the gate must never stream
// test output into the terminal of the session that pushed.

import { constants } from "node:os";

const osSignals = constants.signals;

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

/**
 * A closing line for a failed stage's output naming how it ended. A stage
 * killed by a signal (a crash, or the OOM killer) otherwise ends its log
 * mid-line with no cause. `sh` reports a child it lost to signal N as exit
 * code 128+N, so that is named as the signal too — which is what lets the
 * abort classifier (classify-test-failure.ts) recognise it.
 */
export function exitStatusLine(exitCode: number | null, signalCode: string | null): string {
  if (signalCode) return `[stage killed by ${signalCode}]`;
  if (exitCode !== null && exitCode > 128) {
    const name = Object.entries(osSignals).find(([, n]) => n === exitCode - 128)?.[0];
    if (name) return `[stage exited with code ${exitCode}: killed by ${name}]`;
  }
  return `[stage exited with code ${exitCode ?? "unknown"}]`;
}
