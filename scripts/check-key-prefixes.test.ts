// Covers the key prefix check (scripts/check-key-prefixes.ts, ADR-112). Pattern
// tests run lines that must and must not be flagged; the real-repo block at the
// bottom is the enforcement path (`bun run test:scripts`).
//
// DOM-free on purpose: this file runs under `bun test scripts/`.
import { describe, expect, test } from "bun:test";
import { failureMessage, findHits, kindOf, listScannedFiles, scan } from "./check-key-prefixes";

const hitsIn = (file: string, text: string) => findHits(file, text).map((hit) => hit.pattern);

describe("code", () => {
  const banned: [string, string][] = [
    ["a.rs", `key.strip_prefix("custom:")`],
    ["a.rs", `const P: [&str; 3] = ["custom:", "org:", "plugin:"];`],
    ["a.rs", `json!({ "custom:effort": 3 })`],
    ["a.rs", `format!("custom:{key}")`],
    ["a.ts", "const k = `custom:${name}`;"],
    ["a.ts", `if (key.startsWith('plugin:')) {`],
    ["a.svelte", `const key = "org:region";`],
    ["a.rs", `  *p = "custom:a";`],
    ["a.rs", `/* x */ k.starts_with("custom:")`],
    ["a.rs", `let k = "plugin:nodespace:x";`],
    ["a.ts", `const k = "custom:story-points";`],
    ["a.ts", `const k = "org:Cost.Center2";`],
    ["a.json", `{ "key": "custom:effort" }`],
    ["a.toml", `key = "plugin:owner_person_id"`],
    ["a.sql", `SELECT properties->'collection'->>'plugin:owner_person_id'`],
  ];
  for (const [file, line] of banned) {
    test(`flags: ${line}`, () => {
      expect(hitsIn(file, line).length).toBe(1);
    });
  }

  const allowed: [string, string][] = [
    ["a.rs", `invoke(&app, "plugin:fixture|fixture_ping")`],
    ["a.ts", `const x = atom({ plugin: 'nodespace', key: 'session' })`],
    ["a.rs", `let n = 1; // the "custom:effort" key is gone`],
    ["a.rs", `/* "custom:effort" */ let n = 1;`],
    ["a.rs", `let url = "https://example.com//custom:x"; let n = 2;`],
    ["a.rs", `let node_id = id.strip_prefix("node:");`],
    ["a.rs", `json!({ "effort": 3 })`],
  ];
  for (const [file, line] of allowed) {
    test(`allows: ${line}`, () => {
      expect(hitsIn(file, line)).toEqual([]);
    });
  }
});

describe("guidance", () => {
  for (const line of [
    "Use a `custom:` prefix on a core type.",
    "the custom/org/plugin: prefix rule",
    "store it as custom:notes",
    "a custom: prefix goes before the name",
    "the org: namespace is the company's",
  ]) {
    test(`flags: ${line}`, () => {
      expect(hitsIn("packages/agent/src/seeds/skills/x.md", line).length).toBe(1);
    });
  }
  test("allows prose that names no prefix", () => {
    expect(hitsIn("AGENTS.md", "A field goes in the schema's custom_fields.")).toEqual([]);
  });
});

describe("scope", () => {
  test("scans code and agent guidance, and not this check", () => {
    expect(kindOf("packages/core/src/a.rs")).toBe("code");
    expect(kindOf("scripts/a.ts")).toBe("code");
    expect(kindOf("packages/agent/src/seeds/rules/agent/a.md")).toBe("markdown");
    expect(kindOf("AGENTS.md")).toBe("markdown");
    expect(kindOf("README.md")).toBeNull();
    expect(kindOf("packages/agent/goldens/ablation/x.toml")).toBeNull();
    expect(kindOf("packages/a/b.sql")).toBe("code");
    expect(kindOf("scripts/check-key-prefixes.ts")).toBeNull();
  });
});

describe("the repository", () => {
  test("has no key prefix in code or agent guidance", () => {
    const message = failureMessage(scan(listScannedFiles()));
    if (message) throw new Error(message);
  });
});
