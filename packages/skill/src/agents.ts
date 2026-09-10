import { homedir } from 'node:os';
import { join } from 'node:path';
import type { AgentConfig } from './types.js';

const home = homedir();

// Claude Code reads its config from $CLAUDE_CONFIG_DIR when set (e.g. a separate
// `claude-ns` profile), falling back to ~/.claude. Detect + install into the
// active profile rather than always ~/.claude so the skill lands where the
// running Claude Code will actually look for it.
const claudeConfigDir = process.env.CLAUDE_CONFIG_DIR?.trim() || join(home, '.claude');

/**
 * YAML frontmatter prepended to `SKILL.md` at install time.
 *
 * Shared by every target, not Claude-Code-specific. Under the Agent Skills
 * standard a skill is discovered by its frontmatter `name` + `description`, and
 * a folder without them is not a valid skill anywhere — so the three non-Claude
 * targets were previously installing a spec-invalid skill.
 *
 * Only fields the standard defines appear here (`name`, `description`,
 * `license`, `compatibility`, `metadata`, `allowed-tools`). Claude Code accepts
 * additional keys such as `argument-hint` and `user-invocable`, but other
 * distribution paths hard-error on keys they don't recognize, so harness-specific
 * fields must not go in the shared block.
 *
 * `name` must match the parent directory the skill installs into
 * (`skills/nodespace/` → `name: nodespace`).
 *
 * The `description` is the entire discovery surface: under progressive
 * disclosure an agent loads only `name` + `description` at startup and reads the
 * body *after* deciding the skill is relevant. So its job is to match how a user
 * actually phrases the request — which is why it is hand-tuned and deliberately
 * excluded from generation. There is no upstream source to render it from, and
 * generating it would produce worse text than tuning it.
 *
 * It leads with what NodeSpace is rather than with personal-memory phrasing
 * ("remember this for later"), because the product's own framing is context
 * infrastructure for AI-native development: the repository holds *what* was
 * built, NodeSpace holds *why* it was built and how it should be built. An agent
 * told that needs far less separate instruction to check NodeSpace before
 * writing a spec or an ADR.
 *
 * Max 1024 characters per the spec.
 */
const SKILL_FRONTMATTER = `---
name: nodespace
description: >
  Context infrastructure for AI-native development. Read and write the
  NodeSpace knowledge graph — the durable record of why a system was built
  and how it should be built: specs, architecture decisions, ADRs, designs,
  plans, standards, tasks, and findings. Use before writing or changing a
  spec, ADR, design doc, or plan; when you need the reasoning or constraints
  behind existing code; when recording a decision or discovery that should
  outlive this session; or when asked to "check nodespace".
allowed-tools: Bash(nodespace:*)
---
`;

export const AGENTS: AgentConfig[] = [
  {
    name: 'claude-code',
    detectionDir: claudeConfigDir,
    installDir: join(claudeConfigDir, 'skills', 'nodespace'),
    shims: [
      'SKILL.md',
      'references/cli.md',
      'references/shared-workspaces.md',
      'references/graph-authored-guidance.md',
      'shims/claude-code/nodespace-hook.ts',
    ],
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'codex',
    detectionDir: join(home, '.codex'),
    installDir: join(home, '.codex', 'skills', 'nodespace'),
    shims: [
      'SKILL.md',
      'references/cli.md',
      'references/shared-workspaces.md',
      'references/graph-authored-guidance.md',
      'shims/codex/nodespace-plugin.ts',
    ],
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'antigravity',
    // Antigravity CLI stores its config, plugins, and skills under the
    // Gemini config dir (a holdover from its Gemini CLI lineage) —
    // ~/.gemini/antigravity-cli/, not a separate ~/.antigravity.
    detectionDir: join(home, '.gemini', 'antigravity-cli'),
    installDir: join(home, '.gemini', 'antigravity-cli', 'skills', 'nodespace'),
    // No handler shim: Antigravity CLI is a shell-capable coding agent, same
    // as Claude Code/Codex/OpenCode, so it just runs `nodespace` directly per
    // SKILL.md's Preflight "Branch 1" — no tool-registration integration
    // (MCP or otherwise) needed for it to use the CLI.
    shims: ['SKILL.md', 'references/cli.md', 'references/shared-workspaces.md', 'references/graph-authored-guidance.md'],
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'opencode',
    detectionDir: join(home, '.opencode'),
    installDir: join(home, '.opencode', 'skills', 'nodespace'),
    shims: [
      'SKILL.md',
      'references/cli.md',
      'references/shared-workspaces.md',
      'references/graph-authored-guidance.md',
      'shims/opencode/nodespace-plugin.ts',
    ],
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'pi',
    // Pi (pi.dev / earendil-works/pi) stores its global config, skills, and
    // extensions under ~/.pi/agent/ -- not a bare ~/.pi -- per its skills.md
    // and extensions.md docs (skill dirs: ~/.pi/agent/skills/, ~/.agents/skills/;
    // extension dirs: ~/.pi/agent/extensions/*.ts, ~/.pi/agent/extensions/*/index.ts).
    detectionDir: join(home, '.pi', 'agent'),
    installDir: join(home, '.pi', 'agent', 'skills', 'nodespace'),
    shims: [
      'SKILL.md',
      'references/cli.md',
      'references/shared-workspaces.md',
      'references/graph-authored-guidance.md',
      'shims/pi/nodespace-extension.ts',
    ],
    skillFrontmatter: SKILL_FRONTMATTER,
  },
];

/** The frontmatter block every target installs. Exported for tests. */
export const SHARED_SKILL_FRONTMATTER = SKILL_FRONTMATTER;

/**
 * Builds the shared frontmatter block, optionally stamping the optional
 * `compatibility` field (max 500 chars per the spec) in before the closing
 * `---`.
 *
 * The installer path above never needs this — the app installs whatever
 * `packages/skill` was built with, and SHARED_SKILL_FRONTMATTER already
 * covers it — so this stays a separate, additive export rather than a
 * change to AGENTS. `scripts/publish-skill-repo.ts` is the one caller: it
 * passes the released NodeSpace app version (from the same release tag
 * `scripts/update-homebrew-cask.ts` takes, itself derived from the
 * `tauri.conf.json` canonical version `scripts/check-version-sync.ts`
 * enforces) so a user importing the published skill can tell which app
 * version a given revision targets.
 */
export function buildSkillFrontmatter(opts: { compatibility?: string } = {}): string {
  const { compatibility } = opts;
  if (!compatibility) return SKILL_FRONTMATTER;
  if (compatibility.length > 500) {
    throw new Error(
      `compatibility exceeds the Agent Skills spec's 500-character limit ` +
        `(${compatibility.length}): ${compatibility}`,
    );
  }
  // Insert as the last field, right before the closing `---`, so every
  // other field (and its ordering) stays identical to SHARED_SKILL_FRONTMATTER.
  return SKILL_FRONTMATTER.replace(/\n---\n$/, `\ncompatibility: ${compatibility}\n---\n`);
}
