import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The per-agent ACP shims in `packages/skill/shims/` hand-author the same tool
// definitions. The duplication is deliberate — each shim is copied into an
// agent session's temp directory as a standalone script with no shared module
// resolution at runtime, so they cannot import a single source. This test is the
// automated guard that they never drift out of agreement: any rename or
// description edit in one shim (without the others) fails here.
//
// Antigravity CLI carries no shim here — its native tool-registration path is
// MCP, and it points at the already-existing `nodespace mcp` stdio server
// (packages/cli/src/commands/mcp.rs) instead of a hand-authored tool list, so
// there is nothing in packages/skill/shims/ for it to drift against.
//
// What each remaining shim declares:
//  - codex / opencode / pi (`nodespace-plugin.ts` / `nodespace-extension.ts`):
//    `name` + `description` per tool object — codex is the reference.
//  - claude-code (`nodespace-hook.ts`): registers by NAME via `hook('name', …)`;
//    it carries no per-tool description, so only its tool NAMES are comparable.

const shimsDir = fileURLToPath(new URL("../../shims/", import.meta.url));
const read = (rel: string) => readFileSync(shimsDir + rel, "utf8");

const EXPECTED_TOOLS = [
  "nodespace_create_node",
  "nodespace_get_children",
  "nodespace_get_node",
  "nodespace_search_semantic",
  "nodespace_update_node",
];

/**
 * Extract `{ name: description }` from a `.ts` plugin shim. A tool's own
 * description is the `description:` that immediately follows its `name:`;
 * parameter descriptions sit later inside the schema, so the adjacency excludes
 * them.
 */
function tsPluginToolMap(rel: string): Record<string, string> {
  const src = read(rel);
  const re =
    /name:\s*'(nodespace_[a-z_]+)',\s*\n\s*description:\s*'((?:[^'\\]|\\.)*)'/g;
  const map: Record<string, string> = {};
  for (const m of src.matchAll(re)) map[m[1]] = m[2];
  return map;
}

/** claude-code registers each tool by name via `hook('name', …)`. */
function claudeCodeToolNames(): string[] {
  const src = read("claude-code/nodespace-hook.ts");
  return [...src.matchAll(/hook\(\s*'(nodespace_[a-z_]+)'/g)]
    .map((m) => m[1])
    .sort();
}

describe("ACP shim tool parity", () => {
  const codex = tsPluginToolMap("codex/nodespace-plugin.ts");

  it("codex declares exactly the expected tool set", () => {
    expect(Object.keys(codex).sort()).toEqual(EXPECTED_TOOLS);
  });

  it("opencode tool names + descriptions match codex", () => {
    expect(tsPluginToolMap("opencode/nodespace-plugin.ts")).toEqual(codex);
  });

  it("pi tool names + descriptions match codex", () => {
    expect(tsPluginToolMap("pi/nodespace-extension.ts")).toEqual(codex);
  });

  it("claude-code registers exactly the expected tool names", () => {
    // claude-code carries no per-tool descriptions, so only names are comparable.
    expect(claudeCodeToolNames()).toEqual(EXPECTED_TOOLS);
  });
});
