// Covers the offline, deterministic parts of scripts/publish-skill-repo.ts:
// version normalization and file rendering. pushSkillUpdate (the
// GitHub-talking function) is intentionally not exercised here -- this suite
// runs as part of `bun run test:scripts` / `test:all` (the pre-push gate),
// which must stay fast and deterministic, not depend on network or a real
// SKILL_REPO_TOKEN.
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { SHARED_SKILL_FRONTMATTER } from "../packages/skill/src/agents";
import {
  extractSkillMeta,
  normalizeVersion,
  readSkillSource,
  renderMarketplaceFile,
  renderPublishFiles,
  sharedShimPaths,
  SKILL_REPO,
} from "./publish-skill-repo";

const REPO_ROOT = join(dirname(new URL(import.meta.url).pathname), "..");

describe("normalizeVersion", () => {
  test("strips a leading v", () => {
    expect(normalizeVersion("v0.2.2")).toBe("0.2.2");
  });

  test("leaves a bare version untouched", () => {
    expect(normalizeVersion("0.2.2")).toBe("0.2.2");
  });
});

describe("SKILL_REPO", () => {
  test("targets the public generated-only skill repo", () => {
    expect(SKILL_REPO).toBe("NodeSpaceAI/nodespace-skill");
  });
});

describe("readSkillSource", () => {
  test("reads the live packages/skill/SKILL.md, references/cli.md, and references/shared-workspaces.md off disk", () => {
    const { body, referenceCli, referenceSharedWorkspaces } = readSkillSource();
    // Read independently (not via the function under test) so this actually
    // catches the function reading a stale/wrong path, not just echoing it.
    const expectedBody = readFileSync(
      join(REPO_ROOT, "packages", "skill", "SKILL.md"),
      "utf8",
    );
    const expectedReferenceCli = readFileSync(
      join(REPO_ROOT, "packages", "skill", "references", "cli.md"),
      "utf8",
    );
    const expectedReferenceSharedWorkspaces = readFileSync(
      join(REPO_ROOT, "packages", "skill", "references", "shared-workspaces.md"),
      "utf8",
    );
    expect(body).toBe(expectedBody);
    expect(referenceCli).toBe(expectedReferenceCli);
    expect(referenceSharedWorkspaces).toBe(expectedReferenceSharedWorkspaces);
  });

  // The checked-in SKILL.md body carries no frontmatter (renderPublishFiles
  // is the one place that adds it) -- a body that already starts with `---`
  // would double up frontmatter blocks in the published file.
  test("the body has no baked-in frontmatter", () => {
    const { body } = readSkillSource();
    expect(body.startsWith("---")).toBe(false);
  });
});

describe("sharedShimPaths", () => {
  // Guards the drift class packages/skill/src/tests/installer.test.ts's
  // "publishes every directory the agents install from" test guards
  // elsewhere: this derives the published file set from AGENTS instead of a
  // hardcoded list, so a shared file added to (or removed from) every
  // agent's shims here gets picked up automatically -- and this test fails
  // loudly if that derivation ever stops matching what today's AGENTS
  // actually declares as shared.
  test("is exactly SKILL.md, references/cli.md, and references/shared-workspaces.md today", () => {
    expect(sharedShimPaths().sort()).toEqual([
      "SKILL.md",
      "references/cli.md",
      "references/shared-workspaces.md",
    ]);
  });

  test("excludes every harness-specific shim", () => {
    // Antigravity carries no harness-specific shim at all (see
    // packages/skill/src/agents.ts) -- it's a shell-capable agent that just
    // runs `nodespace` directly, so there's nothing of its own to exclude here.
    const shared = new Set(sharedShimPaths());
    expect(shared.has("shims/claude-code/nodespace-hook.ts")).toBe(false);
    expect(shared.has("shims/codex/nodespace-plugin.ts")).toBe(false);
    expect(shared.has("shims/opencode/nodespace-plugin.ts")).toBe(false);
  });
});

describe("renderPublishFiles", () => {
  test("publishes exactly SKILL.md, references/cli.md, and references/shared-workspaces.md under skills/nodespace/", () => {
    const files = renderPublishFiles("v0.2.2");
    expect(files.map((f) => f.relPath).sort()).toEqual([
      "skills/nodespace/SKILL.md",
      "skills/nodespace/references/cli.md",
      "skills/nodespace/references/shared-workspaces.md",
    ]);
  });

  test("SKILL.md is spec-compliant frontmatter + the unmodified body", () => {
    const files = renderPublishFiles("v0.2.2");
    const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
    const { body } = readSkillSource();

    expect(skillMd.content.startsWith("---\nname: nodespace\n")).toBe(true);
    expect(skillMd.content).toContain(body);
    // name must match the directory it publishes into.
    expect(skillMd.relPath.split("/")[1]).toBe("nodespace");
  });

  test("stamps a compatibility field with the released app version, within the spec's 500-char limit", () => {
    const files = renderPublishFiles("v0.2.2");
    const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
    const m = /^compatibility:\s*(.+)$/m.exec(skillMd.content);
    expect(m).toBeTruthy();
    expect(m![1]).toContain("v0.2.2");
    expect(m![1].length).toBeLessThanOrEqual(500);
  });

  test("compatibility field names both the shell and the MCP-connector requirement, not just the CLI", () => {
    // A published skill installed onto a bash-less MCP surface should be
    // able to tell, from the frontmatter alone, that it needs either a shell
    // or an MCP connector to `nodespace mcp` -- not just "the CLI on $PATH",
    // which reads as a shell-only requirement and doesn't warn a bash-less
    // installer that it needs the MCP passthrough instead. See SKILL.md's
    // Preflight Check (Branch 2) for the guidance this string points at.
    const files = renderPublishFiles("v0.2.2");
    const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
    const m = /^compatibility:\s*(.+)$/m.exec(skillMd.content);
    expect(m).toBeTruthy();
    const compatibility = m![1];
    expect(compatibility.toLowerCase()).toContain("shell");
    expect(compatibility).toContain("nodespace mcp");
  });

  test("normalizes a leading v the same way for the compatibility field", () => {
    const withV = renderPublishFiles("v0.2.2").find(
      (f) => f.relPath === "skills/nodespace/SKILL.md",
    )!;
    const withoutV = renderPublishFiles("0.2.2").find(
      (f) => f.relPath === "skills/nodespace/SKILL.md",
    )!;
    expect(withV.content).toBe(withoutV.content);
  });

  test("references/cli.md is copied through verbatim", () => {
    const files = renderPublishFiles("v0.2.2");
    const referenceCli = files.find((f) => f.relPath === "skills/nodespace/references/cli.md")!;
    const { referenceCli: expected } = readSkillSource();
    expect(referenceCli.content).toBe(expected);
  });

  test("references/shared-workspaces.md is copied through verbatim", () => {
    const files = renderPublishFiles("v0.2.2");
    const referenceSharedWorkspaces = files.find(
      (f) => f.relPath === "skills/nodespace/references/shared-workspaces.md",
    )!;
    const { referenceSharedWorkspaces: expected } = readSkillSource();
    expect(referenceSharedWorkspaces.content).toBe(expected);
  });

  // SKILL.md's stub for the moved section must still name the file it points
  // at, or the publish step would ship a reference nothing in the body links
  // to -- the same dangling-reference failure mode the issue that added this
  // file exists to prevent, just checked against the published copy instead
  // of the local one.
  test("SKILL.md links to references/shared-workspaces.md by the exact published path", () => {
    const files = renderPublishFiles("v0.2.2");
    const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
    expect(skillMd.content).toContain("references/shared-workspaces.md");
  });
});

describe("extractSkillMeta", () => {
  test("extracts the skill name from the shared frontmatter", () => {
    expect(extractSkillMeta(SHARED_SKILL_FRONTMATTER).name).toBe("nodespace");
  });

  test("unfolds the description block into a single-line, whitespace-clean string", () => {
    const { description } = extractSkillMeta(SHARED_SKILL_FRONTMATTER);
    expect(description).not.toContain("\n");
    expect(description).not.toMatch(/ {2}/);
    expect(description).toContain("NodeSpace knowledge graph");
    expect(description.startsWith("Context infrastructure for AI-native development.")).toBe(
      true,
    );
  });

  test("throws a descriptive error when the frontmatter has no name field", () => {
    expect(() => extractSkillMeta("description: >\n  x\n")).toThrow(/name/);
  });

  test("throws a descriptive error when the frontmatter has no folded description block", () => {
    expect(() => extractSkillMeta("name: nodespace\n")).toThrow(/description/);
  });
});

describe("renderMarketplaceFile", () => {
  test("publishes .claude-plugin/marketplace.json at the repo root, not under skills/nodespace/", () => {
    const file = renderMarketplaceFile("v0.2.2");
    expect(file.relPath).toBe(".claude-plugin/marketplace.json");
  });

  test("renders valid JSON matching the documented marketplace shape (name, owner, plugins[])", () => {
    const manifest = JSON.parse(renderMarketplaceFile("v0.2.2").content);
    const [ownerName, marketplaceName] = SKILL_REPO.split("/");

    expect(manifest.name).toBe(marketplaceName);
    expect(manifest.owner).toEqual({
      name: ownerName,
      url: `https://github.com/${ownerName}`,
    });
    expect(Array.isArray(manifest.plugins)).toBe(true);
    expect(manifest.plugins).toHaveLength(1);
  });

  test("plugin name/description derive from the shared skill frontmatter, not a second hand-written copy", () => {
    const manifest = JSON.parse(renderMarketplaceFile("v0.2.2").content);
    const meta = extractSkillMeta(SHARED_SKILL_FRONTMATTER);
    const plugin = manifest.plugins[0];

    expect(plugin.name).toBe(meta.name);
    expect(plugin.description).toBe(meta.description);
    // The marketplace-level description reuses the same source too, rather
    // than being independently hand-written text about the same skill.
    expect(manifest.description).toBe(meta.description);
  });

  test("plugin and marketplace version come from the release argument, normalized the same way as SKILL.md's compatibility field", () => {
    const withV = renderMarketplaceFile("v0.2.2");
    const withoutV = renderMarketplaceFile("0.2.2");
    expect(withV.content).toBe(withoutV.content);

    const manifest = JSON.parse(withV.content);
    expect(manifest.version).toBe("0.2.2");
    expect(manifest.plugins[0].version).toBe("0.2.2");
  });

  test("plugin source is the marketplace root with no explicit skills override, so the default skills/ scan finds skills/nodespace/", () => {
    const plugin = JSON.parse(renderMarketplaceFile("v0.2.2").content).plugins[0];
    expect(plugin.source).toBe("./");
    expect(plugin.skills).toBeUndefined();
  });

  test("plugin license derives from packages/skill/package.json, not a hardcoded copy", () => {
    const expectedLicense = JSON.parse(
      readFileSync(join(REPO_ROOT, "packages", "skill", "package.json"), "utf8"),
    ).license;
    const plugin = JSON.parse(renderMarketplaceFile("v0.2.2").content).plugins[0];
    expect(plugin.license).toBe(expectedLicense);
  });

  test("plugin repository points at the published skill repo", () => {
    const plugin = JSON.parse(renderMarketplaceFile("v0.2.2").content).plugins[0];
    expect(plugin.repository).toBe(`https://github.com/${SKILL_REPO}`);
  });
});
