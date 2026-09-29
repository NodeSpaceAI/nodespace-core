#!/usr/bin/env bun
/**
 * Pin nodespace-website's install.sh (the `curl -fsSL https://nodespace.ai/
 * install.sh | sh` bootstrap installer) to a published nodespace-core
 * release.
 *
 * install.sh is a committed static file in NodeSpaceAI/nodespace-website
 * -- it is not generated from a template here. What this script owns is
 * exactly one line inside that file:
 *
 *   NODESPACE_CLI_VERSION="v0.2.0"
 *
 * install.sh deliberately does NOT fetch "latest" from GitHub Releases --
 * this repo's release workflow publishes a release (making it "latest")
 * before that release's build-and-upload jobs even start, so "latest" can
 * point at a release with a missing or partial asset set for several
 * minutes after it's created. This script is the thing that advances the
 * pin, and it only runs after this repo's own release build jobs report
 * success (see the release workflow's sync-install-script job, a sibling
 * of the job that keeps the Homebrew cask in sync the same way -- both
 * exist because a hand-maintained downstream artifact silently drifted
 * for two months before anything caught it).
 *
 * Usage:
 *   bun run scripts/publish-install-script.ts <version>            # dry run --
 *                                                                   # prints the diff, pushes nothing
 *   bun run scripts/publish-install-script.ts <version> --push      # pushes to nodespace-website's
 *                                                                    # main branch
 *
 *   bun run scripts/publish-install-script.ts drift-check          # compares the pin in the website
 *                                                                   # repo AND on the live site against
 *                                                                   # the latest release; exits 1 on mismatch
 *
 * WEBSITE_DEPLOY_TOKEN is required either way: a PAT (or fine-grained token)
 * with `contents: write` on NodeSpaceAI/nodespace-website, set as a repo
 * secret once this is wired into the release flow -- `secrets.GITHUB_TOKEN`
 * is scoped to nodespace-core only and cannot push cross-repo (same
 * constraint the Homebrew cask sync's token documents). Even a dry run needs
 * it: nodespace-website is a private repo, so the read-only preview fetch
 * below 404s without authentication too.
 */

import { $ } from "bun";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const CORE_REPO = "NodeSpaceAI/nodespace-core";
export const WEBSITE_REPO = "NodeSpaceAI/nodespace-website";

// The CLI targets release.yml's build-headless job actually produces and
// uploads for every release. x86_64-apple-darwin is deliberately excluded
// -- a real release shipped without it despite its own SHA256SUMS listing
// a checksum for it (a release-asset/checksum-file mismatch, filed
// separately), and install.sh's own direct-download path already degrades
// to a clear error message on unsupported platforms rather than assuming
// every target always exists.
//
// Exported: update-homebrew-formula.ts's own required-target set is this
// same list (the `nodespace-cli` Homebrew formula ships prebuilt binaries
// for exactly the targets install.sh does). Sharing one list rather than
// maintaining two prevents them silently drifting apart the way the cask
// and this pin once did independently of each other.
export const REQUIRED_HEADLESS_TARGETS = [
  "aarch64-apple-darwin",
  "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-gnu",
] as const;

export function normalizeTag(version: string): string {
  return version.startsWith("v") ? version : `v${version}`;
}

// `g` + `m`: `g` so a stale duplicate pin line is DETECTED (see pinVersion
// below) instead of just having its first occurrence silently patched
// while a second one is left stale; `m` so `^`/`$` anchor to the line, not
// the whole file. Trailing whitespace is matched with `[ \t]*`, not `\s*`
// -- `\s` also matches the newline itself in `/m` mode, so a greedy
// `\s*$` would swallow the line's own trailing `\n` into the match and
// `.replace()` would then drop it, silently merging this line into the
// next one (caught by publish-install-script.test.ts's exact-shape
// assertions). Only ever consumed via `.match()`/`.replace()` here, never
// `.test()`/`.exec()` -- both of those mutate a global regex's `lastIndex`
// between calls on the same object, which `.match()`/`.replace()` don't.
const VERSION_PIN_RE = /^NODESPACE_CLI_VERSION="v?[^"]+"[ \t]*$/gm;

/** Pure string transform -- no network -- so it's unit-testable without
 * touching install.sh's actual current content. Throws rather than
 * silently guessing if the marker line is missing OR duplicated: a rename
 * of the variable in install.sh, or a second stray copy of the line, both
 * fail this script loudly instead of leaving a stale (or ambiguous) pin
 * in place. */
export function pinVersion(installShContent: string, version: string): string {
  const matches = installShContent.match(VERSION_PIN_RE) ?? [];
  if (matches.length === 0) {
    throw new Error(
      'could not find a NODESPACE_CLI_VERSION="..." pin line in install.sh -- ' +
        "has it been renamed or restructured? Refusing to guess where the pin goes.",
    );
  }
  if (matches.length > 1) {
    throw new Error(
      `found ${matches.length} NODESPACE_CLI_VERSION="..." pin lines in install.sh -- ` +
        "expected exactly one. Refusing to guess which is authoritative.",
    );
  }
  return installShContent.replace(VERSION_PIN_RE, `NODESPACE_CLI_VERSION="${normalizeTag(version)}"`);
}

export interface AssetCheckResult {
  missing: string[];
}

/** Confirms the release actually has the headless CLI binaries install.sh
 * will try to download before pinning to it -- pinning to a version whose
 * assets never finished uploading would be worse than staying on the
 * previous pin. Missing is reported, not silently ignored -- callers
 * decide whether to proceed (see main()'s --push gate). */
export async function checkReleaseAssets(version: string): Promise<AssetCheckResult> {
  const tag = normalizeTag(version);
  const out = await $`gh release view ${tag} --repo ${CORE_REPO} --json assets`.text();
  const parsed = JSON.parse(out) as { assets: { name: string }[] };
  const names = new Set(parsed.assets.map((a) => a.name));
  const missing = REQUIRED_HEADLESS_TARGETS.filter((t) => !names.has(`nodespace-${t}`));
  return { missing };
}

/** Read-only preview of the website's current install.sh, used only for
 * the dry-run/diff-preview console output below -- never as the basis for
 * what actually gets written. The raw.githubusercontent.com endpoint this
 * hits is CDN-cached, so it can lag a fraction behind the real HEAD; that
 * lag only matters for a write, and the write path (pushInstallScriptUpdate)
 * reads its own fresh `git clone` instead of this.
 *
 * nodespace-website is a private repo, so the anonymous raw.githubusercontent.com
 * request 404s with no token -- main() passes WEBSITE_DEPLOY_TOKEN through so a
 * local dry run needs that env var set too, same as --push. */
export async function fetchWebsiteInstallScript(token?: string): Promise<string> {
  const headers: HeadersInit = token ? { Authorization: `token ${token}` } : {};
  const res = await fetch(`https://raw.githubusercontent.com/${WEBSITE_REPO}/main/install.sh`, { headers });
  if (!res.ok) {
    throw new Error(`failed to fetch ${WEBSITE_REPO}'s install.sh: HTTP ${res.status}`);
  }
  return res.text();
}

/** Clones the website repo fresh, patches its ACTUAL current install.sh
 * (read from the clone, not from fetchWebsiteInstallScript's CDN-cached
 * preview above) and pushes if it changed. Reading from the fresh clone
 * rather than reusing the earlier preview matters: an unrelated legitimate
 * edit to install.sh landing between this script starting and this step
 * running would otherwise be silently overwritten by a patch built from
 * stale content -- the diff-check below only guards "nothing changed",
 * not "changed to something built from stale content". */
async function pushInstallScriptUpdate(version: string, token: string): Promise<void> {
  const workDir = mkdtempSync(join(tmpdir(), "nodespace-website-push-"));
  try {
    const authUrl = `https://x-access-token:${token}@github.com/${WEBSITE_REPO}.git`;
    try {
      await $`git clone --depth 1 ${authUrl} ${workDir}`.quiet();
    } catch (err) {
      // Same defense-in-depth as the Homebrew cask sync's push helper:
      // git itself redacts credentials from its own stderr on an auth
      // failure, but scrub the raw token out of whatever the shell
      // wrapper's error carries anyway -- this must never surface it.
      const message = (err instanceof Error ? err.message : String(err)).replaceAll(token, "***");
      throw new Error(`git clone of ${WEBSITE_REPO} failed: ${message}`);
    }

    const installShPath = join(workDir, "install.sh");
    const current = readFileSync(installShPath, "utf8");
    const updated = pinVersion(current, version);

    if (current === updated) {
      console.log("install.sh is already pinned to this version -- nothing to push.");
      return;
    }
    writeFileSync(installShPath, updated);

    await $`git -C ${workDir} add install.sh`.quiet();
    const staged = await $`git -C ${workDir} diff --cached --quiet`.quiet().nothrow();
    if (staged.exitCode === 0) {
      console.log("install.sh is already pinned to this version -- nothing to push.");
      return;
    }

    const tag = normalizeTag(version);
    await $`git -C ${workDir} -c user.name="nodespace-release-bot" -c user.email="release-bot@nodespace.app" commit -m ${`Pin install.sh to ${tag} (automated release sync)`}`.quiet();
    await $`git -C ${workDir} push origin HEAD:main`.quiet();
    console.log(`Pushed install.sh version pin (${tag}) to ${WEBSITE_REPO}.`);
  } finally {
    // Also closes the window where a temp credential URL sits in
    // workDir/.git/config in cleartext -- removed on every path.
    rmSync(workDir, { recursive: true, force: true });
  }
}

export const LIVE_INSTALL_SCRIPT_URL = "https://nodespace.ai/install.sh";

/** Extracts the pinned tag from install.sh content, or null if there is no
 * pin line. Pure -- no network. */
export function extractPin(installShContent: string): string | null {
  const line = installShContent.match(VERSION_PIN_RE)?.[0];
  if (!line) return null;
  return line.match(/"([^"]+)"/)?.[1] ?? null;
}

export type InstallScriptDrift =
  | { kind: "ok" }
  | { kind: "repo-stale"; repoPin: string | null; livePin: string | null }
  | { kind: "deploy-stale"; repoPin: string; livePin: string | null };

/** Pure decision logic. Two distinct failures, because the fix differs:
 * `repo-stale` means the release sync never advanced the pin in the website
 * repo (rerun this script with --push); `deploy-stale` means the repo is
 * current but the live site serves something older (the website's hosting
 * is not deploying its main branch -- nothing to fix from this repo).
 * `repoPin` is null when the repo copy could not be checked (no token), in
 * which case only the live copy is compared. */
export function classifyInstallScriptDrift(
  repoPin: string | null | undefined,
  livePin: string | null,
  latestTag: string,
): InstallScriptDrift {
  const latest = normalizeTag(latestTag);
  const norm = (p: string | null | undefined) => (p ? normalizeTag(p) : null);
  if (repoPin !== undefined && norm(repoPin) !== latest) {
    return { kind: "repo-stale", repoPin: norm(repoPin), livePin: norm(livePin) };
  }
  if (norm(livePin) !== latest) {
    return { kind: "deploy-stale", repoPin: latest, livePin: norm(livePin) };
  }
  return { kind: "ok" };
}

async function fetchLiveInstallScript(): Promise<string> {
  // Cache-buster + no-store: the whole point is to see what a fresh
  // `curl | sh` would get, not a cached copy.
  const res = await fetch(`${LIVE_INSTALL_SCRIPT_URL}?drift-check=${Date.now()}`, {
    cache: "no-store",
  });
  if (!res.ok) throw new Error(`failed to fetch ${LIVE_INSTALL_SCRIPT_URL}: HTTP ${res.status}`);
  return res.text();
}

async function runDriftCheck(): Promise<void> {
  // Genuine drift and "the check itself failed to run" both exit 1 but are
  // worded differently so an auto-filed issue can be triaged from its text.
  let repoPin: string | null | undefined;
  let livePin: string | null;
  let latest: string;
  try {
    const token = process.env.WEBSITE_DEPLOY_TOKEN;
    const [latestOut, live, repo] = await Promise.all([
      $`gh release list --repo ${CORE_REPO} --json tagName,isLatest --jq '.[] | select(.isLatest) | .tagName'`.text(),
      fetchLiveInstallScript(),
      // The website repo is private, so its copy is only checkable with a
      // token; without one, only the live copy is compared.
      token ? fetchWebsiteInstallScript(token) : Promise.resolve(undefined),
    ]);
    latest = latestOut.trim();
    if (!latest) throw new Error("could not determine the latest release from `gh release list`");
    livePin = extractPin(live);
    repoPin = repo === undefined ? undefined : extractPin(repo);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`DRIFT CHECK ERROR (not necessarily drift -- the check itself failed to run): ${message}`);
    process.exit(1);
  }

  const r = classifyInstallScriptDrift(repoPin, livePin, latest);
  if (r.kind === "ok") {
    console.log(`install.sh in sync: ${normalizeTag(latest)}`);
    return;
  }
  if (r.kind === "repo-stale") {
    console.error(
      `INSTALL.SH PIN DRIFT: ${WEBSITE_REPO}'s install.sh is pinned to ${r.repoPin ?? "(no pin found)"}, ` +
        `but the latest published release is ${normalizeTag(latest)}.\n` +
        `Fix: bun run scripts/publish-install-script.ts ${latest} --push (requires WEBSITE_DEPLOY_TOKEN)`,
    );
  } else {
    console.error(
      `INSTALL.SH DEPLOY DRIFT: ${WEBSITE_REPO}'s install.sh is pinned correctly to ${r.repoPin}` +
        `${repoPin === undefined ? " (assumed; repo copy not checked, no token)" : ""}, ` +
        `but ${LIVE_INSTALL_SCRIPT_URL} serves ${r.livePin ?? "(no pin found)"}. ` +
        "The site's hosting is not deploying the repo's main branch -- check the Cloudflare Pages " +
        "project's Git integration (Settings -> Builds & deployments). Nothing to fix in nodespace-core.",
    );
  }
  process.exit(1);
}

function usage(): void {
  console.log(`Usage:
  bun run scripts/publish-install-script.ts <version> [--push]
  bun run scripts/publish-install-script.ts drift-check`);
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const command = args[0];

  if (command === "drift-check") {
    await runDriftCheck();
    return;
  }

  if (!command || !/^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$/.test(command)) {
    usage();
    process.exit(1);
  }

  const push = args.includes("--push");
  const token = process.env.WEBSITE_DEPLOY_TOKEN;
  // nodespace-website is private, so even the read-only preview fetch below
  // needs this token -- not just --push's actual write.
  if (!token) {
    console.error(
      "WEBSITE_DEPLOY_TOKEN is not set (a PAT with contents:write on " +
        `${WEBSITE_REPO}) -- required even for a dry run, since nodespace-website is private.`,
    );
    process.exit(1);
  }

  // Independent network calls (GitHub API for this repo's release assets,
  // raw.githubusercontent.com for the website's current install.sh) --
  // run concurrently rather than paying the sum of both round trips.
  const [{ missing }, currentPreview] = await Promise.all([
    checkReleaseAssets(command),
    fetchWebsiteInstallScript(token),
  ]);
  if (missing.length > 0) {
    console.error(
      `✗ ${normalizeTag(command)} is missing expected headless CLI assets, refusing to pin: ` +
        missing.map((t) => `nodespace-${t}`).join(", "),
    );
    process.exit(1);
  }

  const updatedPreview = pinVersion(currentPreview, command);
  if (currentPreview === updatedPreview) {
    console.log(`install.sh is already pinned to ${normalizeTag(command)} -- nothing to do.`);
    return;
  }

  console.log(`--- NODESPACE_CLI_VERSION diff (preview) ---`);
  console.log(currentPreview.match(VERSION_PIN_RE)?.[0] ?? "(no existing pin found)");
  console.log("->");
  console.log(updatedPreview.match(VERSION_PIN_RE)?.[0]);

  if (!push) {
    console.log("(dry run -- pass --push with WEBSITE_DEPLOY_TOKEN set to publish this)");
    return;
  }
  // Re-reads and re-patches from a fresh clone rather than reusing
  // currentPreview/updatedPreview above -- see pushInstallScriptUpdate's
  // docstring.
  await pushInstallScriptUpdate(command, token as string);
}

if (import.meta.main) {
  // Matches scripts/test-gate.ts's / the Homebrew cask sync's convention:
  // a bare uncaught rejection here would otherwise print a raw Bun stack
  // trace instead of an operator-facing message.
  try {
    await main();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`✗ ${message}`);
    process.exit(1);
  }
}
