export type AgentName = 'claude-code' | 'codex' | 'antigravity' | 'opencode' | 'pi';

export interface AgentConfig {
  name: AgentName;
  detectionDir: string;
  /**
   * The command that starts the harness. A harness that has been installed
   * and never started has made no configuration folder yet, so one found on
   * `$PATH` counts as present as well.
   */
  command: string;
  installDir: string;
  /**
   * The directory uninstall stops pruning empty folders at, never removing it,
   * for a harness whose skills folder lies outside `detectionDir`. It defaults
   * to `detectionDir`.
   */
  pruneRoot?: string;
  /**
   * The harness plugin (ADR-093 §5): `dir` is its folder under the package
   * root, and each of `files`, a path relative to `dir`, installs at that
   * same path. `shared` names files of `SHARED_PLUGIN_DIR` the plugin
   * imports, each as `[file, path it installs at]`.
   *
   * `installDir` is the folder the harness loads the plugin from. Without
   * one the plugin installs into the skill folder, beside `SKILL.md`, for a
   * harness that loads a plugin from there.
   *
   * `SKILL.md` and the skill's `references/*.md` files are not listed
   * anywhere: every agent gets `SKILL.md`, and the installer copies every
   * reference it finds in the package root's `references/` directory
   * (`listReferenceFiles`), so adding or dropping a reference is a change to
   * that directory alone.
   */
  plugin?: {
    dir: string;
    files: string[];
    shared?: Array<[string, string]>;
    installDir?: string;
    /**
     * For a harness that reads hook commands from a JSON file of its own: the
     * file, and the events to register `hookFile` (a path relative to the
     * folder the plugin installs into) for. The installer owns those handlers
     * alone, found by the command's path, and leaves every other entry as it is.
     */
    hooks?: { file: string; command: string; events: Array<{ event: string; timeoutSeconds: number }> };
  };
  /**
   * The harness's user-level instructions file, for a harness with no plugin
   * (ADR-093 §6). The installer writes one marked block into it, creating the
   * file when it is absent, and owns nothing else in it.
   */
  instructionsFile?: string;
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
  /**
   * Every file of the skill now in place, whether or not this run wrote it.
   * The instructions file is among them for a harness that gets the block.
   */
  installed: string[];
  /**
   * Whether this run altered anything on disk: wrote a file whose content
   * differed or was missing, removed one the skill no longer ships, or
   * wrote or replaced the instructions block. False for a re-run over an install that is already current, and
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
  /**
   * Set when the harness runs the hooks this install registered only after
   * the user has trusted them: the install cannot do that step, so the caller
   * says what the user has to do.
   */
  needsHookTrust?: boolean;
}

export interface UninstallResult {
  agent: AgentName;
  removed: string[];
}

/** What a harness has beyond the static skill (ADR-093 §5, §6), and whether it is in place. */
export interface IntegrationStatus {
  kind: 'plugin' | 'instructions-block';
  installed: boolean;
}
