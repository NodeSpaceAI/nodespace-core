export type AgentName = 'claude-code' | 'codex' | 'antigravity' | 'opencode' | 'pi';

export interface AgentConfig {
  name: AgentName;
  detectionDir: string;
  installDir: string;
  /**
   * The harness plugin installed beside `SKILL.md`, for a harness that loads
   * one from its skill folder (ADR-093 §5): `dir` is its folder under the
   * package root, and each of `files`, a path relative to `dir`, installs at
   * that same path inside `installDir`.
   *
   * `SKILL.md` and the skill's `references/*.md` files are not listed
   * anywhere: every agent gets `SKILL.md`, and the installer copies every
   * reference it finds in the package root's `references/` directory
   * (`listReferenceFiles`), so adding or dropping a reference is a change to
   * that directory alone.
   */
  plugin?: { dir: string; files: string[] };
  /**
   * Frontmatter to prepend to `SKILL.md` when installing for this agent.
   *
   * The Agent Skills standard discovers a skill by its YAML `name` +
   * `description`, and the checked-in SKILL.md body carries none so that the
   * file stays a plain body with no harness assumptions baked in. The installer
   * writes `frontmatter + body`.
   *
   * Every target supplies this today — a skill folder without frontmatter is
   * not a valid skill under the standard. It stays optional only so a future
   * target that genuinely needs a different block, or none, can say so.
   */
  skillFrontmatter?: string;
}

export interface InstallResult {
  agent: AgentName;
  /** Every file of the skill now in place, whether or not this run wrote it. */
  installed: string[];
  /**
   * Whether this run altered the install directory: wrote a file whose
   * content differed or was missing, or removed one the skill no longer
   * ships. False for a re-run over an install that is already current, and
   * for an agent nothing was installed for.
   */
  changed: boolean;
  /**
   * Set when `installed` is empty because Claude Code already has the
   * skill via its own plugin marketplace
   * (`/plugin install nodespace@...`, tracked in `installed_plugins.json`).
   * The marketplace copy stays authoritative — this installer does not
   * overwrite it, since the marketplace owns its own update cadence.
   */
  skipReason?: 'plugin-managed';
}

export interface UninstallResult {
  agent: AgentName;
  removed: string[];
}
