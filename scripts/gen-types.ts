#!/usr/bin/env bun
// `bun run gen:types` — the frontend's wire types, generated from Rust.
//
// `nodespace-types` is the one definition of every wire type (ADR-086 §8). Its
// `gen-ts` binary derives a TypeScript file per type, plus the core type
// registry and the typed core field table, and this script formats them and
// writes them to `packages/desktop-app/src/lib/types/generated/`. The files
// are committed and never edited by hand.
//
//   bun run gen:types           regenerate (writes only what changed)
//   bun run gen:types --check   fail when the committed files differ
//
// Both modes first check that every wire type in the crate's source has a
// generated file, so a type added without its `TS` derive fails here instead
// of being left out.
//
// The merge gate and `bun run test:changed` run `--check`, so a Rust wire-type
// change cannot land without its regenerated TypeScript.

import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = join(import.meta.dir, "..");
const APP = join(REPO, "packages", "desktop-app");
const TYPES_SRC = join(REPO, "packages", "nodespace-types", "src");
export const GENERATED_DIR = join(APP, "src", "lib", "types", "generated");

/** A generated directory's `.ts` files, by name. */
export type GeneratedFiles = Map<string, string>;

export interface Drift {
  file: string;
  /** `missing`: not committed. `stale`: committed but no longer generated. `changed`: contents differ. */
  kind: "missing" | "stale" | "changed";
}

/** How `committed` differs from `generated`, sorted by file. Pure, for testing. */
export function diffGenerated(generated: GeneratedFiles, committed: GeneratedFiles): Drift[] {
  const drift: Drift[] = [];
  for (const [file, contents] of generated) {
    const existing = committed.get(file);
    if (existing === undefined) drift.push({ file, kind: "missing" });
    else if (existing !== contents) drift.push({ file, kind: "changed" });
  }
  for (const file of committed.keys()) {
    if (!generated.has(file)) drift.push({ file, kind: "stale" });
  }
  return drift.sort((a, b) => a.file.localeCompare(b.file));
}

/** The `.ts` files directly in `dir`; an absent directory has none. */
export function readGeneratedDir(dir: string): GeneratedFiles {
  if (!existsSync(dir)) return new Map();
  return new Map(
    readdirSync(dir)
      .filter((name) => name.endsWith(".ts"))
      .sort()
      .map((name) => [name, readFileSync(join(dir, name), "utf8")])
  );
}

/** Makes `dir` hold exactly `generated`, touching only the files in `drift`. */
export function applyGenerated(dir: string, generated: GeneratedFiles, drift: Drift[]): void {
  mkdirSync(dir, { recursive: true });
  for (const { file, kind } of drift) {
    if (kind === "stale") rmSync(join(dir, file));
    else writeFileSync(join(dir, file), generated.get(file) as string);
  }
}

/** The report `--check` prints for a non-empty drift. */
export function describeDrift(drift: Drift[]): string {
  const reasons = { missing: "not committed", stale: "no longer generated", changed: "out of date" };
  return [
    "The generated TypeScript does not match the Rust wire types:",
    ...drift.map(({ file, kind }) => `  ${file}: ${reasons[kind]}`),
    "Run `bun run gen:types` and commit packages/desktop-app/src/lib/types/generated/.",
  ].join("\n");
}

/**
 * The wire types a Rust source file declares: every top-level struct or enum
 * that derives or implements `Serialize` or `Deserialize`. Pure, for testing.
 *
 * Only declarations at column zero count, which is what leaves out the
 * fixtures inside a `#[cfg(test)]` module without having to find its end.
 */
export function wireTypesInSource(source: string): string[] {
  const lines = source.split("\n");
  const handWritten = new Set(
    [...source.matchAll(/^impl(?:<[^>]*>)? (?:serde::)?(?:Serialize|Deserialize(?:<[^>]*>)?) for (\w+)/gm)].map(
      (m) => m[1]
    )
  );
  const found: string[] = [];
  lines.forEach((line, i) => {
    const name = line.match(/^(?:pub(?:\([^)]*\))? )?(?:struct|enum) (\w+)/)?.[1];
    if (name === undefined) return;
    // The attributes, doc comments and comments directly above the
    // declaration; rustfmt wraps a long attribute over indented lines.
    let derives = false;
    for (let j = i - 1; j >= 0 && /^(#\[|\/\/|\)\]|\s+\S)/.test(lines[j]); j--) {
      if (/^#\[derive\(.*\b(Serialize|Deserialize)\b/.test(lines[j])) derives = true;
    }
    if (derives || handWritten.has(name)) found.push(name);
  });
  return found;
}

/** `AiChatNode` to `ai-chat-node.ts`: the generator's file naming. */
export function generatedFileName(typeName: string): string {
  return `${typeName.replace(/(?!^)([A-Z])/g, "-$1").toLowerCase()}.ts`;
}

/** The wire types in `sources` (path to contents) that `generated` has no file for. */
export function missingWireTypes(sources: Map<string, string>, generated: GeneratedFiles): string[] {
  const missing: string[] = [];
  for (const [path, source] of sources) {
    for (const name of wireTypesInSource(source)) {
      if (!generated.has(generatedFileName(name))) missing.push(`${name} (${path})`);
    }
  }
  return missing;
}

/** Every `.rs` file of the crate, by path relative to its `src/`, except the generator itself. */
function readWireTypeSources(): Map<string, string> {
  const paths = [...new Bun.Glob("**/*.rs").scanSync({ cwd: TYPES_SRC })].filter((p) => !p.startsWith("bin/")).sort();
  return new Map(paths.map((path) => [path, readFileSync(join(TYPES_SRC, path), "utf8")]));
}

function run(argv: string[], cwd: string): void {
  const proc = Bun.spawnSync(argv, { cwd, stdout: "inherit", stderr: "inherit" });
  if (proc.exitCode !== 0) throw new Error(`${argv.slice(0, 3).join(" ")} … failed (exit ${proc.exitCode})`);
}

/** Runs the Rust generator and formats its output with the frontend's Prettier config. */
function generate(): GeneratedFiles {
  const out = mkdtempSync(join(tmpdir(), "nodespace-gen-types-"));
  try {
    run(["cargo", "run", "-q", "-p", "nodespace-types", "--features", "ts", "--bin", "gen-ts", "--", out], REPO);
    run(["bunx", "prettier", "--config", ".prettierrc", "--log-level", "warn", "--write", out], APP);
    return readGeneratedDir(out);
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
}

function fail(message: string): never {
  console.error(`\n✗ ${message}\n`);
  process.exit(1);
}

if (import.meta.main) {
  const check = process.argv.includes("--check");
  let generated: GeneratedFiles;
  try {
    generated = generate();
  } catch (err) {
    fail(err instanceof Error ? err.message : String(err));
  }

  const sources = readWireTypeSources();
  // A scan that finds nothing would pass every type unchecked.
  if ([...sources.values()].flatMap(wireTypesInSource).length === 0) {
    fail(`Found no wire type under ${TYPES_SRC}; the source scan is broken.`);
  }
  const missing = missingWireTypes(sources, generated);
  if (missing.length > 0) {
    fail(
      [
        "These nodespace-types wire types have no generated TypeScript:",
        ...missing.map((m) => `  ${m}`),
        'Add `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]` to each and list it in `declarations`',
        "(packages/nodespace-types/src/bin/gen_ts.rs).",
      ].join("\n")
    );
  }

  const drift = diffGenerated(generated, readGeneratedDir(GENERATED_DIR));
  if (drift.length === 0) {
    console.log(`✓ Generated TypeScript is up to date (${generated.size} files).`);
  } else if (check) {
    fail(describeDrift(drift));
  } else {
    applyGenerated(GENERATED_DIR, generated, drift);
    for (const { file, kind } of drift) console.log(`  ${kind === "stale" ? "removed" : "wrote"} ${file}`);
    console.log(`✓ Regenerated ${drift.length} of ${generated.size} files in packages/desktop-app/src/lib/types/generated/.`);
  }
}
