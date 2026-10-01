// The generated-TypeScript drift check (`bun run gen:types --check`), tested
// against fixture directories: what counts as drift, what the gate prints, and
// that regenerating leaves a directory holding exactly the generated set.
// Running the Rust generator itself is the gate stage's job, not this test's.
import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  applyGenerated,
  describeDrift,
  diffGenerated,
  GENERATED_DIR,
  generatedFileName,
  missingWireTypes,
  readGeneratedDir,
  wireTypesInSource,
  type GeneratedFiles,
} from "./gen-types";
import { classify } from "./gate-scope";
import { TIERS } from "./gate-stage";

const dirs: string[] = [];

/** A fixture directory holding `files`. */
function fixture(files: Record<string, string>): string {
  const dir = mkdtempSync(join(tmpdir(), "gen-types-test-"));
  dirs.push(dir);
  for (const [name, contents] of Object.entries(files)) writeFileSync(join(dir, name), contents);
  return dir;
}

afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

const TASK = "export type TaskNode = { status: string };\n";
const generated: GeneratedFiles = new Map([
  ["index.ts", "export type { TaskNode } from './task-node';\n"],
  ["task-node.ts", TASK],
]);

describe("diffGenerated", () => {
  test("a committed directory that matches has no drift", () => {
    const committed = readGeneratedDir(fixture(Object.fromEntries(generated)));
    expect(diffGenerated(generated, committed)).toEqual([]);
  });

  test("a Rust change without regenerated TypeScript is drift", () => {
    // The committed file still has the shape from before a field was added.
    const committed = readGeneratedDir(
      fixture({ ...Object.fromEntries(generated), "task-node.ts": "export type TaskNode = {};\n" })
    );
    expect(diffGenerated(generated, committed)).toEqual([{ file: "task-node.ts", kind: "changed" }]);
  });

  test("a new wire type with no committed file is drift", () => {
    const committed = readGeneratedDir(fixture({ "index.ts": generated.get("index.ts") as string }));
    expect(diffGenerated(generated, committed)).toEqual([{ file: "task-node.ts", kind: "missing" }]);
  });

  test("a committed file for a removed wire type is drift", () => {
    const committed = readGeneratedDir(fixture({ ...Object.fromEntries(generated), "old-node.ts": "export {};\n" }));
    expect(diffGenerated(generated, committed)).toEqual([{ file: "old-node.ts", kind: "stale" }]);
  });

  test("a hand edit to a generated file is drift", () => {
    const committed = readGeneratedDir(
      fixture({ ...Object.fromEntries(generated), "task-node.ts": `${TASK}// tweaked by hand\n` })
    );
    expect(diffGenerated(generated, committed).map((d) => d.kind)).toEqual(["changed"]);
  });

  test("reports every difference, sorted by file", () => {
    const committed = readGeneratedDir(fixture({ "task-node.ts": "stale contents\n", "zebra.ts": "export {};\n" }));
    expect(diffGenerated(generated, committed)).toEqual([
      { file: "index.ts", kind: "missing" },
      { file: "task-node.ts", kind: "changed" },
      { file: "zebra.ts", kind: "stale" },
    ]);
  });
});

describe("readGeneratedDir", () => {
  test("reads only .ts files, and an absent directory as empty", () => {
    const dir = fixture({ "a.ts": "a\n", "notes.txt": "ignored\n" });
    expect([...readGeneratedDir(dir).keys()]).toEqual(["a.ts"]);
    expect(readGeneratedDir(join(dir, "missing")).size).toBe(0);
  });
});

describe("applyGenerated", () => {
  test("leaves the directory holding exactly the generated files", () => {
    const dir = fixture({ "task-node.ts": "stale contents\n", "old-node.ts": "export {};\n", "keep.txt": "untouched\n" });
    applyGenerated(dir, generated, diffGenerated(generated, readGeneratedDir(dir)));

    expect(readGeneratedDir(dir)).toEqual(generated);
    expect(existsSync(join(dir, "old-node.ts"))).toBe(false);
    expect(readFileSync(join(dir, "keep.txt"), "utf8")).toBe("untouched\n");
    // A second run finds nothing to do.
    expect(diffGenerated(generated, readGeneratedDir(dir))).toEqual([]);
  });

  test("creates the directory on a first run", () => {
    const dir = join(fixture({}), "generated");
    applyGenerated(dir, generated, diffGenerated(generated, readGeneratedDir(dir)));
    expect(readGeneratedDir(dir)).toEqual(generated);
  });

  test("writes nothing when there is no drift", () => {
    const dir = fixture(Object.fromEntries(generated));
    mkdirSync(join(dir, "nested"));
    applyGenerated(dir, generated, []);
    expect(readGeneratedDir(dir)).toEqual(generated);
  });
});

describe("describeDrift", () => {
  test("names each file, why it drifted and how to fix it", () => {
    const report = describeDrift([
      { file: "task-node.ts", kind: "changed" },
      { file: "new-node.ts", kind: "missing" },
      { file: "old-node.ts", kind: "stale" },
    ]);
    expect(report).toContain("task-node.ts: out of date");
    expect(report).toContain("new-node.ts: not committed");
    expect(report).toContain("old-node.ts: no longer generated");
    expect(report).toContain("bun run gen:types");
  });
});

describe("wireTypesInSource", () => {
  test("finds a type that derives Serialize or Deserialize", () => {
    const source = [
      "#[derive(Debug, Clone, Serialize, Deserialize)]",
      '#[serde(rename_all = "camelCase")]',
      "pub struct NodeReference {",
      "    pub id: String,",
      "}",
      "",
      "#[derive(Debug, Deserialize)]",
      "pub struct CreateRequest {",
      "    pub id: String,",
      "}",
      "",
      "#[derive(Debug, Clone, PartialEq)]",
      "pub struct ResolvedRelationship {",
      "    pub stored_type: String,",
      "}",
    ].join("\n");
    expect(wireTypesInSource(source)).toEqual(["NodeReference", "CreateRequest"]);
  });

  test("finds a type that implements Serialize by hand", () => {
    const source = [
      "#[derive(Debug, Clone, PartialEq, Eq, Default)]",
      "pub enum TaskStatus {",
      "    Open,",
      "}",
      "",
      "impl Serialize for TaskStatus {",
      "    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { todo!() }",
      "}",
      "",
      "pub enum OnlyRead {",
      "    A,",
      "}",
      "",
      "impl<'de> Deserialize<'de> for OnlyRead {",
      "}",
    ].join("\n");
    expect(wireTypesInSource(source)).toEqual(["TaskStatus", "OnlyRead"]);
  });

  test("reads the derive through doc comments, comments and wrapped attributes", () => {
    const source = [
      "/// A wire type.",
      "#[derive(Debug, Clone, Serialize)]",
      "#[cfg_attr(",
      '    feature = "ts",',
      '    ts(type = "string")',
      ")]",
      "// A note between the attributes and the declaration.",
      "pub(crate) enum Wrapped {",
      "    A,",
      "}",
    ].join("\n");
    expect(wireTypesInSource(source)).toEqual(["Wrapped"]);
  });

  test("does not take a derive from the declaration above", () => {
    const source = ["#[derive(Serialize)]", "pub struct First;", "", "pub struct Second;"].join("\n");
    expect(wireTypesInSource(source)).toEqual(["First"]);
  });

  test("leaves out test fixtures, wherever the test module sits", () => {
    const source = [
      "#[cfg(test)]",
      "mod early_tests {",
      "    #[derive(Serialize)]",
      "    struct Fixture {",
      "        a: u8,",
      "    }",
      "}",
      "",
      "#[derive(Serialize)]",
      "pub struct AfterTheTests {",
      "    pub a: u8,",
      "}",
    ].join("\n");
    expect(wireTypesInSource(source)).toEqual(["AfterTheTests"]);
  });
});

describe("missingWireTypes", () => {
  const source = "#[derive(Serialize)]\npub struct AiChatNode {\n}\n\n#[derive(Serialize)]\npub struct NewNode {\n}\n";

  test("names a wire type that has no generated file, and where it is declared", () => {
    const files: GeneratedFiles = new Map([["ai-chat-node.ts", ""]]);
    expect(missingWireTypes(new Map([["nested/new.rs", source]]), files)).toEqual(["NewNode (nested/new.rs)"]);
  });

  test("is empty when every wire type has its file", () => {
    const files: GeneratedFiles = new Map([
      ["ai-chat-node.ts", ""],
      ["new-node.ts", ""],
    ]);
    expect(missingWireTypes(new Map([["a.rs", source]]), files)).toEqual([]);
  });

  test("names files as the generator does", () => {
    expect(generatedFileName("AiChatNode")).toBe("ai-chat-node.ts");
    expect(generatedFileName("Node")).toBe("node.ts");
  });
});

describe("the drift check in the gates", () => {
  test("the committed directory holds generated files", () => {
    const committed = readGeneratedDir(GENERATED_DIR);
    expect(committed.size).toBeGreaterThan(40);
    for (const contents of committed.values()) {
      expect(contents.startsWith("// Generated from `packages/nodespace-types`")).toBe(true);
    }
  });

  test("the stage runs the check, not a regeneration", () => {
    expect(TIERS.typesCheck.command).toBe("bun run types:check");
    const scripts = JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8")).scripts;
    expect(scripts["types:check"]).toBe("bun run scripts/gen-types.ts --check");
  });

  test("the merge gate and test:changed both run the stage", () => {
    for (const script of ["test-gate.ts", "test-changed.ts"]) {
      expect(readFileSync(join(import.meta.dir, script), "utf8")).toContain("run(TIERS.typesCheck)");
    }
  });

  test("everything the check reads reaches the tier that runs it", () => {
    for (const file of [
      "packages/nodespace-types/src/task.rs",
      "packages/desktop-app/src/lib/types/generated/task-node.ts",
      "packages/desktop-app/.prettierrc",
      "scripts/gen-types.ts",
    ]) {
      expect(classify([file]).rust).toBe(true);
    }
    // A hand-written frontend type does not.
    expect(classify(["packages/desktop-app/src/lib/types/task-node.ts"]).rust).toBe(false);
  });
});
