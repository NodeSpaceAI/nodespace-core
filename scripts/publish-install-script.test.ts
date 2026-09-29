// Covers the offline, deterministic parts of scripts/publish-install-script.ts:
// the version-pin string transform and tag normalization. The
// GitHub-talking functions (checkReleaseAssets, fetchWebsiteInstallScript,
// pushInstallScriptUpdate) are intentionally not exercised here -- this
// suite runs as part of `bun run test:scripts` / `test:all` (the pre-push
// gate), which must stay fast and deterministic, not depend on network or
// `gh` auth. See scripts/update-homebrew-cask.test.ts for the same pattern
// applied to the sibling cask-sync script.
import { describe, expect, test } from "bun:test";
import { classifyInstallScriptDrift, extractPin, formatInstallScriptDrift, normalizeTag, pinVersion } from "./publish-install-script";

describe("normalizeTag", () => {
  test("adds a leading v when missing", () => {
    expect(normalizeTag("0.2.0")).toBe("v0.2.0");
  });

  test("leaves an already-prefixed tag untouched", () => {
    expect(normalizeTag("v0.2.0")).toBe("v0.2.0");
  });
});

describe("pinVersion", () => {
  const fixture = [
    "#!/bin/sh",
    "set -eu",
    "",
    "NODESPACE_CLI_VERSION=\"v0.1.6\"",
    "",
    'NS_REPO="NodeSpaceAI/nodespace-core"',
    "",
  ].join("\n");

  test("replaces the pin line with a normalized tag", () => {
    const updated = pinVersion(fixture, "0.2.0");
    expect(updated).toContain('NODESPACE_CLI_VERSION="v0.2.0"');
    expect(updated).not.toContain('NODESPACE_CLI_VERSION="v0.1.6"');
  });

  test("accepts a version already carrying a leading v", () => {
    const updated = pinVersion(fixture, "v0.3.0");
    expect(updated).toContain('NODESPACE_CLI_VERSION="v0.3.0"');
  });

  test("leaves every other line untouched", () => {
    const updated = pinVersion(fixture, "0.2.0");
    expect(updated).toContain("#!/bin/sh");
    expect(updated).toContain('NS_REPO="NodeSpaceAI/nodespace-core"');
  });

  test("is idempotent -- pinning to the version already present is a no-op string-wise", () => {
    const once = pinVersion(fixture, "0.1.6");
    expect(once).toBe(fixture);
  });

  test("throws rather than silently no-op-ing when the pin marker is missing", () => {
    const noMarker = "#!/bin/sh\necho hello\n";
    expect(() => pinVersion(noMarker, "0.2.0")).toThrow(/could not find/);
  });

  test("matches the real install.sh pin line shape", () => {
    // Guards against the regex and the actual committed line in
    // nodespace-website's install.sh silently drifting apart.
    const real = 'NODESPACE_CLI_VERSION="v0.2.0"\n';
    const updated = pinVersion(real, "0.3.0");
    expect(updated).toBe('NODESPACE_CLI_VERSION="v0.3.0"\n');
  });
});

describe("extractPin", () => {
  test("returns the pinned tag", () => {
    expect(extractPin('set -eu\nNODESPACE_CLI_VERSION="v0.2.10"\n')).toBe("v0.2.10");
  });

  test("returns null when there is no pin line", () => {
    expect(extractPin("#!/bin/sh\necho hi\n")).toBeNull();
  });
});

describe("classifyInstallScriptDrift", () => {
  test("ok when repo and live both match the latest release (v prefix ignored)", () => {
    expect(classifyInstallScriptDrift("v0.3.2", "v0.3.2", "0.3.2")).toEqual({ kind: "ok" });
  });

  test("repo-stale when the repo pin lags the latest release", () => {
    expect(classifyInstallScriptDrift("v0.2.0", "v0.2.0", "v0.3.2")).toEqual({
      kind: "repo-stale",
      repoPin: "v0.2.0",
      livePin: "v0.2.0",
    });
  });

  test("repo-stale when the repo has no pin line at all", () => {
    expect(classifyInstallScriptDrift(null, "v0.3.2", "v0.3.2").kind).toBe("repo-stale");
  });

  test("deploy-stale when the repo is current but the live site lags", () => {
    expect(classifyInstallScriptDrift("v0.3.2", "v0.2.0", "v0.3.2")).toEqual({
      kind: "deploy-stale",
      repoPin: "v0.3.2",
      livePin: "v0.2.0",
    });
  });

  test("with no repo copy checked (undefined), only the live pin decides", () => {
    expect(classifyInstallScriptDrift(undefined, "v0.3.2", "v0.3.2")).toEqual({ kind: "ok" });
    expect(classifyInstallScriptDrift(undefined, "v0.2.0", "v0.3.2").kind).toBe("deploy-stale");
  });

  test("a live copy with no pin is deploy-stale", () => {
    expect(classifyInstallScriptDrift("v0.3.2", null, "v0.3.2").kind).toBe("deploy-stale");
  });
});

describe("extractPin edge cases", () => {
  test("null when the pin line is duplicated", () => {
    expect(extractPin('NODESPACE_CLI_VERSION="v0.1.0"\nNODESPACE_CLI_VERSION="v0.2.0"\n')).toBeNull();
  });

  test("tolerates trailing whitespace and a v-less pin", () => {
    expect(extractPin('NODESPACE_CLI_VERSION="0.2.0"  \n')).toBe("0.2.0");
  });
});

describe("formatInstallScriptDrift", () => {
  test("repo-stale message leads with PIN DRIFT and names the fix", () => {
    const m = formatInstallScriptDrift({ kind: "repo-stale", repoPin: "v0.2.0", livePin: "v0.2.0" }, "0.3.2", true);
    expect(m.startsWith("INSTALL.SH PIN DRIFT")).toBe(true);
    expect(m).toContain("publish-install-script.ts v0.3.2 --push");
  });

  test("deploy-stale with the repo checked says the repo is fine", () => {
    const m = formatInstallScriptDrift({ kind: "deploy-stale", repoPin: "v0.3.2", livePin: "v0.2.0" }, "v0.3.2", true);
    expect(m.startsWith("INSTALL.SH DEPLOY DRIFT:")).toBe(true);
    expect(m).toContain("pinned correctly");
  });

  test("deploy-stale without a repo check is ambiguous, never claims the repo is fine", () => {
    const m = formatInstallScriptDrift({ kind: "deploy-stale", repoPin: "v0.3.2", livePin: "v0.2.0" }, "v0.3.2", false);
    expect(m).toContain("repo copy not checked");
    expect(m).not.toContain("pinned correctly");
    expect(m).toContain("--push");
  });
});
