// The minimum `nodespace` CLI version SKILL.md asks for.
//
// SKILL.md carries a placeholder where the minimum goes, and the build that
// stages or publishes the skill replaces it with the workspace version the
// skill is built from. The skill and the CLI it needs therefore come from one
// revision and cannot drift. The CLI is versioned by the workspace (its
// manifest inherits `version.workspace`), so the workspace version is the CLI
// version that ships with this skill.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/** Where SKILL.md names the minimum CLI version. */
export const MIN_CLI_VERSION_PLACEHOLDER = '{{MIN_CLI_VERSION}}';

type Triple = readonly [number, number, number];

/** `major.minor.patch` as numbers (a `v` prefix and `-pre`/`+build` suffix are ignored); undefined when it is not that shape. */
export function parseVersion(version: string): Triple | undefined {
  const match = /^v?(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(version.trim());
  return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : undefined;
}

/** True when a CLI at `cli` meets a skill that needs `minimum`. Unparseable versions never satisfy. */
export function cliSatisfies(cli: string, minimum: string): boolean {
  const have = parseVersion(cli);
  const need = parseVersion(minimum);
  if (!have || !need) return false;
  for (let i = 0; i < 3; i++) {
    if (have[i] !== need[i]) return have[i]! > need[i]!;
  }
  return true;
}

/** The workspace version in `repoRoot`'s Cargo.toml (`[workspace.package]`). */
export function workspaceVersion(repoRoot: string): string {
  const manifest = readFileSync(join(repoRoot, 'Cargo.toml'), 'utf8');
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(manifest);
  const version = section && /^version\s*=\s*"([^"]+)"/m.exec(section[1]!);
  if (!version || !parseVersion(version[1]!)) {
    throw new Error('Cargo.toml has no [workspace.package] version in major.minor.patch form');
  }
  return version[1]!;
}

/** `skill` with every placeholder replaced by `version`. Throws when there is none to replace or the version is malformed. */
export function stampMinCliVersion(skill: string, version: string): string {
  if (!parseVersion(version)) throw new Error(`cannot stamp '${version}' as a minimum CLI version`);
  if (!skill.includes(MIN_CLI_VERSION_PLACEHOLDER)) {
    throw new Error(`SKILL.md has no ${MIN_CLI_VERSION_PLACEHOLDER} to stamp`);
  }
  return skill.split(MIN_CLI_VERSION_PLACEHOLDER).join(version.trim().replace(/^v/, ''));
}
