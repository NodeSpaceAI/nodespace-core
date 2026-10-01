// Covers the literal node-type comparison check
// (scripts/check-node-type-literals.ts). Pattern tests run each pattern
// against lines that must and must not match; the scope tests cover which
// files and which parts of a file are scanned. The real-repo block at the
// bottom is the enforcement path (`bun run test:scripts`, so every merge
// gate) and throws the same actionable message the CLI prints.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { codeLines, failureMessage, findHits, isScanned, isTestPath, listScannedFiles, scan } from "./check-node-type-literals";

const hitsIn = (file: string, text: string) => findHits(file, text).map((hit) => hit.pattern);

describe("patterns", () => {
  const banned: [string, string][] = [
    ["a.rs", `if node.node_type == "collection" {`],
    ["a.rs", `if node.node_type != "task" {`],
    ["a.rs", `if node.node_type.as_str() == "schema" {`],
    ["a.rs", `DomainEvent::NodeCreated { node_type, .. } if node_type == "play" => {`],
    ["a.rs", `if "task" == node.node_type {`],
    ["a.rs", `if node.node_type != SKILL_NODE_TYPE {`],
    ["a.rs", `if node_type == crate::models::AI_CHAT_NODE_TYPE {`],
    ["a.rs", `if QUERY_NODE_TYPE == node.node_type {`],
    ["a.rs", `if matches!(node.node_type.as_str(), "task" | "collection") {`],
    ["a.rs", `"SELECT * FROM node WHERE node_type = 'collection' ORDER BY content"`],
    ["a.rs", `"AND cn.node_type != 'person'"`],
    ["a.rs", `"WHERE node_type NOT IN ('collection', 'schema', 'ai-chat')"`],
    ["a.rs", `"WHERE n.node_type IN ('collection')"`],
    ["a.rs", `"AND n.node_type NOT IN \\"`],
    ["a.ts", `if (node.nodeType === 'task') {`],
    ["a.ts", `return node.nodeType !== "ai-chat";`],
    ["a.ts", `if ('schema' === event.payload.nodeType) {`],
    ["a.svelte", `const person = $derived(node?.nodeType === 'person' ? node : undefined);`],
  ];
  for (const [file, line] of banned) {
    test(`flags: ${line}`, () => {
      expect(hitsIn(file, line).length).toBe(1);
    });
  }

  const allowed: [string, string][] = [
    ["a.rs", `if CoreNodeType::Schema.is_exactly(&node.node_type) {`],
    ["a.rs", `if self.type_is_a(&node.node_type, CoreNodeType::Collection).await? {`],
    ["a.rs", `let sql = format!("SELECT id FROM node WHERE {}", is_a_sql("node_type", &[CoreNodeType::Collection]));`],
    ["a.rs", `"WHERE n.node_type IN (SELECT node_type FROM type_ancestry WHERE ancestor = ?1)"`],
    ["a.rs", `conditions.push(format!("node_type IN ({})", placeholders.join(", ")));`],
    ["a.rs", `"SELECT id FROM node WHERE node_type IN (?1, ?2)"`],
    ["a.rs", `if existing.node_type == updated.node_type {`],
    ["a.rs", `if node_type == other_type {`],
    ["a.rs", `"SELECT ancestor FROM type_ancestry WHERE node_type = ?1 ORDER BY depth"`],
    ["a.rs", `if relationship_type == "has_child" {`],
    ["a.ts", `if (isA(node.nodeType, 'task')) {`],
    ["a.ts", `if (a.nodeType === b.nodeType) {`],
    ["a.ts", `if (typeof node.nodeType === 'string') {`],
    ["a.rs", `"search_nodes(node_type='task', filters=[])"`],
  ];
  for (const [file, line] of allowed) {
    test(`allows: ${line}`, () => {
      expect(hitsIn(file, line)).toEqual([]);
    });
  }
});

describe("scope", () => {
  test("comment lines are not code", () => {
    const text = ["// node.node_type == \"task\" would miss subtypes", "/// `node_type == \"schema\"`", " * nodeType === 'task'", "let x = 1;"].join("\n");
    expect(codeLines("a.rs", text).map((l) => l.line)).toEqual([4]);
    expect(findHits("a.rs", text)).toEqual([]);
  });

  test("a Rust file is scanned up to its test module", () => {
    const text = ['fn rule(node: &Node) -> bool { node.node_type == "task" }', "", "#[cfg(test)]", "mod tests {", '    fn t(node: &Node) -> bool { node.node_type == "task" }', "}"].join("\n");
    const hits = findHits("packages/core/src/a.rs", text);
    expect(hits.map((hit) => hit.line)).toEqual([1]);
  });

  test("a cfg(test) attribute on something other than a module does not end the scan", () => {
    const text = ["#[cfg(test)]", "use helper::thing;", 'fn rule(node: &Node) -> bool { node.node_type == "task" }'].join("\n");
    expect(findHits("packages/core/src/a.rs", text).map((hit) => hit.line)).toEqual([3]);
  });

  test("test files and the registry modules are out of scope", () => {
    expect(isTestPath("packages/core/tests/it/a.rs")).toBe(true);
    expect(isTestPath("packages/core/src/schema/schema_test.rs")).toBe(true);
    expect(isTestPath("packages/core/src/playbook/tests.rs")).toBe(true);
    expect(isTestPath("packages/desktop-app/src/tests/a.test.ts")).toBe(true);
    expect(isTestPath("packages/core/src/services/node_service/crud.rs")).toBe(false);

    expect(isScanned("packages/core/src/services/node_service/crud.rs")).toBe(true);
    expect(isScanned("packages/desktop-app/src/lib/a.svelte")).toBe(true);
    expect(isScanned("packages/core/tests/it/a.rs")).toBe(false);
    expect(isScanned("packages/nodespace-types/src/core_type.rs")).toBe(false);
    expect(isScanned("packages/desktop-app/src/lib/types/core-node-types.ts")).toBe(false);
    expect(isScanned("scripts/gh-utils.ts")).toBe(false);
    expect(isScanned("packages/core/Cargo.toml")).toBe(false);
  });

  test("the failure message names each hit and the fix", () => {
    expect(failureMessage([])).toBeNull();
    const message = failureMessage(findHits("packages/core/src/a.rs", 'if node.node_type == "task" {}'));
    expect(message).toContain("packages/core/src/a.rs:1");
    expect(message).toContain("type_is_a");
    expect(message).toContain("is_exactly");
  });
});

describe("the repository", () => {
  test("has no literal node-type comparison outside tests", () => {
    const files = listScannedFiles();
    expect(files.length).toBeGreaterThan(100);
    const message = failureMessage(scan(files));
    if (message) throw new Error(message);
  });
});
