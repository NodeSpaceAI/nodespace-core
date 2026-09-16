/**
 * Display names for the AI coding agents the NodeSpace skill installs into.
 *
 * The ids are the installer's own agent names (`packages/skill/src/agents.ts`'s
 * `AGENTS[].name`), which arrive verbatim from every backend surface that
 * reports agents: `detect_agents`'s `agents`, and `SkillSetupResult`'s
 * `agentsInstalled` / `agentsSkipped`.
 *
 * Shared rather than duplicated per component so the onboarding wizard's
 * pre-install question and its post-install confirmation cannot drift apart —
 * the two disagreeing (the question naming only Claude Code while the
 * confirmation correctly listed several agents) is exactly what this module
 * exists to prevent.
 */

const DISPLAY_NAMES: Record<string, string> = {
  'claude-code': 'Claude Code',
  codex: 'Codex',
  antigravity: 'Antigravity CLI',
  opencode: 'OpenCode',
  pi: 'Pi',
};

/** "Claude Code" from "claude-code", "OpenCode" from "opencode", etc. */
export function displayAgentName(agent: string): string {
  return DISPLAY_NAMES[agent] ?? agent;
}

/**
 * A natural-language list of agent display names, for prose:
 * `[]` → `''`, one → `'Claude Code'`, two → `'Claude Code and Codex'`,
 * three+ → `'Claude Code, Codex, and Antigravity CLI'` (serial comma).
 *
 * Callers are responsible for the empty case — an empty list yields an empty
 * string rather than a placeholder, since the sentence around it generally
 * needs to change shape entirely when there is nothing to name.
 */
export function formatAgentList(agents: string[]): string {
  const names = agents.map(displayAgentName);
  if (names.length === 0) return '';
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.slice(0, -1).join(', ')}, and ${names[names.length - 1]}`;
}
