// Covers the offline, deterministic parts of scripts/update-homebrew-cask.ts:
// digest hashing, arm64-only cask rendering, and the tap-drift comparison
// predicate. The GitHub-talking functions (fetchReleaseAssets,
// downloadAndHash, checkTapDrift, pushCaskUpdate) are intentionally not
// exercised here -- this suite runs as part of `bun run test:scripts` /
// `test:all` (the merge gate), which must stay fast and deterministic,
// not depend on network or `gh` auth.
import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  type ArchDigestResult,
  isVersionDrifted,
  normalizeVersion,
  renderCask,
  sha256Hex,
} from "./update-homebrew-cask";

// The behavioral tests below spawn /bin/sh, /usr/bin/plutil and /usr/bin/ruby:
// correctness checks, not performance ones, so the 5s default would measure
// how busy the machine is rather than whether the check works.
setDefaultTimeout(30_000);

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
    // deliberate change to the generator (as the last one did, for the
    // install checks and the zap trash list). The published tap only
    // re-syncs to whatever the generator currently produces on the next
    // real release, so this is the generator's own expected-output
    // snapshot, not a live assertion about what's on
    // NodeSpaceAI/homebrew-nodespace right now.
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

  # arm64-only by design -- see the platform-support note above the \`url\` line.
  depends_on arch:  :arm64
  # release.yml builds with MACOSX_DEPLOYMENT_TARGET=14.0 (Metal GPU
  # embeddings require Sonoma+).
  depends_on macos: :sonoma

  app "NodeSpace.app"
  binary "#{appdir}/NodeSpace.app/Contents/MacOS/nodespace"

  # Refuses to install over another NodeSpace product, reading only static
  # data from the app already in place.
  preflight_steps do
    if_path_exists "NodeSpace.app", base: :appdir do
      run "/bin/sh", args: ["-c", <<~SH, "sh", "{{appdir}}/NodeSpace.app"], print_stderr: false
        product=$(/usr/bin/plutil -extract NodeSpaceProduct raw -o - "$1/Contents/Info.plist" 2>/dev/null)
        if [ "$product" != community ]; then
          msg="The NodeSpace app on this Mac is a different NodeSpace product, or an older"
          msg="$msg NodeSpace that does not say which product it is. To replace it, move"
          msg="$msg /Applications/NodeSpace.app to the Trash, then run the install again."
          msg="$msg Your databases stay on this Mac."
          echo "$msg" >&2
          exit 1
        fi
      SH
    end
  end

  # The same check before an uninstall, which is how a reinstall or upgrade
  # sees the app: Homebrew moves the old app aside before the new version's
  # preflight_steps run.
  uninstall_preflight_steps do
    if_path_exists "NodeSpace.app", base: :appdir do
      run "/bin/sh", args: ["-c", <<~SH, "sh", "{{appdir}}/NodeSpace.app"], print_stderr: false
        product=$(/usr/bin/plutil -extract NodeSpaceProduct raw -o - "$1/Contents/Info.plist" 2>/dev/null)
        if [ "$product" != community ]; then
          msg="The NodeSpace app on this Mac is a different NodeSpace product, or an older"
          msg="$msg NodeSpace that does not say which product it is. To replace it, move"
          msg="$msg /Applications/NodeSpace.app to the Trash, then run the install again."
          msg="$msg Your databases stay on this Mac."
          echo "$msg" >&2
          exit 1
        fi
      SH
    end
  end

  # Neither \`~/.nodespace/database\` nor \`~/.nodespace/models\` is listed
  # here: a zap never deletes the user's databases, and models can hold
  # 100GB+ of downloaded weights the user may expect to survive an
  # uninstall/reinstall cycle.
  zap trash: [
    "~/.nodespace/bin",
    "~/.nodespace/logs",
    "~/Library/LaunchAgents/app.nodespace.daemon.dev.plist",
    "~/Library/LaunchAgents/app.nodespace.daemon.plist",
  ]
end
`,
    );
  });

  test("zap trash removes both launchd agents but neither the database nor the models", () => {
    const digests: ArchDigestResult = { arm: armDigest };
    const cask = renderCask("v0.2.0", digests);
    const zap = cask.slice(cask.indexOf("zap trash: ["), cask.indexOf("\nend\n"));

    expect(zap).toContain('"~/Library/LaunchAgents/app.nodespace.daemon.plist"');
    expect(zap).toContain('"~/Library/LaunchAgents/app.nodespace.daemon.dev.plist"');
    // A zap never deletes the user's databases; models can be 100GB+. The
    // comment above the stanza names both paths in prose, so this looks only
    // at the stanza itself.
    expect(zap).not.toContain("database");
    expect(zap).not.toContain("models");
  });
});

describe("renderCask other-product check", () => {
  const digests: ArchDigestResult = {
    arm: { arch: "arm", fileName: "NodeSpace_0.2.0_aarch64.dmg", sha256: "a".repeat(64) },
  };
  const cask = renderCask("v0.2.0", digests);

  // Written out in full on purpose: ADR-084 fixes this wording, and the .pkg
  // prints the same text but for "this installer" where this says "the
  // install". A test that built it from the generator's own constants could
  // not notice it drifting.
  const REFUSAL =
    "The NodeSpace app on this Mac is a different NodeSpace product, or an older NodeSpace that does not say " +
    "which product it is. To replace it, move /Applications/NodeSpace.app to the Trash, then run the install " +
    "again. Your databases stay on this Mac.";

  /** One top-level (two-space indented) stanza of the cask, `do` through `end`. */
  function stanza(name: string): string {
    const match = cask.match(new RegExp(`^  ${name} do\n[\\s\\S]*?\n  end$`, "m"));
    if (!match) throw new Error(`no ${name} stanza in the rendered cask`);
    return match[0];
  }

  /** The body of the stanza's `<<~SH` heredoc, dedented the way Ruby's `<<~` does. */
  function scriptOf(name: string): string {
    const match = stanza(name).match(/<<~SH[^\n]*\n([\s\S]*?)\n\s*SH\n/);
    if (!match) throw new Error(`no <<~SH heredoc in the ${name} stanza`);
    const lines = match[1].split("\n");
    const indent = Math.min(...lines.filter((l) => l.trim() !== "").map((l) => l.match(/^ */)?.[0].length ?? 0));
    return lines.map((l) => l.slice(indent)).join("\n") + "\n";
  }

  test("the check runs before an install and before an uninstall, guarded on an app being there", () => {
    for (const name of ["preflight_steps", "uninstall_preflight_steps"]) {
      const body = stanza(name);
      expect(body).toContain('if_path_exists "NodeSpace.app", base: :appdir do');
      expect(body).toContain('run "/bin/sh", args: ["-c", <<~SH, "sh", "{{appdir}}/NodeSpace.app"]');
    }
    // A reinstall or upgrade sees the old app only through the installed
    // version's uninstall-phase steps, so the two must be the same check.
    expect(scriptOf("uninstall_preflight_steps")).toBe(scriptOf("preflight_steps"));
  });

  test("a failing check must fail the install: the run step never opts out of must_succeed", () => {
    // `run` defaults to must_succeed: true. Turning it off would let the
    // refusal exit be ignored and the install carry on over the other product.
    for (const name of ["preflight_steps", "uninstall_preflight_steps"]) {
      expect(stanza(name)).not.toContain("must_succeed");
    }
  });

  test("the script reaches the shell unchanged: no #, no backslash, no {{", () => {
    // `<<~SH` (not `<<~'SH'`, which brew style rejects as redundant) still
    // interpolates `#{...}` and processes escapes, and Homebrew expands
    // `{{token}}` in every `run` argument, so any of these would change what
    // the shell receives.
    const script = scriptOf("preflight_steps");
    expect(script).not.toContain("#");
    expect(script).not.toContain("\\");
    expect(script).not.toContain("{{");
  });

  test("both stanzas sit after binary and before zap, the order brew style requires", () => {
    const at = (needle: string) => {
      const i = cask.indexOf(needle);
      expect(i).toBeGreaterThan(-1);
      return i;
    };
    expect(at("  preflight_steps do")).toBeGreaterThan(at('  binary "'));
    expect(at("  uninstall_preflight_steps do")).toBeGreaterThan(at("  preflight_steps do"));
    expect(at("  zap trash: [")).toBeGreaterThan(at("  uninstall_preflight_steps do"));
  });

  test.skipIf(process.platform !== "darwin" || !existsSync("/usr/bin/ruby"))(
    "the whole rendered cask is valid Ruby",
    () => {
      const result = spawnSync("/usr/bin/ruby", ["-c"], { input: cask, encoding: "utf8" });
      expect(result.stderr).toBe("");
      expect(result.status).toBe(0);
    },
  );

  // The script calls /usr/bin/plutil, so these need macOS. They run the real
  // script from the rendered cask against fixture bundles, with a scrubbed
  // environment.
  describe.skipIf(process.platform !== "darwin")("against fixture bundles", () => {
    // The space is deliberate: an appdir can contain one, and an unquoted
    // expansion in the script would then split the path and let a refusal
    // pass. Created in beforeAll because a skipped describe still runs its
    // body but not its hooks, so nothing is left behind off macOS.
    let tmp = "";
    beforeAll(() => {
      tmp = mkdtempSync(join(tmpdir(), "cask preflight "));
    });
    afterAll(() => rmSync(tmp, { recursive: true, force: true }));
    let counter = 0;

    type Fixture = {
      /** The NodeSpaceProduct entry, as raw plist XML; `undefined` leaves the key out. */
      product?: string;
      binaryPlist?: boolean;
    };

    const string = (value: string) => `<string>${value}</string>`;

    /** Builds `<tmp>/<n>/NodeSpace.app`; every executable inside appends its name to `<tmp>/<n>/ran` when run. */
    function bundle({ product, binaryPlist = false }: Fixture): string {
      const root = join(tmp, String(counter++));
      const app = join(root, "NodeSpace.app");
      const macos = join(app, "Contents", "MacOS");
      mkdirSync(macos, { recursive: true });
      const key = product === undefined ? "" : `<key>NodeSpaceProduct</key>${product}`;
      const plist = join(app, "Contents", "Info.plist");
      writeFileSync(
        plist,
        `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleName</key><string>NodeSpace</string>${key}</dict></plist>
`,
      );
      if (binaryPlist) {
        expect(spawnSync("/usr/bin/plutil", ["-convert", "binary1", plist]).status).toBe(0);
      }
      for (const name of ["nodespace", "nodespaced", "plutil"]) {
        const exe = join(macos, name);
        writeFileSync(exe, `#!/bin/sh\necho ${name} >> "${join(root, "ran")}"\necho community\n`);
        chmodSync(exe, 0o755);
      }
      return app;
    }

    function check(app: string): { status: number | null; stdout: string; stderr: string } {
      const result = spawnSync("/bin/sh", ["-c", scriptOf("preflight_steps"), "sh", app], {
        encoding: "utf8",
        env: { PATH: "/usr/bin:/bin" },
      });
      return { status: result.status, stdout: result.stdout, stderr: result.stderr };
    }

    const cases: Array<[string, Fixture, "proceeds" | "refuses"]> = [
      ["declares community", { product: string("community") }, "proceeds"],
      ["declares community in a binary plist", { product: string("community"), binaryPlist: true }, "proceeds"],
      ["declares another product", { product: string("other-product") }, "refuses"],
      ["declares another product in a binary plist", { product: string("other-product"), binaryPlist: true }, "refuses"],
      ["declares an empty product", { product: string("") }, "refuses"],
      ["declares a product that is not a string", { product: "<true/>" }, "refuses"],
      ["declares no product", {}, "refuses"],
    ];

    for (const [name, fixture, outcome] of cases) {
      test(`${outcome} when the app ${name}`, () => {
        const app = bundle(fixture);
        const result = check(app);
        if (outcome === "refuses") {
          expect(result.status).toBe(1);
          expect(result.stderr.trim()).toBe(REFUSAL);
        } else {
          expect(result.status).toBe(0);
          expect(result.stderr).toBe("");
        }
        expect(result.stdout).toBe("");
      });
    }

    test("refuses when the app has no readable Info.plist", () => {
      const app = bundle({ product: string("community") });
      rmSync(join(app, "Contents", "Info.plist"));
      const result = check(app);
      expect(result.status).toBe(1);
      expect(result.stderr.trim()).toBe(REFUSAL);
    });

    test("never runs anything inside the bundle", () => {
      const app = bundle({ product: string("other-product") });
      expect(check(app).status).toBe(1);
      expect(existsSync(join(app, "..", "ran"))).toBe(false);
    });
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
