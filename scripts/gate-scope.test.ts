// Covers the pre-push gate's change scoping (scripts/gate-scope.ts). The
// property that matters is one-directional: a skipped stage must be one no
// changed file can reach. Every uncertain case has to fall back to the full
// pyramid, so most of these tests pin that fallback.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { classify, describeScope } from "./gate-scope";

const NONE = { frontend: false, rust: false, skill: false, scripts: false };

describe("classify", () => {
  test("a Svelte component change runs only the frontend tiers", () => {
    expect(classify(["packages/desktop-app/src/lib/design/components/base-node.svelte"])).toEqual({
      fullReason: null,
      ...NONE,
      frontend: true,
    });
  });

  test("the dev-proxy counts as frontend", () => {
    expect(classify(["packages/dev-tools/src/proxy.ts"]).frontend).toBe(true);
  });

  test("a Rust change runs Rust and the (seconds-long) scripts tests, not the frontend tiers", () => {
    const scope = classify(["packages/core/src/services/node_service/mod.rs"]);
    expect(scope).toEqual({ fullReason: null, ...NONE, rust: true, scripts: true });
  });

  test.each([
    "Cargo.lock",
    "packages/desktop-app/src-tauri/tauri.conf.json",
    "packages/core/tests/fixtures/data.json",
    "packages/proto/proto/nodespace.proto",
    ".cargo/config.toml",
  ])("%s counts as a Rust input", (file) => {
    expect(classify([file]).rust).toBe(true);
  });

  test("a skill change runs the skill tier and Rust, whose tests read the skill's files", () => {
    expect(classify(["packages/skill/SKILL.md"])).toEqual({
      fullReason: null,
      ...NONE,
      skill: true,
      rust: true,
      scripts: true,
    });
  });

  test("the eval golden fixtures under scripts/ also run Rust, whose tests assert them", () => {
    expect(classify(["scripts/eval/golden/case-1.json"]).rust).toBe(true);
  });

  test("a tooling script change runs only the scripts tests", () => {
    expect(classify(["scripts/gh-utils.ts"])).toEqual({ fullReason: null, ...NONE, scripts: true });
  });

  test("prose and agent config run no test tiers", () => {
    expect(classify(["CLAUDE.md", "README.md", ".claude/skills/run-app/SKILL.md"])).toEqual({
      fullReason: null,
      ...NONE,
    });
  });

  test("mixed changes union their stages", () => {
    const scope = classify(["packages/desktop-app/src/routes/+page.svelte", "packages/cli/src/main.rs"]);
    expect(scope).toEqual({ fullReason: null, ...NONE, frontend: true, rust: true, scripts: true });
  });

  test.each([
    "scripts/test-gate.ts",
    "scripts/gate-scope.ts",
    "scripts/gate-lock.ts",
    "package.json",
    "bun.lock",
    ".husky/pre-push",
  ])("a change to the gate's own machinery (%s) runs everything", (file) => {
    const scope = classify([file]);
    expect(scope.fullReason).not.toBeNull();
    expect(scope.frontend && scope.rust && scope.skill && scope.scripts).toBe(true);
  });

  test("an unrecognized path runs everything rather than guessing", () => {
    const scope = classify(["packages/brand-new-package/index.ts"]);
    expect(scope.fullReason).toContain("unrecognized path");
    expect(scope.rust && scope.frontend).toBe(true);
  });

  test("an empty diff runs everything rather than skipping it all", () => {
    expect(classify([]).fullReason).not.toBeNull();
  });
});

test("describeScope names what runs, what is skipped, and the override", () => {
  const text = describeScope(classify(["packages/desktop-app/src/app.css"]));
  expect(text).toContain("running: frontend");
  expect(text).toContain("skipping: rust, skill, scripts");
  expect(text).toContain("NODESPACE_GATE_FULL=1");
});
