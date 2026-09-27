#!/usr/bin/env bun
/**
 * Sync the Homebrew cask (NodeSpaceAI/homebrew-nodespace) to a published
 * nodespace-core release.
 *
 * This repo has no hosted CI (by design) — release-cutting is the local
 * `bun run release <version>` flow (scripts/release.ts), which tags and
 * publishes the GitHub release that `.github/workflows/release.yml` then
 * builds assets for. Nothing in that chain used to touch the tap: the cask
 * was hand-edited in a separate repo, silently drifted for two months
 * (v0.1.6 while v0.2.0 shipped), and nothing caught it. This script is the
 * step that was missing.
 *
 * It never hand-types a digest: every sha256 is computed locally with
 * node:crypto against the actual bytes downloaded from the release asset
 * URL GitHub reports for that file.
 *
 * Usage:
 *   bun run scripts/update-homebrew-cask.ts <version>            # dry run — prints the
 *                                                                 # cask + diff, pushes nothing
 *   bun run scripts/update-homebrew-cask.ts <version> --push      # pushes to homebrew-nodespace's
 *                                                                 # main branch (requires
 *                                                                 # HOMEBREW_TAP_TOKEN)
 *   bun run scripts/update-homebrew-cask.ts drift-check           # compares the tap's live cask
 *                                                                 # version against the latest
 *                                                                 # published release; exits 1
 *                                                                 # (loud) on mismatch
 *
 * `--push` requires HOMEBREW_TAP_TOKEN: a PAT (or fine-grained token) with
 * `contents: write` on NodeSpaceAI/homebrew-nodespace, set as a repo secret.
 * `secrets.GITHUB_TOKEN` is scoped to nodespace-core only and cannot push
 * cross-repo.
 *
 * Where this hooks in: release.yml's `sync-homebrew-cask` job runs this
 * script with `--push` on the `release` event, gated on `build-tauri-macos-arm`
 * (deliberately not on `build-tauri`, so a Windows build failure cannot
 * block the cask sync — see that job's comments for why). If it fails, the
 * workflow's `notify-failure` job opens an issue. `homebrew-drift-check.yml`
 * is an independent scheduled backstop that catches a stale cask even if
 * the sync job itself was skipped or failed silently.
 *
 * The actual "clone the tap, write the file, commit, push" mechanics live
 * in push-to-external-repo.ts, shared with update-homebrew-formula.ts (the same
 * kind of sync job for the sibling `nodespace-cli` Homebrew formula).
 */

import { $ } from "bun";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pushFilesToRepo } from "./push-to-external-repo";

export const CORE_REPO = "NodeSpaceAI/nodespace-core";
export const TAP_REPO = "NodeSpaceAI/homebrew-nodespace";

// The .app bundle's CLI binary lives at Contents/MacOS/nodespace in the
// current Tauri packaging layout (verified by mounting the real v0.2.0 .dmg
// — the previous hand-maintained cask pointed at Contents/Resources/bin/
// instead, which doesn't exist and breaks `brew install --cask`). Re-verify
// this path if the Tauri bundle/externalBin layout ever changes.
const CASK_BINARY_PATH = "Contents/MacOS/nodespace";

// release.yml builds with MACOSX_DEPLOYMENT_TARGET=14.0 (Metal GPU
// embeddings require Sonoma+).
const MIN_MACOS = "sonoma";

export interface ReleaseAsset {
  name: string;
  url: string;
}

// Apple Silicon (arm64) is the only supported macOS target: there is no way
// to verify x86_64 (Intel) macOS builds, and shipping a build nobody can
// test is worse than not shipping it at all (same reasoning
// publish-install-script.ts's REQUIRED_HEADLESS_TARGETS comment documents
// for excluding x86_64-apple-darwin there). This is reversible -- re-adding
// an architecture later means restoring a suffix map (like the
// `ARCH_SUFFIX: Record<"arm" | "intel", string>` this file used to have)
// and a second rendering branch in renderCask() (on_arm/on_intel).
export interface ArchDigest {
  arch: "arm";
  fileName: string;
  sha256: string;
}

// The Apple Silicon .dmg filename suffix release.yml / tauri-action produce
// -- e.g. NodeSpace_0.2.0_aarch64.dmg. Used both to look up the real release
// asset (with the resolved version) and to build the cask's url stanza
// (with Ruby's `#{version}` interpolation, so a future manual glance at the
// file matches how casks are conventionally written).
const ARCH_SUFFIX_AARCH64 = "aarch64";

export function normalizeVersion(version: string): string {
  return version.replace(/^v/, "");
}

/** Pure hashing helper — kept separate from network I/O so it's unit-testable. */
export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export async function fetchReleaseAssets(version: string): Promise<ReleaseAsset[]> {
  const tag = version.startsWith("v") ? version : `v${version}`;
  const out = await $`gh release view ${tag} --repo ${CORE_REPO} --json assets`.text();
  const parsed = JSON.parse(out) as { assets: ReleaseAsset[] };
  return parsed.assets;
}

/** Downloads `url` to `destPath` and returns its sha256 — the digest always
 * comes from bytes that were actually fetched, never typed or copied. */
export async function downloadAndHash(url: string, destPath: string): Promise<string> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`failed to download ${url}: HTTP ${res.status}`);
  }
  const bytes = new Uint8Array(await res.arrayBuffer());
  await Bun.write(destPath, bytes);
  return sha256Hex(bytes);
}

export interface ArchDigestResult {
  arm: ArchDigest;
}

/** Resolves the Apple Silicon .dmg digest from the actual published release
 * assets. There is no other macOS architecture to fall back to, so a
 * missing arm asset is a hard error, not a warning -- see the file header
 * comment on why Apple Silicon is the only target. */
export async function resolveArchDigests(
  version: string,
  workDir: string,
): Promise<ArchDigestResult> {
  const v = normalizeVersion(version);
  const assets = await fetchReleaseAssets(version);
  const fileName = `NodeSpace_${v}_${ARCH_SUFFIX_AARCH64}.dmg`;

  const asset = assets.find((a) => a.name === fileName);
  if (!asset) {
    throw new Error(
      `release v${v} is missing its Apple Silicon build (${fileName}) -- cannot render a cask ` +
        "with no macOS artifact to point at.",
    );
  }
  const sha256 = await downloadAndHash(asset.url, join(workDir, fileName));
  return { arm: { arch: "arm", fileName, sha256 } };
}

const CASK_HEADER = (version: string) => `cask "nodespace" do
  version "${version}"`;

// Matches the platform-support note already published in
// NodeSpaceAI/homebrew-nodespace's Casks/nodespace.rb word for word -- kept
// here so a fresh render from this generator reproduces that file
// byte-for-byte instead of drifting from the wording that's actually live.
// If this note ever needs to change, update it in both places together.
const ARCH_NOTE = `  # Apple Silicon (arm64) is the only supported macOS target. This is an
  # intentional decision, not a leftover workaround: there is no way to
  # verify x86_64 (Intel) macOS builds, and shipping a build nobody can
  # test is worse than not shipping it at all. It's reversible if that
  # changes -- Intel Mac users can build nodespace-core from source in
  # the meantime.`;

const NAME_BLOCK = `  name "NodeSpace"
  desc "AI-native local-first knowledge management"
  homepage "https://nodespace.app/"`;

// Stanza order matters to `brew style`/`brew audit --cask`: livecheck comes
// right after name/desc/homepage, before depends_on -- verified against
// `brew style --fix`'s own canonical reordering.
const LIVECHECK_BLOCK = `
  # Explicit github_latest strategy: without this, brew's default livecheck
  # falls back to scanning ALL repo tags, which picks up unrelated
  # \`review-*\` tooling tags (e.g. review-20260813-095222) instead of the
  # actual latest published release.
  livecheck do
    url :url
    strategy :github_latest
  end`;

const CASK_FOOTER = (binaryPath: string) => `
  app "NodeSpace.app"
  binary "#{appdir}/NodeSpace.app/${binaryPath}"

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
`;

// The url stanza uses Ruby's `#{version}` interpolation for the filename
// too (matching the resolved digest's fileName, just not hardcoded to the
// current version) -- conventional cask style, and means a reader diffing
// this file sees the same pattern regardless of which version generated it.
const ARM_URL_FILENAME = `NodeSpace_#{version}_${ARCH_SUFFIX_AARCH64}.dmg`;

/** Renders the full Casks/nodespace.rb content for the resolved Apple
 * Silicon digest -- the only architecture this generator targets (see the
 * comment on ArchDigest above for why). resolveArchDigests() throws before
 * returning if that digest isn't available, so there is no "missing
 * architecture" case to render around here -- a cask is only ever rendered
 * for a confirmed, downloaded, hashed asset. */
export function renderCask(version: string, digests: ArchDigestResult): string {
  const v = normalizeVersion(version);
  const { arm } = digests;

  return `${CASK_HEADER(v)}
  sha256 "${arm.sha256}"

${ARCH_NOTE}
  url "https://github.com/${CORE_REPO}/releases/download/v#{version}/${ARM_URL_FILENAME}"
${NAME_BLOCK}
${LIVECHECK_BLOCK}

  # release.yml builds with MACOSX_DEPLOYMENT_TARGET=14.0 (Metal GPU
  # embeddings require Sonoma+).
  depends_on macos: :${MIN_MACOS}
  # arm64-only by design -- see the platform-support note above the \`url\` line.
  depends_on arch:  :arm64
${CASK_FOOTER(CASK_BINARY_PATH)}`;
}

/** Pure comparison — no network. Exported separately from checkTapDrift so
 * the decision logic is unit-testable without hitting GitHub. */
export function isVersionDrifted(tapVersion: string, latestReleaseVersion: string): boolean {
  return normalizeVersion(tapVersion) !== normalizeVersion(latestReleaseVersion);
}

export interface DriftCheckResult {
  ok: boolean;
  tapVersion: string;
  latestReleaseVersion: string;
}

export async function fetchTapCaskVersion(): Promise<string> {
  const res = await fetch(
    `https://raw.githubusercontent.com/${TAP_REPO}/main/Casks/nodespace.rb`,
  );
  if (!res.ok) {
    throw new Error(`failed to fetch tap cask: HTTP ${res.status}`);
  }
  const text = await res.text();
  const m = text.match(/^\s*version\s+"([^"]+)"/m);
  if (!m) throw new Error("could not find a version stanza in the tap's Casks/nodespace.rb");
  return m[1];
}

/** The `github_latest` API-backed release, not a raw tag scan -- avoids
 * picking up unrelated tags (e.g. this repo's `review-*` tooling tags). */
export async function fetchLatestReleaseVersion(): Promise<string> {
  const out =
    await $`gh release list --repo ${CORE_REPO} --json tagName,isLatest --jq '.[] | select(.isLatest) | .tagName'`.text();
  const tag = out.trim();
  if (!tag) throw new Error("could not determine the latest release from `gh release list`");
  return tag;
}

export async function checkTapDrift(): Promise<DriftCheckResult> {
  const [tapVersion, latestReleaseVersion] = await Promise.all([
    fetchTapCaskVersion(),
    fetchLatestReleaseVersion(),
  ]);
  return {
    ok: !isVersionDrifted(tapVersion, latestReleaseVersion),
    tapVersion,
    latestReleaseVersion,
  };
}

async function pushCaskUpdate(version: string, caskContent: string, token: string): Promise<void> {
  const v = normalizeVersion(version);
  const pushed = await pushFilesToRepo(
    TAP_REPO,
    [{ relPath: "Casks/nodespace.rb", content: caskContent }],
    `Update cask to v${v} (automated release sync)`,
    token,
  );
  console.log(
    pushed
      ? `Pushed cask update for v${v} to ${TAP_REPO}.`
      : "Tap cask already matches -- nothing to push.",
  );
}

function usage(): void {
  console.log(`Usage:
  bun run scripts/update-homebrew-cask.ts <version> [--push]
  bun run scripts/update-homebrew-cask.ts drift-check`);
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
    // situation -- an operator triaging an auto-filed issue needs to be able
    // to tell them apart from the message alone, so they're deliberately
    // worded and labeled differently rather than sharing one code path.
    let r: DriftCheckResult;
    try {
      r = await checkTapDrift();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error(
        `DRIFT CHECK ERROR (not necessarily drift -- the check itself failed to run): ${message}`,
      );
      process.exit(1);
    }
    if (r.ok) {
      console.log(`Tap in sync: v${normalizeVersion(r.tapVersion)}`);
      return;
    }
    console.error(
      `TAP DRIFT: homebrew-nodespace's cask reports v${normalizeVersion(r.tapVersion)}, ` +
        `but the latest published release is v${normalizeVersion(r.latestReleaseVersion)}.\n` +
        `Fix: bun run scripts/update-homebrew-cask.ts ${r.latestReleaseVersion} --push`,
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

  const workDir = mkdtempSync(join(tmpdir(), "nodespace-cask-assets-"));
  try {
    const digests = await resolveArchDigests(command, workDir);
    const content = renderCask(command, digests);
    console.log("--- Casks/nodespace.rb ---");
    console.log(content);

    if (!push) {
      console.log("(dry run -- pass --push with HOMEBREW_TAP_TOKEN set to publish this)");
      return;
    }
    await pushCaskUpdate(command, content, token as string);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  // Matches scripts/test-gate.ts's convention: a bare uncaught rejection
  // here would otherwise print a raw Bun stack trace / ShellError dump
  // instead of an operator-facing message.
  try {
    await main();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`✗ ${message}`);
    process.exit(1);
  }
}
