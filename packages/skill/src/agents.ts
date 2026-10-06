import { homedir } from 'node:os';
import { join } from 'node:path';
import type { AgentConfig } from './types.js';

const home = homedir();

// Claude Code reads its config from $CLAUDE_CONFIG_DIR when set (e.g. a separate
// `claude-ns` profile), falling back to ~/.claude. Detect + install into the
// active profile rather than always ~/.claude so the skill lands where the
// running Claude Code will actually look for it.
const claudeConfigDir = process.env.CLAUDE_CONFIG_DIR?.trim() || join(home, '.claude');

/** A path from the environment, with a leading `~` read as the home directory. */
function fromEnv(name: string): string | undefined {
  const value = process.env[name]?.trim();
  if (!value) return undefined;
  return value === '~' || value.startsWith('~/') ? join(home, value.slice(1)) : value;
}

// Each harness says where its own files live, and each lets the environment
// move that: Codex's home with `CODEX_HOME`, Pi's agent directory with
// `PI_CODING_AGENT_DIR`, and OpenCode's configuration with `XDG_CONFIG_HOME`.
const codexHome = fromEnv('CODEX_HOME') ?? join(home, '.codex');
const piAgentDir = fromEnv('PI_CODING_AGENT_DIR') ?? join(home, '.pi', 'agent');
// OpenCode keeps its configuration, plugins and skills in its XDG config
// directory. `~/.opencode` holds only what its installer puts there.
const opencodeConfigDir = join(fromEnv('XDG_CONFIG_HOME') ?? join(home, '.config'), 'opencode');
// Antigravity reads its rules, for every project on the machine, from the
// configuration directory it shares across its surfaces, not from the
// command-line tool's own folder.
const antigravityConfigDir = join(home, '.gemini', 'config');

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

/**
 * The Claude Code plugin (ADR-093 §5): a manifest, a hooks file, one hooks
 * module and the type contract for the session state the module keeps. Claude
 * Code loads a plugin it finds in a skill folder, so these install beside
 * `SKILL.md` and need no flag. The plugin's tests are not installed.
 */
export const CLAUDE_CODE_PLUGIN = {
  dir: 'plugins/claude-code',
  files: [
    '.claude-plugin/plugin.json',
    'hooks/hooks.json',
    'hooks/register.ts',
    'types/index.d.ts',
  ],
};

/**
 * The folder under the package root holding what the Pi extension and the
 * OpenCode plugin both import. Each installs its files beside its own.
 */
export const SHARED_PLUGIN_DIR = 'plugins/shared';

/** What both do, behind the few things a harness supplies (ADR-093 §5). */
const SESSION_MODULE = 'nodespace-session.ts';

/**
 * The Pi extension: Pi loads `extensions/<name>/index.ts` from its agent
 * directory with no flag, and that file may import the ones beside it.
 */
export const PI_PLUGIN = {
  dir: 'plugins/pi',
  files: ['index.ts'],
  shared: [[SESSION_MODULE, SESSION_MODULE]] as Array<[string, string]>,
  installDir: join(piAgentDir, 'extensions', 'nodespace'),
};

/**
 * The OpenCode plugin: OpenCode loads every `.ts` file directly in its
 * `plugins/` folder and calls each export of one as a plugin. The module the
 * plugin imports therefore sits one folder down, where OpenCode does not look.
 */
export const OPENCODE_PLUGIN = {
  dir: 'plugins/opencode',
  files: ['nodespace.ts'],
  shared: [[SESSION_MODULE, `nodespace/${SESSION_MODULE}`]] as Array<[string, string]>,
  installDir: join(opencodeConfigDir, 'plugins'),
};

export const AGENTS: AgentConfig[] = [
  {
    name: 'claude-code',
    detectionDir: claudeConfigDir,
    installDir: join(claudeConfigDir, 'skills', 'nodespace'),
    plugin: CLAUDE_CODE_PLUGIN,
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'codex',
    detectionDir: codexHome,
    installDir: join(codexHome, 'skills', 'nodespace'),
    // Codex reads `AGENTS.md` in its home into every session. An
    // `AGENTS.override.md` beside it, when the user keeps one, is read instead.
    instructionsFile: join(codexHome, 'AGENTS.md'),
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'antigravity',
    // ~/.gemini/antigravity-cli/ is the CLI's own data folder (state, logs,
    // its built-in skills), so its presence is what shows the CLI is
    // installed. Skills for every project are read from the shared config
    // directory, `~/.gemini/config/skills/<name>/`, which Antigravity's
    // documentation names (and where `skills.json` is not needed). Checked against Antigravity CLI 1.3.0: a skill in
    // that folder is listed to the agent with no `skills.json` entry, and one
    // in `antigravity-cli/skills/` is not.
    detectionDir: join(home, '.gemini', 'antigravity-cli'),
    installDir: join(antigravityConfigDir, 'skills', 'nodespace'),
    pruneRoot: antigravityConfigDir,
    instructionsFile: join(antigravityConfigDir, 'AGENTS.md'),
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'opencode',
    detectionDir: opencodeConfigDir,
    // OpenCode also reads the Claude skills folder. It keeps one skill per
    // `name`, so the skill installed in both is listed to the agent once.
    installDir: join(opencodeConfigDir, 'skills', 'nodespace'),
    plugin: OPENCODE_PLUGIN,
    skillFrontmatter: SKILL_FRONTMATTER,
  },
  {
    name: 'pi',
    // Pi (pi.dev / earendil-works/pi) stores its global config, skills, and
    // extensions under ~/.pi/agent/ -- not a bare ~/.pi -- per its skills.md
    // and extensions.md docs (skill dirs: ~/.pi/agent/skills/, ~/.agents/skills/;
    // extension dirs: ~/.pi/agent/extensions/*.ts, ~/.pi/agent/extensions/*/index.ts).
    detectionDir: piAgentDir,
    installDir: join(piAgentDir, 'skills', 'nodespace'),
    plugin: PI_PLUGIN,
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
