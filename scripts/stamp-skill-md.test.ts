import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { $ } from "bun";
import { workspaceVersion } from "../packages/skill/src/min-cli-version";

test("writes SKILL.md with the workspace version stamped and no placeholder", async () => {
  const dir = mkdtempSync(join(tmpdir(), "stamp-skill-"));
  try {
    const dest = join(dir, "out", "SKILL.md");
    await $`bun run ${join(import.meta.dir, "stamp-skill-md.ts")} ${dest}`.quiet();
    const text = readFileSync(dest, "utf8");
    expect(text).not.toContain("{{MIN_CLI_VERSION}}");
    expect(text).toContain(`version **${workspaceVersion(join(import.meta.dir, ".."))} or newer**`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
