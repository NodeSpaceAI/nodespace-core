#!/usr/bin/env bun
/**
 * Writes `packages/skill/SKILL.md` to a destination with the minimum CLI
 * version stamped in (the workspace version), for packaging steps that copy the
 * skill outside `build-skill.ts` -- the release tarball of the skill-installer
 * resources. Usage: `bun run scripts/stamp-skill-md.ts <dest>`.
 */
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { stampMinCliVersion, workspaceVersion } from "../packages/skill/src/min-cli-version";

const REPO_ROOT = join(import.meta.dir, "..");

export function stampedSkillMd(repoRoot: string = REPO_ROOT): string {
  return stampMinCliVersion(
    readFileSync(join(repoRoot, "packages", "skill", "SKILL.md"), "utf8"),
    workspaceVersion(repoRoot),
  );
}

if (import.meta.main) {
  const dest = process.argv[2];
  if (!dest) {
    console.error("usage: stamp-skill-md.ts <dest>");
    process.exit(2);
  }
  mkdirSync(dirname(resolve(dest)), { recursive: true });
  writeFileSync(dest, stampedSkillMd());
}
