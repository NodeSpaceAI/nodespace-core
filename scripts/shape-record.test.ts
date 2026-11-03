// Covers how the built-in shape record check (ADR-104) is wired into the gates.
// The check itself is Rust: packages/core/src/shape_record/ and its example.

import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { TIERS } from "./gate-stage";

const REPO = join(import.meta.dir, "..");
const read = (path: string) => readFileSync(join(REPO, path), "utf8");

describe("the shape record check in the gates", () => {
  test("the stage runs the check, not a regeneration", () => {
    expect(TIERS.shapeCheck.command).toBe("bun run shape:check");
    const scripts = JSON.parse(read("package.json")).scripts;
    expect(scripts["shape:check"]).toBe("cargo run -q -p nodespace-core --example shape_record -- check");
    expect(scripts["shape:gen"]).toBe("cargo run -q -p nodespace-core --example shape_record -- generate");
  });

  test("the merge gate and test:changed both run the stage", () => {
    expect(read("scripts/test-gate.ts")).toContain("run(TIERS.shapeCheck)");
    expect(read("scripts/test-changed.ts")).toContain("run({ ...TIERS.shapeCheck,");
  });

  test("the files the check reads are committed", () => {
    for (const file of ["current.json", "change-log.json", "snapshots/baseline.json"]) {
      expect(existsSync(join(REPO, "packages/core/shape-record", file))).toBe(true);
    }
  });
});
