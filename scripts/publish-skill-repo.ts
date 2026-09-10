#!/usr/bin/env bun
/**
 * Publish the NodeSpace Agent Skill to NodeSpaceAI/nodespace-skill -- the
 * public repo external agent harnesses (Claude Code, Codex, Antigravity CLI,
 * OpenCode, ...) import to get NodeSpace's Agent Skill without installing
 * the NodeSpace app first.
 *
 * `@nodespaceai/skill` is not published to npm and this script does not
 * change that: the desktop app runs `packages/skill`'s built installer
 * directly with `bun` (never `npx`/`npm` -- see
 * packages/desktop-app/src-tauri/src/skill_setup.rs), so npm publishing was
 * never actually required for the app's own install path -- the package
 * never landing on the registry never blocked anything real. This repo is
 * the sole distribution channel for the harness-native import path.
 *
 * It never hand-writes the skill body: it copies `packages/skill/SKILL.md`
 * (the body -- the checked-in file carries no frontmatter, see
 * packages/skill/src/types.ts) and every shared `references/*.md` file
 * (see `sharedShimPaths`) verbatim, whatever they currently are. If a future change to
 * `packages/skill` shrinks SKILL.md to a stub with guidance fetched from the
 * graph at runtime instead, this script keeps working unmodified -- it has
 * no assumption baked in about the body's size or shape, only about where it
 * lives on disk.
 *
 * The published SKILL.md gets the same shared frontmatter every installer
 * target uses (`buildSkillFrontmatter` in packages/skill/src/agents.ts:
 * `name`, `description`, `allowed-tools` -- every field is one the Agent
 * Skills spec (agentskills.io) defines, `allowed-tools` included: it's
 * listed as one of the spec's six frontmatter fields, "Experimental" but not
 * Claude-Code-specific, so sharing it across all four installer targets --
 * and this generic publish target -- is spec-compliant, not a Claude Code
 * leak), plus one field the installer path doesn't set: `compatibility`,
 * stamped with the released NodeSpace app version so a user importing this
 * repo can tell which app version a given revision targets. That version
 * comes from the same release tag update-homebrew-cask.ts takes as its
 * `<version>` argument -- itself the canonical app version
 * scripts/check-version-sync.ts enforces (tauri.conf.json) -- not
 * re-derived from packages/skill/package.json's own version field, which is
 * NOT kept in sync with the app (it still reads 0.2.0 while the app ships
 * 0.2.2) and would be a second, already-drifted source of truth.
 *
 * This is a generated-only repo, same contract as homebrew-nodespace
 * (NodeSpaceAI/nodespace-skill's own README says so): every push here
 * overwrites by rendering fresh from `packages/skill`, never reads back or
 * merges what's already there. Hand edits to NodeSpaceAI/nodespace-skill do
 * not survive the next release.
 *
 * Alongside the skill content, this also renders and pushes
 * `.claude-plugin/marketplace.json` (see `renderMarketplaceFile`) so
 * `/plugin marketplace add NodeSpaceAI/nodespace-skill` +
 * `/plugin install nodespace@nodespace-skill` works in Claude Code without
 * a clone-and-copy detour. That mechanism is Claude Code-specific: Codex and
 * Antigravity CLI have their own native plugin mechanisms with different,
 * incompatible manifest formats (`.codex-plugin/plugin.json` and Antigravity's
 * own `plugin.json` -- installed under `~/.gemini/antigravity-cli/plugins/`,
 * not a marketplace-style registry with its own add/install commands), and
 * OpenCode has no first-party marketplace concept at all (its plugins are npm
 * packages or local files referenced from `opencode.json`, with only
 * third-party community tooling layered on top). None of those are addressed
 * here -- clone-and-copy stays the documented install path for all three.
 *
 * Usage:
 *   bun run scripts/publish-skill-repo.ts <version>            # dry run -- prints
 *                                                                # the rendered files, pushes nothing
 *   bun run scripts/publish-skill-repo.ts <version> --push      # pushes to nodespace-skill's
 *                                                                # main branch (requires
 *                                                                # SKILL_REPO_TOKEN)
 *
 * `--push` requires SKILL_REPO_TOKEN: a PAT (or fine-grained token) with
 * `contents: write` on NodeSpaceAI/nodespace-skill, set as a repo secret --
 * the same shape as HOMEBREW_TAP_TOKEN and WEBSITE_DEPLOY_TOKEN, one
 * dedicated token per external target repo rather than one shared credential
 * (a token scoped to nodespace-skill cannot be used to push anywhere else if
 * it ever leaks). `secrets.GITHUB_TOKEN` is scoped to nodespace-core only
 * and cannot push cross-repo.
 *
 * Where this hooks in: release.yml's `sync-skill-repo` job runs this script
 * with `--push` on the `release` event (the same `published`-only trigger
 * the Homebrew tap sync uses, so a release saved as a draft and published
 * later fires this exactly once).
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { AGENTS, buildSkillFrontmatter, SHARED_SKILL_FRONTMATTER } from "../packages/skill/src/agents";
import { pushFilesToRepo, type RepoFile } from "./push-to-external-repo";

export const SKILL_REPO = "NodeSpaceAI/nodespace-skill";

const REPO_ROOT = join(dirname(new URL(import.meta.url).pathname), "..");
const SKILL_DIR = join(REPO_ROOT, "packages", "skill");

export function normalizeVersion(version: string): string {
  return version.replace(/^v/, "");
}

/** The harness-agnostic files every installer target ships -- the
 * intersection of every agent's `shims` list in packages/skill/src/agents.ts
 * (today: SKILL.md, references/cli.md, references/shared-workspaces.md, and
 * references/graph-authored-guidance.md). Harness-specific shims (the
 * `shims/claude-code/nodespace-hook.ts` family) are deliberately excluded:
 * they're per-harness integration glue the installer places into each
 * agent's own hook/plugin system, not part of a generic Agent Skills folder
 * a user can drop into any of them.
 *
 * Derived from AGENTS rather than hardcoded, on purpose: this is the same
 * "several separate places enumerate what the skill is made of" drift class
 * `packages/skill/src/tests/installer.test.ts` guards against for
 * build-skill.ts, the installer, and the PTY path -- a hardcoded list
 * here would be exactly the kind of copy that silently stops matching AGENTS
 * if a shared reference is ever added or removed. */
export function sharedShimPaths(): string[] {
  const [first, ...rest] = AGENTS.map((a) => new Set(a.shims));
  return [...first].filter((path) => rest.every((shims) => shims.has(path)));
}

/** Reads packages/skill's current build inputs from disk -- never a cached
 * or previously-rendered copy, so this always reflects whatever
 * `packages/skill` produces *right now*, drift-free by construction. */
export function readSkillSource(): {
  body: string;
  referenceCli: string;
  referenceSharedWorkspaces: string;
  referenceGraphAuthoredGuidance: string;
} {
  return {
    body: readFileSync(join(SKILL_DIR, "SKILL.md"), "utf8"),
    referenceCli: readFileSync(join(SKILL_DIR, "references", "cli.md"), "utf8"),
    referenceSharedWorkspaces: readFileSync(
      join(SKILL_DIR, "references", "shared-workspaces.md"),
      "utf8",
    ),
    referenceGraphAuthoredGuidance: readFileSync(
      join(SKILL_DIR, "references", "graph-authored-guidance.md"),
      "utf8",
    ),
  };
}

/** Renders every shared file this script publishes -- the SKILL.md
 * frontmatter is generated here (`compatibility` needs the release version,
 * which `packages/skill`'s own build doesn't know at compile time); every
 * other shared file (currently `references/cli.md`,
 * `references/shared-workspaces.md`, and
 * `references/graph-authored-guidance.md`) is copied through unmodified. */
export function renderPublishFiles(version: string): RepoFile[] {
  const v = normalizeVersion(version);
  const frontmatter = buildSkillFrontmatter({
    compatibility: `Targets NodeSpace app v${v}. Requires either a shell with the \`nodespace\` CLI on $PATH, or an MCP client connected to \`nodespace mcp\` (its bash-less passthrough) -- see Preflight Check in SKILL.md.`,
  });

  return sharedShimPaths().map((relSrcPath) => {
    const content = readFileSync(join(SKILL_DIR, relSrcPath), "utf8");
    const isSkillMd = relSrcPath === "SKILL.md";
    return {
      relPath: `skills/nodespace/${relSrcPath}`,
      content: isSkillMd ? frontmatter + "\n" + content : content,
    };
  });
}

/** Pulls `name` and a plain-text `description` out of the shared skill
 * frontmatter block (`SHARED_SKILL_FRONTMATTER`, packages/skill/src/agents.ts)
 * -- the same block every installer target's SKILL.md ships, and the source
 * this script already trusts for the published SKILL.md itself. The
 * marketplace manifest's plugin identity is read from here rather than
 * typed in a second time, so it cannot drift from what the skill declares
 * about itself.
 *
 * Written as a small extractor targeted at this file's own two-field shape
 * (a scalar `name:` line, a folded `description: >` block) rather than a
 * general YAML parser: this script fully owns SHARED_SKILL_FRONTMATTER, so
 * there is no external schema whose future shape a real parser would be
 * guarding against here -- only this file's own two fields, which the tests
 * pin. */
export function extractSkillMeta(frontmatter: string): { name: string; description: string } {
  const nameMatch = /^name:\s*(.+)$/m.exec(frontmatter);
  if (!nameMatch) {
    throw new Error("shared skill frontmatter is missing a `name:` field");
  }

  const descMatch = /^description:\s*>\s*\n((?:^ {2}.*\n?)+)/m.exec(frontmatter);
  if (!descMatch) {
    throw new Error("shared skill frontmatter is missing a folded `description:` block");
  }
  // Replicates YAML's `>` folded-scalar rule -- join single newlines with a
  // space -- well enough for this specific two-space-indented block. The
  // manifest field this feeds is a plain string, not YAML, so the folded
  // form (not the raw indented block) is what belongs there.
  const description = descMatch[1]
    .split("\n")
    .map((line) => line.replace(/^ {2}/, ""))
    .filter((line) => line.length > 0)
    .join(" ")
    .trim();

  return { name: nameMatch[1].trim(), description };
}

/** Renders `.claude-plugin/marketplace.json` -- the file Claude Code's
 * `/plugin marketplace add NodeSpaceAI/nodespace-skill` reads. Shape
 * verified against the real docs (https://code.claude.com/docs/en/plugin-marketplaces,
 * https://code.claude.com/docs/en/plugins-reference) and a real, currently
 * live example (anthropics/claude-code's own
 * `.claude-plugin/marketplace.json`), not guessed: a marketplace needs
 * `name` + `owner` + `plugins[]`, and each plugin entry's own
 * `name`/`description`/`version`/`license`/`repository` fields are
 * sufficient metadata on their own -- no separate
 * `skills/nodespace/.claude-plugin/plugin.json` is required for a plugin
 * that only uses the default `skills/` directory convention, which
 * `skills/nodespace/SKILL.md` (see `renderPublishFiles`) already satisfies.
 *
 * `source: "./"` points the plugin root at the marketplace root -- this
 * repo's own root -- rather than a `./plugins/<name>` subdirectory, because
 * this repo publishes exactly one plugin and already places
 * `skills/nodespace/` at the root for the clone-and-copy path. No `skills`
 * field is declared on the plugin entry, which matters: for a marketplace
 * entry whose `source` resolves to the marketplace root, an *explicit*
 * `skills` field replaces the default `skills/` scan instead of adding to
 * it. Leaving it undeclared keeps that default scan active, so
 * `skills/nodespace/SKILL.md` is discovered with no extra configuration.
 *
 * Every identifying field (name, description, version, license,
 * repository) is derived rather than typed in here a second time: name and
 * description from `extractSkillMeta(SHARED_SKILL_FRONTMATTER)` (the same
 * source `renderPublishFiles` trusts for the published SKILL.md), version
 * from the release argument this script always takes, license from
 * `packages/skill/package.json` (the skill's own package manifest), and the
 * marketplace's own `name`/`owner` from `SKILL_REPO` (already this script's
 * single source of truth for which repo it publishes to). */
export function renderMarketplaceFile(version: string): RepoFile {
  const v = normalizeVersion(version);
  const { name, description } = extractSkillMeta(SHARED_SKILL_FRONTMATTER);
  const [ownerName, marketplaceName] = SKILL_REPO.split("/");
  const { license } = JSON.parse(
    readFileSync(join(SKILL_DIR, "package.json"), "utf8"),
  ) as { license: string };

  const manifest = {
    $schema: "https://json.schemastore.org/claude-code-marketplace.json",
    name: marketplaceName,
    owner: { name: ownerName, url: `https://github.com/${ownerName}` },
    description,
    version: v,
    plugins: [
      {
        name,
        description,
        version: v,
        license,
        repository: `https://github.com/${SKILL_REPO}`,
        source: "./",
      },
    ],
  };

  return {
    relPath: ".claude-plugin/marketplace.json",
    content: `${JSON.stringify(manifest, null, 2)}\n`,
  };
}

async function pushSkillUpdate(version: string, files: RepoFile[], token: string): Promise<void> {
  const v = normalizeVersion(version);
  const pushed = await pushFilesToRepo(
    SKILL_REPO,
    files,
    `Publish skill for v${v} (automated release sync)`,
    token,
  );
  console.log(
    pushed
      ? `Pushed skill update for v${v} to ${SKILL_REPO}.`
      : `${SKILL_REPO} already matches -- nothing to push.`,
  );
}

function usage(): void {
  console.log(`Usage:
  bun run scripts/publish-skill-repo.ts <version> [--push]`);
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command || !/^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$/.test(command)) {
    usage();
    process.exit(1);
  }

  const push = args.includes("--push");
  const token = process.env.SKILL_REPO_TOKEN;
  if (push && !token) {
    console.error(
      "SKILL_REPO_TOKEN is not set -- required for --push (a PAT with contents:write on " +
        `${SKILL_REPO}). Running without --push shows what would change.`,
    );
    process.exit(1);
  }

  const files = [...renderPublishFiles(command), renderMarketplaceFile(command)];
  for (const file of files) {
    console.log(`--- ${file.relPath} ---`);
    console.log(file.content);
  }

  if (!push) {
    console.log("(dry run -- pass --push with SKILL_REPO_TOKEN set to publish this)");
    return;
  }
  await pushSkillUpdate(command, files, token as string);
}

if (import.meta.main) {
  // Matches scripts/test-gate.ts's convention: a bare uncaught rejection
  // here would otherwise print a raw Bun stack trace / ShellError dump
  // instead of an operator-facing message.
  try {
    await main();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`✗ ${message}`);
    process.exit(1);
  }
}
