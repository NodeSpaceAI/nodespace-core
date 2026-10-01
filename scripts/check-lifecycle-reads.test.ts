// Covers the lifecycle_status read check (scripts/check-lifecycle-reads.ts).
// Pattern tests run lines that must and must not be flagged; the scope tests
// cover which files are scanned and what the envelope files may do. The
// real-repo block at the bottom is the enforcement path (`bun run
// test:scripts`, so every merge gate) and throws the same actionable message
// the CLI prints.
//
// DOM-free on purpose: this file runs under `bun test scripts/`, which
// bypasses the Happy-DOM vitest config (see CLAUDE.md).
import { describe, expect, test } from "bun:test";
import { ENVELOPE_FILES, GOVERNANCE_MODULE, failureMessage, findHits, isScanned, listScannedFiles, scan } from "./check-lifecycle-reads";

const hitsIn = (file: string, text: string) => findHits(file, text).map((hit) => hit.pattern);

describe("interpreting the field", () => {
  const banned: [string, string][] = [
    ["a.rs", `if node.lifecycle_status == "archived" && !include_archived {`],
    ["a.rs", `.filter(|n| n.lifecycle_status == status)`],
    ["a.rs", `Ok(Some(node)) if node.lifecycle_status == "active" => {`],
    ["a.rs", `if "archived" == node.lifecycle_status {`],
    ["a.rs", `if node.lifecycle_status != governance::ACTIVE {`],
    ["a.rs", `(&current_status, node.lifecycle_status.as_str()),`],
    ["a.rs", `match node.lifecycle_status.as_str() {`],
    ["a.rs", `if matches!(update.lifecycle_status.as_deref(), Some("archived")) {`],
    ["a.rs", `"SELECT id FROM node WHERE LOWER(title) = ?1 AND lifecycle_status = 'active' LIMIT 1"`],
    ["a.rs", `"WHERE node_title_fts MATCH ?1 AND n.lifecycle_status != 'archived'"`],
    ["a.rs", `"AND n.lifecycle_status IN ('active')"`],
    ["a.rs", `"UPDATE node SET lifecycle_status = 'archived', version = version + 1 WHERE id = ?1"`],
    ["a.rs", `"SELECT id FROM node WHERE lifecycle_status = ?1 LIMIT 1"`],
    ["a.rs", `"JOIN node n ON n.id = r.in_node AND n.lifecycle_status != ?2"`],
    ["a.rs", `"and n.lifecycle_status in (?1, ?2)"`],
    ["a.rs", `"SELECT CASE lifecycle_status WHEN 'active' THEN 1 ELSE 0 END FROM node"`],
    ["a.rs", `match lifecycle_status.as_str() {`],
    ["a.rs", `if matches!(lifecycle_status, Some(ref s) if s.is_empty()) {`],
    ["a.rs", `"lifecycle_status" => Some(serde_json::Value::String(status)),`],
    ["a.rs", `if let Some(status) = params.get("lifecycle_status").and_then(|v| v.as_str()) {`],
    ["a.rs", `root_props["lifecycle_status"] = serde_json::json!("archived");`],
    ["a.rs", `"id" | "node_type" | "content" | "version" | "lifecycle_status"`],
    ["a.ts", `if (node.lifecycleStatus === 'archived') {`],
    ["a.ts", `return chat.lifecycleStatus !== 'active';`],
    ["a.svelte", `const readOnly = $derived(node?.lifecycleStatus === 'archived');`],
  ];
  for (const [file, line] of banned) {
    test(`flags: ${line}`, () => {
      expect(hitsIn(file, line).length).toBe(1);
    });
  }

  test("is banned in the envelope files too", () => {
    for (const file of ENVELOPE_FILES) {
      expect(hitsIn(file, `if node.lifecycle_status == "archived" {`)).toEqual(["comparison"]);
      expect(hitsIn(file, `"SELECT * FROM node WHERE lifecycle_status = 'active'"`)).toEqual(["sqlComparison"]);
    }
  });
});

describe("reading the field", () => {
  const banned: [string, string][] = [
    ["a.rs", `Value::String(Arc::new(node.lifecycle_status.clone())),`],
    ["a.rs", `let status = &trigger_node.lifecycle_status;`],
    ["a.rs", `show(existing.lifecycle_status.clone());`],
    ["a.ts", `const status = node.lifecycleStatus;`],
    ["a.svelte", `{node.lifecycleStatus}`],
  ];
  for (const [file, line] of banned) {
    test(`flags: ${line}`, () => {
      expect(hitsIn(file, line)).toEqual(["read"]);
    });
  }

  const allowed: [string, string][] = [
    // The governance module's own forms.
    ["a.rs", `if !crate::governance::participates(&node) {`],
    ["a.rs", `format!("SELECT id FROM node WHERE {}", crate::governance::participates_sql(""))`],
    // Assigning it.
    ["a.rs", `updated.lifecycle_status = status.clone();`],
    ["a.rs", `node.lifecycle_status = "archived".to_string();`],
    ["a.ts", `request.lifecycleStatus = status;`],
    // Carrying it into the same-named field of another shape.
    ["a.rs", `lifecycle_status: node.lifecycle_status,`],
    ["a.rs", `lifecycle_status: Some(updated.lifecycle_status.clone()),`],
    ["a.rs", `lifecycle_status: "active".to_string(),`],
    ["a.rs", `"lifecycle_status": node.lifecycle_status,`],
    ["a.ts", `lifecycleStatus: n.lifecycleStatus,`],
    // A write request's field is the value being written.
    ["a.rs", `if let Some(status) = update.lifecycle_status {`],
    ["a.rs", `let lifecycle_status = match params.lifecycle_status {`],
    ["a.rs", `if let Some(status) = &update.lifecycle_status {`],
    ["a.ts", `if (body.lifecycleStatus) request.lifecycleStatus = body.lifecycleStatus;`],
    // A write binds the column; it doesn't filter on it.
    ["a.rs", `"UPDATE node SET lifecycle_status = ?1 WHERE id = ?2"`],
    ["a.rs", `"lifecycle_status = COALESCE(?4, lifecycle_status), \\"`],
    ["a.rs", `"UPDATE node SET content = ?1, lifecycle_status = ?5, version = ?6 WHERE id = ?8 AND version = ?9"`],
    ["a.rs", `let lifecycle_status = match params.lifecycle_status {`],
    ["a.rs", `"INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"`],
    // Prose that names the column is not SQL.
    ["a.rs", `return Err(anyhow!("lifecycle_status is required"));`],
    ["a.rs", `tracing::warn!("no lifecycle_status in the request");`],
    // Other fields that only share a word.
    ["a.rs", `let lifecycle = self.lifecycle.read().expect("lifecycle lock poisoned");`],
    ["a.ts", `if (chat.sessionStatus === 'ended') {`],
  ];
  for (const [file, line] of allowed) {
    test(`allows: ${line}`, () => {
      expect(hitsIn(file, line)).toEqual([]);
    });
  }

  test("is allowed in the envelope files", () => {
    for (const file of ENVELOPE_FILES) {
      expect(hitsIn(file, `Self::validate_lifecycle_status(&node.lifecycle_status)?;`)).toEqual([]);
      expect(hitsIn(file, `println!("lifecycle:       {}", node.lifecycle_status);`)).toEqual([]);
    }
  });
});

describe("scope", () => {
  test("comments and a Rust file's test module are not scanned", () => {
    const text = [
      '// node.lifecycle_status == "archived" hides the node',
      'fn rule(node: &Node) -> bool { node.lifecycle_status == "active" }',
      "",
      "#[cfg(test)]",
      "mod tests {",
      '    fn t(node: &Node) -> bool { node.lifecycle_status == "active" }',
      "}",
    ].join("\n");
    expect(findHits("packages/core/src/a.rs", text).map((hit) => hit.line)).toEqual([2]);
  });

  test("the governance module and test files are out of scope", () => {
    expect(isScanned(GOVERNANCE_MODULE)).toBe(false);
    expect(isScanned("packages/core/tests/it/a.rs")).toBe(false);
    expect(isScanned("packages/desktop-app/src/tests/a.test.ts")).toBe(false);
    expect(isScanned("scripts/gh-utils.ts")).toBe(false);

    expect(isScanned("packages/core/src/ops/search_ops.rs")).toBe(true);
    expect(isScanned("packages/desktop-app/src/lib/a.svelte")).toBe(true);
    for (const file of ENVELOPE_FILES) expect(isScanned(file)).toBe(true);
  });

  test("every envelope file exists", () => {
    const files = new Set(listScannedFiles());
    for (const file of ENVELOPE_FILES) expect(files.has(file)).toBe(true);
  });

  test("the failure message names each hit and the fix", () => {
    expect(failureMessage([])).toBeNull();
    const message = failureMessage(findHits("packages/core/src/a.rs", 'if node.lifecycle_status == "archived" {}'));
    expect(message).toContain("packages/core/src/a.rs:1");
    expect(message).toContain("governance::participates");
    expect(message).toContain("participates_sql");
  });
});

describe("the repository", () => {
  test("reads lifecycle_status only in the governance module", () => {
    const files = listScannedFiles();
    expect(files.length).toBeGreaterThan(100);
    const message = failureMessage(scan(files));
    if (message) throw new Error(message);
  });
});
