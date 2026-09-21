#!/usr/bin/env bun
/**
 * Sync the Homebrew formula (NodeSpaceAI/homebrew-nodespace,
 * Formula/nodespace-cli.rb) to a published nodespace-core release.
 *
 * Sibling of scripts/update-homebrew-cask.ts, which does the same job for
 * the `nodespace` cask (the full GUI app). `nodespace-cli` is the headless
 * CLI + daemon path (`brew install nodespace-cli`) and ships a different
 * asset shape: 3 platform/arch targets (macOS arm64, Linux arm64, Linux
 * x86_64 -- see REQUIRED_HEADLESS_TARGETS, imported from
 * publish-install-script.ts, the same list install.sh pins against) x 2
 * binaries (`nodespace`, `nodespaced`) = 6 digests, versus the cask's
 * single arm64 .dmg. Before this script existed, the formula hardcoded its
 * own version and all 6 digests, independent of the cask's sync automation
 * -- the same "hand-maintained
 * artifact silently drifts" failure mode already fixed once for the cask,
 * reintroduced for the formula in the very PR that added it.
 *
 * The formula's `on_intel do odie(...) end` block under `on_macos` (no
 * macOS Intel build) stays hardcoded rather than becoming conditional on
 * asset presence: unlike the cask's now-removed Intel .dmg leg (which
 * `build-tauri` used to produce), release.yml's
 * `build-headless` matrix has never had a macOS x86_64 leg at all -- there
 * is no CI job this script could observe flip from absent to present.
 * Making this conditional now would be speculative generality for a
 * scenario with no live path to occur under the current pipeline.
 *
 * It never hand-types a digest: every sha256 is computed locally with
 * node:crypto against the actual bytes downloaded from the release asset
 * URL GitHub reports for that file, exactly like update-homebrew-cask.ts.
 *
 * Usage:
 *   bun run scripts/update-homebrew-formula.ts <version>            # dry run -- prints the
 *                                                                    # formula + diff, pushes nothing
 *   bun run scripts/update-homebrew-formula.ts <version> --push      # pushes to homebrew-nodespace's
 *                                                                     # main branch (requires
 *                                                                     # HOMEBREW_TAP_TOKEN)
 *   bun run scripts/update-homebrew-formula.ts drift-check           # compares the tap's live formula
 *                                                                     # version against the latest
 *                                                                     # published release; exits 1
 *                                                                     # (loud) on mismatch
 *
 * `--push` requires HOMEBREW_TAP_TOKEN: the same PAT (contents: write on
 * NodeSpaceAI/homebrew-nodespace) update-homebrew-cask.ts uses -- one repo
 * secret, shared by both sync jobs.
 *
 * Where this hooks in: release.yml's `sync-homebrew-formula` job runs this
 * script with `--push` on the `release` event, gated on `build-headless`
 * finishing (not cancelled) -- the same gating `sync-install-script` uses
 * for the same reason: `build-headless` is a single job with an internal
 * matrix, so `needs` only exposes its aggregate result, not individual
 * legs. This script's own resolveFormulaDigests() re-verifies the specific
 * assets it needs via `gh release view` and fails loudly (a hard error,
 * matching the "missing arm64 is a hard error, not a
 * warning" precedent set for the cask) if any of the 6 is missing -- every one
 * of REQUIRED_HEADLESS_TARGETS is required for this formula, not optional,
 * so there is no partial/degraded formula to render. `homebrew-drift-check.yml`
 * is an independent scheduled backstop that catches a stale formula even
 * if the sync job itself was skipped or failed silently.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pushFilesToRepo } from "./push-to-external-repo";
import { REQUIRED_HEADLESS_TARGETS } from "./publish-install-script";
import {
  downloadAndHash,
  fetchLatestReleaseVersion,
  fetchReleaseAssets,
  isVersionDrifted,
  normalizeVersion,
  TAP_REPO,
} from "./update-homebrew-cask";

export { TAP_REPO };

// Explicit, named targets rather than a generic loop over
// REQUIRED_HEADLESS_TARGETS: the Ruby template's structure genuinely
// differs per target (on_macos has only an arm leg; on_linux has arm +
// intel), so a mapping from "target string" to "which block it renders
// into" is inherently structural, not something a loop expresses more
// clearly. `requireTarget` below still ties each constant back to the
// shared REQUIRED_HEADLESS_TARGETS list at import time, so if that list
// ever changes without these three being updated to match, every
// consumer of this module (the sync command, drift-check, and
// `bun run test:scripts`) fails immediately and loudly instead of quietly
// resolving digests for the wrong target set.
function requireTarget(target: string): string {
  if (!(REQUIRED_HEADLESS_TARGETS as readonly string[]).includes(target)) {
    throw new Error(
      `internal error: "${target}" is not in publish-install-script.ts's REQUIRED_HEADLESS_TARGETS ` +
        `(${REQUIRED_HEADLESS_TARGETS.join(", ")}) -- update-homebrew-formula.ts's target constants ` +
        "are out of sync with it.",
    );
  }
  return target;
}

export const MACOS_ARM_TARGET = requireTarget("aarch64-apple-darwin");
export const LINUX_ARM_TARGET = requireTarget("aarch64-unknown-linux-gnu");
export const LINUX_X86_TARGET = requireTarget("x86_64-unknown-linux-gnu");

export interface FormulaAssetDigest {
  fileName: string;
  sha256: string;
}

interface TargetDigests {
  cli: FormulaAssetDigest;
  daemon: FormulaAssetDigest;
}

// One cli + one daemon digest per required target -- 3 targets x 2
// binaries = the formula's 6 hand-maintained digests this script replaces.
export interface FormulaDigests {
  macosArm: TargetDigests;
  linuxArm: TargetDigests;
  linuxX86: TargetDigests;
}

/** Resolves all 6 digests (cli + daemon, for each of the 3 required
 * targets) from the actual published release assets. Every one of these
 * targets is required for the formula to install on any of the platforms
 * it claims to support -- there is no architecture to gracefully omit the
 * way the cask can (a headless Linux user with a missing Linux binary has
 * no working install path at all), so a missing asset is a hard error,
 * not a warning -- matching the "missing arm64 is a hard
 * error" precedent set for the cask. */
export async function resolveFormulaDigests(
  version: string,
  workDir: string,
): Promise<FormulaDigests> {
  const v = normalizeVersion(version);
  const assets = await fetchReleaseAssets(version);

  async function digestFor(binary: "nodespace" | "nodespaced", target: string): Promise<FormulaAssetDigest> {
    const fileName = `${binary}-${target}`;
    const asset = assets.find((a) => a.name === fileName);
    if (!asset) {
      throw new Error(
        `release v${v} is missing ${fileName} -- cannot render nodespace-cli.rb without every ` +
          `required target built (${REQUIRED_HEADLESS_TARGETS.join(", ")}, cli + daemon each).`,
      );
    }
    const sha256 = await downloadAndHash(asset.url, join(workDir, fileName));
    return { fileName, sha256 };
  }

  // Independent network calls -- resolved concurrently rather than paying
  // the sum of all 6 download+hash round trips sequentially.
  const [macosArmCli, macosArmDaemon, linuxArmCli, linuxArmDaemon, linuxX86Cli, linuxX86Daemon] =
    await Promise.all([
      digestFor("nodespace", MACOS_ARM_TARGET),
      digestFor("nodespaced", MACOS_ARM_TARGET),
      digestFor("nodespace", LINUX_ARM_TARGET),
      digestFor("nodespaced", LINUX_ARM_TARGET),
      digestFor("nodespace", LINUX_X86_TARGET),
      digestFor("nodespaced", LINUX_X86_TARGET),
    ]);

  return {
    macosArm: { cli: macosArmCli, daemon: macosArmDaemon },
    linuxArm: { cli: linuxArmCli, daemon: linuxArmDaemon },
    linuxX86: { cli: linuxX86Cli, daemon: linuxX86Daemon },
  };
}

/** Renders the full Formula/nodespace-cli.rb content for the resolved
 * digests. Full re-render, not a two-line regex patch: the file has two
 * prose comments that mention the version by name (the "vX.Y.Z has no
 * macOS Intel build" note and its "gh release view vX.Y.Z" aside) in
 * addition to the `version "X.Y.Z"` stanza itself, so patching only the
 * stanza would leave those comments stale the moment a release actually
 * bumps the version -- the same reasoning renderCask() in
 * update-homebrew-cask.ts documents for always re-rendering the whole
 * cask rather than patching fields in place. */
export function renderFormula(version: string, digests: FormulaDigests): string {
  const v = normalizeVersion(version);
  return `class NodespaceCli < Formula
  desc "Headless CLI and daemon for NodeSpace, a local-first knowledge graph"
  homepage "https://nodespace.ai"
  # Kept explicit (rather than letting \`brew audit\` infer it from the url
  # below) so a future version bump is a single-line diff that every
  # \`#{version}\`-interpolated url picks up -- same reasoning
  # scripts/update-homebrew-cask.ts documents for the sibling cask.
  # \`brew audit --strict\` flags this as "redundant with version scanned
  # from URL"; that's a known, accepted trade-off, not an oversight.
  version "${v}"
  # nodespace-core's actual LICENSE file is FSL-1.1-Apache-2.0 (Functional
  # Source License), which has no SPDX identifier -- \`license
  # :cannot_represent\` is Homebrew's documented escape hatch for exactly
  # this case (a real license that isn't in SPDX's list), verified against
  # \`brew audit --strict\` (a literal SPDX-style string here fails audit:
  # "contains non-standard SPDX licenses").
  license :cannot_represent

  # A plain local var, not the \`version\` DSL method: inside a nested
  # \`resource "nodespaced" do ... end\` block below, \`self\` is the Resource,
  # not the Formula, so \`#{version}\` there resolves to the RESOURCE's own
  # (unset, empty) version rather than the Formula's -- confirmed the hard
  # way: \`brew install\` 404'd on ".../releases/download/v/nodespaced-..."
  # (empty version segment) before this was pulled out as a captured local.
  #
  # Must be assigned BEFORE the on_macos/on_linux blocks below: Ruby
  # decides whether a bare identifier is a local-variable reference or a
  # method call at PARSE time, based on whether an assignment to that name
  # has already been seen earlier in the source -- not at block-execution
  # time. \`brew style --fix\`'s automatic stanza reordering (which moves
  # on_macos/on_linux up, ahead of conflicts_with, per
  # FormulaAudit/ComponentsOrder) will happily move them above this
  # assignment too if run carelessly, which turns \`#{release_version}\`
  # into an undefined-local-variable-or-method error the moment the block
  # runs (confirmed by actually running \`brew style --fix\` and then \`brew
  # audit\` against the result). Keep this line above both blocks.
  release_version = version

  # Distinct from the \`nodespace\` cask (installs the full GUI app, which
  # bundles its own nodespaced + nodespace CLI under NodeSpace.app). This
  # formula is the headless-only path: \`brew install nodespace-cli\`, no
  # GUI, no Applications entry.
  #
  # Ships prebuilt binaries from nodespace-core's GitHub Releases, same as
  # the cask -- there's no source build here, just like the cask's .dmg.
  #
  # v${v} has no macOS Intel build (see the on_intel odie below). NOTE:
  # the release's own SHA256SUMS file lists checksums for
  # nodespace-x86_64-apple-darwin / nodespaced-x86_64-apple-darwin even
  # though neither is an actual uploaded release asset -- verified against
  # \`gh release view v${v} --json assets\`, not just SHA256SUMS. Every
  # digest below was computed locally from bytes actually downloaded from
  # the release, never copied from SHA256SUMS.
  on_macos do
    on_arm do
      url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{release_version}/${digests.macosArm.cli.fileName}"
      sha256 "${digests.macosArm.cli.sha256}"
    end
    on_intel do
      odie "nodespace-cli has no macOS Intel build in v#{release_version}. " \\
           "Use the nodespace-cli formula on Apple Silicon or Linux, or " \\
           "\`cargo install nodespace-cli\` in the meantime."
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{release_version}/${digests.linuxArm.cli.fileName}"
      sha256 "${digests.linuxArm.cli.sha256}"
    end
    on_intel do
      url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{release_version}/${digests.linuxX86.cli.fileName}"
      sha256 "${digests.linuxX86.cli.sha256}"
    end
  end

  # NOTE: a real, verified path collision. The cask's \`binary\` stanza and
  # this formula's \`bin.install ... => "nodespace"\` both resolve to the
  # same symlink path, $(brew --prefix)/bin/nodespace. There is no
  # Homebrew mechanism that actually prevents this across the cask/formula
  # boundary -- verified directly, not assumed: \`conflicts_with cask:
  # "..."\` is accepted by the formula DSL but has NO enforcement effect
  # (confirmed: installing this formula while the cask's bin/nodespace
  # symlink exists still succeeds); the cask-side mirror,
  # \`conflicts_with formula: "..."\`, doesn't exist at all in current
  # Homebrew (\`Unknown key: :formula. Valid keys are: :cask\`). So neither
  # side can declare this.
  #
  # What Homebrew actually does (also verified directly, with a synthetic
  # cask providing a real bin/nodespace symlink and then installing this
  # formula against it): the SECOND of the two to install still installs
  # its files into its own Cellar keg successfully, but \`brew link\` skips
  # the conflicting bin/nodespace symlink with a clear "already exists"
  # warning rather than either failing outright or silently overwriting
  # it -- and it skips linking bin/nodespaced too (Homebrew links a keg's
  # files as a unit, not file-by-file), even though nodespaced's name
  # doesn't collide with anything. \`brew link --overwrite nodespace-cli\`
  # (or the equivalent for the cask) resolves it, consciously choosing
  # which copy PATH should resolve to. \`brew services start nodespace-cli\`
  # still works either way -- see the caveats below.

  # The CLI is useless without the daemon it talks to over a Unix socket
  # -- this formula installs both, matching what the cask's .app bundle
  # already ships (nodespaced alongside the CLI).
  resource "nodespaced" do
    on_macos do
      on_arm do
        url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{release_version}/${digests.macosArm.daemon.fileName}"
        sha256 "${digests.macosArm.daemon.sha256}"
      end
    end
    on_linux do
      on_arm do
        url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{release_version}/${digests.linuxArm.daemon.fileName}"
        sha256 "${digests.linuxArm.daemon.sha256}"
      end
      on_intel do
        url "https://github.com/NodeSpaceAI/nodespace-core/releases/download/v#{release_version}/${digests.linuxX86.daemon.fileName}"
        sha256 "${digests.linuxX86.daemon.sha256}"
      end
    end
  end

  def install
    # Each on_macos/on_linux branch above resolves to exactly one url, so
    # exactly one nodespace-<triple> file is present in the staging dir --
    # \`Dir[...]\` sidesteps hardcoding the per-branch filename a second
    # time. Guarded explicitly rather than letting a missing/renamed
    # asset surface as a bare Ruby TypeError on \`nil => "nodespace"\`.
    #
    # Explicit chmod after each install: verified in an isolated Homebrew
    # prefix that the downloaded binaries already land executable (the
    # release build pipeline chmods them before upload, and Homebrew's own
    # install path preserved that through a real \`brew install\` -- \`ls -la\`
    # showed \`-r-xr-xr-x\` and \`brew test\` passed without this line). Kept
    # anyway as a zero-cost guarantee that doesn't depend on the upload
    # pipeline or Homebrew's internals continuing to behave that way.
    cli_binary = Dir["nodespace-*"].first
    odie "no nodespace-<triple> binary found in the downloaded archive" if cli_binary.nil?
    bin.install cli_binary => "nodespace"
    (bin/"nodespace").chmod 0755

    resource("nodespaced").stage do
      daemon_binary = Dir["nodespaced-*"].first
      odie "no nodespaced-<triple> binary found in the downloaded archive" if daemon_binary.nil?
      bin.install daemon_binary => "nodespaced"
      (bin/"nodespaced").chmod 0755
    end
  end

  service do
    run [opt_bin/"nodespaced"]
    # \`keep_alive false\` is a no-op, not "don't restart" -- Homebrew's
    # \`keep_alive?\` is only true when this is explicitly set truthy, and
    # false/omitted both leave it false either way. \`true\` here is what
    # actually gets launchd/systemd to relaunch nodespaced if it crashes,
    # which is the whole point of \`brew services start\` over just running
    # the binary directly (see the caveats above). \`brew services stop\`
    # still works normally -- it unloads the service definition rather
    # than fighting KeepAlive.
    keep_alive true
    # Without this, \`nodespaced\` defaults to its GUI-app tray mode (a
    # \`tao\`/\`NSApplication\` event loop taking over the process's main
    # thread) -- the right default for the cask's bundled daemon, but never
    # for this formula, which ships no GUI at all. Verified directly on a
    # real daemon: with the env var unset, \`kill -TERM\`/\`SIGINT\` is never
    # observed by the process at all (a live thread sample shows every
    # worker thread and the run loop fully parked, 0% CPU, forever) and it
    # requires \`SIGKILL\`; with \`NODESPACED_HEADLESS=1\` set, the identical
    # signal is handled and the process exits cleanly in well under a
    # second. \`brew services start/stop nodespace-cli\` relies on a clean
    # exit here -- \`stop\` sends SIGTERM and waits.
    #
    # Verified this way on macOS only. \`environment_variables\` is Homebrew's
    # own cross-platform Services DSL primitive -- the same declaration is
    # documented to translate into both the generated launchd plist (macOS)
    # and the generated systemd unit (Linux) -- but that Linux translation
    # was not independently re-verified against a live \`brew services start\`
    # on this formula's Linux targets.
    environment_variables NODESPACED_HEADLESS: "1"
    log_path var/"log/nodespace/nodespaced.log"
    error_log_path var/"log/nodespace/nodespaced.log"
  end

  def caveats
    <<~EOS
      nodespaced (the daemon) must be running before \`nodespace\` commands work:
        NODESPACED_HEADLESS=1 nodespaced &   # run directly, or
        brew services start nodespace-cli   # run as a background service

      This is the headless CLI only -- no GUI, no Applications entry. For
      the full app, use \`brew install --cask nodespace\` instead. Both CAN
      be installed at once, but they claim the same \`nodespace\` binary
      name on PATH: whichever installs second gets its bin/nodespace link
      skipped with a warning rather than silently overwritten. Run
      \`brew link --overwrite nodespace-cli\` (or \`--overwrite nodespace\`
      for the cask) to choose which one PATH resolves to.
      \`brew services start nodespace-cli\` and running \`nodespaced\`
      directly both work regardless of link state.
    EOS
  end

  test do
    cli_output = shell_output("#{bin}/nodespace --version")
    assert_match version.to_s, cli_output

    # Also asserts the nested \`nodespaced\` resource actually installed and
    # runs -- a regression in its Dir-glob/sha256/install-target handling
    # would otherwise pass this test block while leaving the daemon (which
    # every \`nodespace\` command depends on) silently missing.
    assert_path_exists bin/"nodespaced"
    assert_predicate bin/"nodespaced", :executable?
    daemon_output = shell_output("#{bin}/nodespaced --version")
    assert_match version.to_s, daemon_output
  end
end
`;
}

/** Pure comparison -- no network. Reuses isVersionDrifted from
 * update-homebrew-cask.ts rather than redefining the same string-compare
 * twice. */
export interface FormulaDriftCheckResult {
  ok: boolean;
  tapVersion: string;
  latestReleaseVersion: string;
}

export async function fetchTapFormulaVersion(): Promise<string> {
  const res = await fetch(
    `https://raw.githubusercontent.com/${TAP_REPO}/main/Formula/nodespace-cli.rb`,
  );
  if (!res.ok) {
    throw new Error(`failed to fetch tap formula: HTTP ${res.status}`);
  }
  const text = await res.text();
  const m = text.match(/^\s*version\s+"([^"]+)"/m);
  if (!m) throw new Error("could not find a version stanza in the tap's Formula/nodespace-cli.rb");
  return m[1];
}

export async function checkFormulaDrift(): Promise<FormulaDriftCheckResult> {
  const [tapVersion, latestReleaseVersion] = await Promise.all([
    fetchTapFormulaVersion(),
    fetchLatestReleaseVersion(),
  ]);
  return {
    ok: !isVersionDrifted(tapVersion, latestReleaseVersion),
    tapVersion,
    latestReleaseVersion,
  };
}

async function pushFormulaUpdate(
  version: string,
  formulaContent: string,
  token: string,
): Promise<void> {
  const v = normalizeVersion(version);
  const pushed = await pushFilesToRepo(
    TAP_REPO,
    [{ relPath: "Formula/nodespace-cli.rb", content: formulaContent }],
    `Update nodespace-cli formula to v${v} (automated release sync)`,
    token,
  );
  console.log(
    pushed
      ? `Pushed formula update for v${v} to ${TAP_REPO}.`
      : "Tap formula already matches -- nothing to push.",
  );
}

function usage(): void {
  console.log(`Usage:
  bun run scripts/update-homebrew-formula.ts <version> [--push]
  bun run scripts/update-homebrew-formula.ts drift-check`);
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command) {
    usage();
    process.exit(1);
  }

  if (command === "drift-check") {
    // Genuine drift and "the check itself couldn't run" (network/gh/auth
    // blip) both need to fail loud (exit 1), but they are NOT the same
    // situation -- an operator triaging an auto-filed issue needs to be
    // able to tell them apart from the message alone, matching
    // update-homebrew-cask.ts's drift-check command.
    let r: FormulaDriftCheckResult;
    try {
      r = await checkFormulaDrift();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error(
        `DRIFT CHECK ERROR (not necessarily drift -- the check itself failed to run): ${message}`,
      );
      process.exit(1);
    }
    if (r.ok) {
      console.log(`Tap formula in sync: v${normalizeVersion(r.tapVersion)}`);
      return;
    }
    console.error(
      `FORMULA DRIFT: homebrew-nodespace's nodespace-cli formula reports v${normalizeVersion(r.tapVersion)}, ` +
        `but the latest published release is v${normalizeVersion(r.latestReleaseVersion)}.\n` +
        `Fix: bun run scripts/update-homebrew-formula.ts ${r.latestReleaseVersion} --push`,
    );
    process.exit(1);
  }

  if (!/^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$/.test(command)) {
    usage();
    process.exit(1);
  }

  const push = args.includes("--push");
  const token = process.env.HOMEBREW_TAP_TOKEN;
  if (push && !token) {
    console.error(
      "HOMEBREW_TAP_TOKEN is not set -- required for --push (a PAT with contents:write on " +
        `${TAP_REPO}). Running without --push shows what would change.`,
    );
    process.exit(1);
  }

  const workDir = mkdtempSync(join(tmpdir(), "nodespace-formula-assets-"));
  try {
    const digests = await resolveFormulaDigests(command, workDir);
    const content = renderFormula(command, digests);
    console.log("--- Formula/nodespace-cli.rb ---");
    console.log(content);

    if (!push) {
      console.log("(dry run -- pass --push with HOMEBREW_TAP_TOKEN set to publish this)");
      return;
    }
    await pushFormulaUpdate(command, content, token as string);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  // Matches update-homebrew-cask.ts's / scripts/test-gate.ts's convention:
  // a bare uncaught rejection here would otherwise print a raw Bun stack
  // trace / ShellError dump instead of an operator-facing message.
  try {
    await main();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`✗ ${message}`);
    process.exit(1);
  }
}
