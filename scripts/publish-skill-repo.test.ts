// Covers scripts/publish-skill-repo.ts: version normalization, file
// rendering, and `--push`. This suite runs as part of `bun run test:scripts` /
// `test:all` (the merge gate), which must stay fast and deterministic, so
// `--push` runs against a local bare repository standing in for SKILL_REPO
// (scripts/fake-external-repo.ts), never the network or a real
// SKILL_REPO_TOKEN.
import { describe, expect, setDefaultTimeout, test } from "bun:test";
import { $ } from "bun";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { SHARED_SKILL_FRONTMATTER } from "../packages/skill/src/agents";
import { createFakeRemote } from "./fake-external-repo";
import {
  extractSkillMeta,
  normalizeVersion,
  readSkillSource,
  renderMarketplaceFile,
  renderPublishFiles,
  publishedSkillPaths,
  renderPluginFiles,
  SKILL_PUBLISH_DIR,
  SKILL_REPO,
} from "./publish-skill-repo";

const REPO_ROOT = join(dirname(new URL(import.meta.url).pathname), "..");

// The `--push` test runs bun and git processes. Bun's 5s default per-test
// timeout is tight on the loaded machines these tests run on (the merge gate
// shares them with Rust builds), and a timeout here would eject an unrelated PR.
setDefaultTimeout(30_000);

// Every reference file under packages/skill/references/, spelled out. The file
// list comes from the directory rather than a list kept per agent, so a file
// added or dropped there is a deliberate change to this list.
const REFERENCES = [
  "references/cli.md",
  "references/graph-authored-guidance.md",
];

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
  test("reads the live packages/skill/SKILL.md and every references/*.md off disk", () => {
    const { body, references } = readSkillSource();
    // Read independently (not via the function under test) so this actually
    // catches the function reading a stale/wrong path, not just echoing it.
    const expectedBody = readFileSync(
      join(REPO_ROOT, "packages", "skill", "SKILL.md"),
      "utf8",
    );
    const referencesDir = join(REPO_ROOT, "packages", "skill", "references");
    const expectedReferences = Object.fromEntries(
      readdirSync(referencesDir)
        .filter((name) => name.endsWith(".md"))
        .map((name) => [`references/${name}`, readFileSync(join(referencesDir, name), "utf8")]),
    );
    expect(body).toBe(expectedBody);
    expect(Object.keys(expectedReferences).length).toBeGreaterThan(0);
    expect(references).toEqual(expectedReferences);
  });

  // The checked-in SKILL.md body carries no frontmatter (renderPublishFiles
  // is the one place that adds it) -- a body that already starts with `---`
  // would double up frontmatter blocks in the published file.
  test("the body has no baked-in frontmatter", () => {
    const { body } = readSkillSource();
    expect(body.startsWith("---")).toBe(false);
  });
});

describe("publishedSkillPaths", () => {
  // Guards the drift class packages/skill/src/tests/installer.test.ts's
  // "publishes every directory the agents install from" test guards
  // elsewhere: this derives the published file set from `SKILL.md` plus the
  // references directory instead of a hardcoded list, so a
  // reference added to (or removed from) packages/skill/references/ is picked
  // up automatically -- and this test fails loudly if that derivation ever
  // stops matching what the skill actually contains.
  test("is exactly SKILL.md plus every reference in packages/skill/references/ today", () => {
    expect(publishedSkillPaths().sort()).toEqual(["SKILL.md", ...REFERENCES]);
  });

  test("includes every reference file the installer would install, and nothing else from references/", () => {
    const referencesDir = join(REPO_ROOT, "packages", "skill", "references");
    const onDisk = readdirSync(referencesDir)
      .filter((name) => name.endsWith(".md"))
      .map((name) => `references/${name}`);
    const published = publishedSkillPaths().filter((path) => path.startsWith("references/"));
    expect(published.sort()).toEqual(onDisk.sort());
  });

  test("publishes no harness plugin file inside the skill folder", () => {
    // The skill folder is the generic Agent Skills folder any harness can
    // take. The Claude Code plugin is published at the repository root.
    expect(publishedSkillPaths().filter((path) => path.startsWith("plugins/") || path.startsWith("hooks/"))).toEqual([]);
  });

  // Guidance that only another build's users need never reaches the public
  // skill repository (ADR-082 section 6). The exact-list test above already
  // implies this; this one names the removed file. Its name is built from
  // fragments, so a search of the repository for it finds no copy here.
  test("does not publish the removed multi-user reference", () => {
    expect(publishedSkillPaths()).not.toContain(["references/shared", "workspaces.md"].join("-"));
  });

  // A build adds guidance with NODESPACE_SKILL_EXTENSIONS (scripts/build-skill.ts).
  // The publish reads packages/skill directly and never that variable, so a
  // set variable changes nothing the public repository receives.
  test("ignores NODESPACE_SKILL_EXTENSIONS: an extension directory adds nothing to the publish", () => {
    const extension = mkdtempSync(join(tmpdir(), "publish-skill-extension-"));
    const saved = process.env.NODESPACE_SKILL_EXTENSIONS;
    try {
      mkdirSync(join(extension, "references"));
      writeFileSync(join(extension, "references", "extension-only.md"), "# Extension-only guidance\n");
      writeFileSync(join(extension, "SKILL.md"), "## Extension-only section\n");
      process.env.NODESPACE_SKILL_EXTENSIONS = extension;
      expect(publishedSkillPaths().sort()).toEqual(["SKILL.md", ...REFERENCES]);
      const files = renderPublishFiles("v0.2.2");
      expect(files.map((f) => f.relPath)).not.toContain("skills/nodespace/references/extension-only.md");
      const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
      expect(skillMd.content).not.toContain("Extension-only section");
    } finally {
      if (saved === undefined) delete process.env.NODESPACE_SKILL_EXTENSIONS;
      else process.env.NODESPACE_SKILL_EXTENSIONS = saved;
      rmSync(extension, { recursive: true, force: true });
    }
  });
});

describe("renderPublishFiles", () => {
  test("publishes exactly SKILL.md plus every reference under skills/nodespace/", () => {
    const files = renderPublishFiles("v0.2.2");
    expect(files.map((f) => f.relPath).sort()).toEqual(
      ["SKILL.md", ...REFERENCES].map((path) => `skills/nodespace/${path}`),
    );
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
    // Reaching NodeSpace section for the guidance this string points at.
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

  test("every reference is copied through verbatim", () => {
    const files = renderPublishFiles("v0.2.2");
    const { references } = readSkillSource();
    expect(Object.keys(references).sort()).toEqual(REFERENCES);
    for (const [path, expected] of Object.entries(references)) {
      const published = files.find((f) => f.relPath === `skills/nodespace/${path}`);
      expect(published?.content, path).toBe(expected);
    }
  });

  // SKILL.md's stub for the moved section must still name the file it points
  // at, or the publish step would ship a reference nothing in the body links
  // to: a dangling reference, checked against the published copy instead of
  // the local one.
  test("SKILL.md links to references/graph-authored-guidance.md by the exact published path", () => {
    const files = renderPublishFiles("v0.2.2");
    const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
    expect(skillMd.content).toContain("references/graph-authored-guidance.md");
  });

  // The public copy of the skill must not link to a file the public repo does
  // not carry: the work-tracking playbooks SKILL.md points at were linked but
  // never published while the file list was hand-kept.
  test("every references/... link in the published SKILL.md resolves to a published file", () => {
    const files = renderPublishFiles("v0.2.2");
    const skillMd = files.find((f) => f.relPath === "skills/nodespace/SKILL.md")!;
    const published = new Set(files.map((f) => f.relPath));

    const links = [...new Set(skillMd.content.match(/references\/[A-Za-z0-9._-]+\.md/g))];
    expect(links.length).toBeGreaterThan(0);
    for (const link of links) {
      expect(published.has(`skills/nodespace/${link}`), `SKILL.md links ${link}`).toBe(true);
    }
  });
});

describe("renderPluginFiles", () => {
  test("publishes the installed plugin's files at the repository root, the manifest stamped with the release", () => {
    const files = renderPluginFiles("v0.2.2");
    expect(files.map((f) => f.relPath).sort()).toEqual([
      ".claude-plugin/plugin.json",
      "hooks/hooks.json",
      "hooks/register.ts",
      "types/index.d.ts",
    ]);

    const manifest = JSON.parse(files.find((f) => f.relPath === ".claude-plugin/plugin.json")!.content);
    expect(manifest.version).toBe("0.2.2");
    // The marketplace entry names the plugin `nodespace`; a manifest that
    // disagreed would install under another name.
    expect(manifest.name).toBe(JSON.parse(renderMarketplaceFile("v0.2.2").content).plugins[0].name);

    const pluginDir = join(REPO_ROOT, "packages", "skill", "plugins", "claude-code");
    for (const file of files.filter((f) => f.relPath !== ".claude-plugin/plugin.json")) {
      expect(file.content).toBe(readFileSync(join(pluginDir, file.relPath), "utf8"));
    }
  });

  // The plugin's own tests run inside Claude Code, outside the gate. This is
  // the gate's one read of the module: a file that does not parse never ships.
  test("the hooks module parses", () => {
    const module = renderPluginFiles("v0.2.2").find((f) => f.relPath === "hooks/register.ts")!;
    const js = new Bun.Transpiler({ loader: "ts" }).transformSync(module.content);
    expect(js).toContain("register");
  });

  test("the hooks file names a module that is published", () => {
    const files = renderPluginFiles("v0.2.2");
    const hooks = JSON.parse(files.find((f) => f.relPath === "hooks/hooks.json")!.content) as { modules: string[] };
    for (const module of hooks.modules) {
      expect(files.map((f) => f.relPath)).toContain(join("hooks", module));
    }
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

describe("--push", () => {
  // The public repo after a release that published a reference
  // packages/skill has since dropped: the next publish must delete it, and
  // must not touch the repo's own files outside SKILL_PUBLISH_DIR.
  test("removes a published file packages/skill no longer has, rewrites every rendered file, and keeps the repo's own files", async () => {
    const stale = `${SKILL_PUBLISH_DIR}/references/removed-guidance.md`;
    const remote = await createFakeRemote(SKILL_REPO, "test-token", {
      "README.md": "hand-written readme\n",
      ".claude-plugin/marketplace.json": "{}\n",
      [`${SKILL_PUBLISH_DIR}/SKILL.md`]: "previous release\n",
      [stale]: "guidance packages/skill no longer has\n",
    });
    const restoreEnv = remote.use();
    try {
      const script = join(REPO_ROOT, "scripts", "publish-skill-repo.ts");
      const result = await $`${process.execPath} ${script} v0.2.2 --push`
        .env({ ...process.env, SKILL_REPO_TOKEN: "test-token" })
        .quiet()
        .nothrow();
      expect(result.exitCode, result.stderr.toString()).toBe(0);

      const rendered = [
        ...renderPublishFiles("v0.2.2"),
        ...renderPluginFiles("v0.2.2"),
        renderMarketplaceFile("v0.2.2"),
      ];
      const tree = await remote.tree();
      expect(tree).not.toContain(stale);
      expect(tree).toEqual(["README.md", ...rendered.map((f) => f.relPath)].sort());
      expect(await remote.show("README.md")).toBe("hand-written readme\n");
      for (const file of rendered) {
        expect(await remote.show(file.relPath), file.relPath).toBe(file.content);
      }
    } finally {
      restoreEnv();
      remote.cleanup();
    }
  });
});
