// Covers the offline, deterministic part of scripts/verify-pkg-gatekeeper.ts:
// picking the .pkg filename out of a directory listing. The `gh`/`spctl`-
// talking functions (downloadPublishedPkg, assessGatekeeperInstall) are
// intentionally not exercised here -- this suite runs as part of
// `bun run test:scripts` / `test:all` (the pre-push gate), which must stay
// fast and deterministic, not depend on network, `gh` auth, or macOS-only
// tools (same reasoning as update-homebrew-cask.test.ts).
import { describe, expect, test } from "bun:test";
import { pickPkgFilename } from "./verify-pkg-gatekeeper";

describe("pickPkgFilename", () => {
  test("picks the .pkg entry out of a mixed directory listing", () => {
    expect(
      pickPkgFilename(["SHA256SUMS", "NodeSpace_0.3.0_aarch64-apple-darwin.pkg", "NodeSpace_0.3.0_aarch64.dmg"]),
    ).toBe("NodeSpace_0.3.0_aarch64-apple-darwin.pkg");
  });

  test("trims surrounding whitespace from `ls`-style output", () => {
    expect(pickPkgFilename(["  NodeSpace_0.3.0_aarch64-apple-darwin.pkg  "])).toBe(
      "NodeSpace_0.3.0_aarch64-apple-darwin.pkg",
    );
  });

  test("returns undefined when no .pkg entry is present", () => {
    expect(pickPkgFilename(["SHA256SUMS", "NodeSpace_0.3.0_aarch64.dmg", ""])).toBeUndefined();
  });

  test("returns undefined for an empty listing", () => {
    expect(pickPkgFilename([])).toBeUndefined();
  });

  test("ignores a file that merely contains .pkg mid-name, not as the extension", () => {
    expect(pickPkgFilename(["NodeSpace.pkg.sha256", "notes.txt"])).toBeUndefined();
  });

  test("picks the first .pkg entry when more than one is present", () => {
    expect(pickPkgFilename(["a.pkg", "b.pkg"])).toBe("a.pkg");
  });
});
