// Covers the offline, deterministic parts of scripts/update-homebrew-cask.ts:
// digest hashing, arm64-only cask rendering, and the tap-drift comparison
// predicate. The GitHub-talking functions (fetchReleaseAssets,
// downloadAndHash, checkTapDrift, pushCaskUpdate) are intentionally not
// exercised here -- this suite runs as part of `bun run test:scripts` /
// `test:all` (the pre-push gate), which must stay fast and deterministic,
// not depend on network or `gh` auth.
import { describe, expect, test } from "bun:test";
import {
  type ArchDigestResult,
  isVersionDrifted,
  normalizeVersion,
  renderCask,
  sha256Hex,
} from "./update-homebrew-cask";

describe("normalizeVersion", () => {
  test("strips a leading v", () => {
    expect(normalizeVersion("v0.2.0")).toBe("0.2.0");
  });

  test("leaves a bare version untouched", () => {
    expect(normalizeVersion("0.2.0")).toBe("0.2.0");
  });
});

describe("sha256Hex", () => {
  test("matches `shasum -a 256` for a known input", () => {
    // `printf 'nodespace-test-fixture' | shasum -a 256`
    const bytes = new TextEncoder().encode("nodespace-test-fixture");
    expect(sha256Hex(bytes)).toBe(
      "cd028627062b027682af9676d7b1901b1b4ea0aea9f055f780a74ed3f252ad18",
    );
  });
});

describe("renderCask", () => {
  const armDigest = {
    arch: "arm" as const,
    fileName: "NodeSpace_0.2.0_aarch64.dmg",
    sha256: "a".repeat(64),
  };

  test("renders a single top-level url/sha256 pinned to arm64, with no Intel/x64 trace", () => {
    const digests: ArchDigestResult = { arm: armDigest };
    const cask = renderCask("v0.2.0", digests);

    expect(cask).toContain('version "0.2.0"');
    expect(cask).toContain(`sha256 "${armDigest.sha256}"`);
    expect(cask).toContain("NodeSpace_#{version}_aarch64.dmg");
    expect(cask).toContain("depends_on arch:  :arm64");
    expect(cask).not.toContain("on_arm do");
    expect(cask).not.toContain("on_intel do");
    expect(cask).not.toContain("x64");
    expect(cask).not.toContain("intel");
  });

  test("always points the binary stanza at Contents/MacOS/nodespace", () => {
    const digests: ArchDigestResult = { arm: armDigest };
    expect(renderCask("v0.2.0", digests)).toContain(
      'binary "#{appdir}/NodeSpace.app/Contents/MacOS/nodespace"',
    );
  });

  test("always includes a github_latest livecheck block", () => {
    const digests: ArchDigestResult = { arm: armDigest };
    const cask = renderCask("v0.2.0", digests);
    expect(cask).toContain("livecheck do");
    expect(cask).toContain("strategy :github_latest");
  });

  test("renders the full expected cask byte-for-byte for a known digest", () => {
    // Pinned to a real published sha256 (NodeSpace_0.2.0_aarch64.dmg) so
    // this test fails the moment renderCask's output changes at all,
    // intentionally or not -- update the expected string alongside any
    // deliberate change to the generator (as this one did, for the zap
    // trash list). The published tap only re-syncs to whatever the
    // generator currently produces on the next real release, so this is
    // the generator's own expected-output snapshot, not a live assertion
    // about what's on NodeSpaceAI/homebrew-nodespace right now.
    const digests: ArchDigestResult = {
      arm: {
        arch: "arm",
        fileName: "NodeSpace_0.2.0_aarch64.dmg",
        sha256: "b19edf954ae06c6c5845b148748c104750871179285ba352265844f98cffd638",
      },
    };
    const cask = renderCask("v0.2.0", digests);

    expect(cask).toBe(
      `cask "nodespace" do
  version "0.2.0"
  sha256 "b19edf954ae06c6c5845b148748c104750871179285ba352265844f98cffd638"

  # Apple Silicon (arm64) is the only supported macOS target. This is an
  # intentional decision, not a leftover workaround: there is no way to
  # verify x86_64 (Intel) macOS builds, and shipping a build nobody can
  # test is worse than not shipping it at all. It's reversible if that
  # changes -- Intel Mac users can build nodespace-core from source in
  # the meantime.
  url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{version}/NodeSpace_#{version}_aarch64.dmg"
  name "NodeSpace"
  desc "AI-native local-first knowledge management"
  homepage "https://nodespace.app/"

  # Explicit github_latest strategy: without this, brew's default livecheck
  # falls back to scanning ALL repo tags, which picks up unrelated
  # \`review-*\` tooling tags (e.g. review-20260813-095222) instead of the
  # actual latest published release.
  livecheck do
    url :url
    strategy :github_latest
  end

  # release.yml builds with MACOSX_DEPLOYMENT_TARGET=14.0 (Metal GPU
  # embeddings require Sonoma+).
  depends_on macos: :sonoma
  # arm64-only by design -- see the platform-support note above the \`url\` line.
  depends_on arch:  :arm64

  app "NodeSpace.app"
  binary "#{appdir}/NodeSpace.app/Contents/MacOS/nodespace"

  # \`~/.nodespace/models\` is deliberately NOT listed here -- it can hold
  # 100GB+ of downloaded model weights, and trashing that on every zap
  # would be a hostile surprise for a directory the user may reasonably
  # expect to survive an uninstall/reinstall cycle.
  zap trash: [
    "~/.nodespace/bin",
    "~/.nodespace/logs",
    "~/.nodespace/database",
    "~/Library/LaunchAgents/app.nodespace.daemon.plist",
    "~/Library/LaunchAgents/app.nodespace.daemon.dev.plist",
  ]
end
`,
    );
  });

  test("zap trash removes both the release and dev launchd agents, and the database, but not models", () => {
    const digests: ArchDigestResult = { arm: armDigest };
    const cask = renderCask("v0.2.0", digests);

    expect(cask).toContain('"~/Library/LaunchAgents/app.nodespace.daemon.plist"');
    expect(cask).toContain('"~/Library/LaunchAgents/app.nodespace.daemon.dev.plist"');
    expect(cask).toContain('"~/.nodespace/database"');
    // The comment explaining the exclusion mentions the path in prose;
    // this checks it never appears as an actual trashed array entry.
    expect(cask).not.toContain('"~/.nodespace/models"');
  });
});

describe("isVersionDrifted", () => {
  test("false when versions match, with or without a leading v", () => {
    expect(isVersionDrifted("0.2.0", "v0.2.0")).toBe(false);
    expect(isVersionDrifted("v0.2.0", "0.2.0")).toBe(false);
  });

  test("true when the tap is behind the latest release", () => {
    expect(isVersionDrifted("v0.1.6", "v0.2.0")).toBe(true);
  });
});
