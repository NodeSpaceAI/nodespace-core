#!/usr/bin/env bun
/**
 * Re-verify the CURRENTLY PUBLISHED macOS .pkg release asset against live
 * Gatekeeper, independent of (and any amount of time after) the release
 * build that produced it.
 *
 * Why this exists: release.yml's own pkg-build job already runs
 * `spctl --assess --type install` on the .pkg it just built -- but that
 * only proves the file passes Gatekeeper on the SAME runner,
 * seconds after notarytool accepted it and the ticket was stapled. That is
 * a materially weaker guarantee than "a real user's download will install
 * days or weeks from now": v0.3.0's published .pkg passed that build-time
 * check, yet a real end-user install attempt days later hit a hard
 * Gatekeeper rejection (`spctl --assess --type install` => rejected) even
 * though the file was provably untouched since upload and its signing
 * chain was independently confirmed still valid and non-revoked (live
 * OCSP: "good") -- `pkgutil --check-signature` and `xcrun stapler
 * validate` both kept reporting it as signed and notarized throughout.
 *
 * A .pkg installer's outer container must itself pass a live "type
 * install" Gatekeeper assessment before an install can proceed -- unlike a
 * .dmg, where Gatekeeper only ever assesses the *contained* .app, not the
 * disk image itself. That asymmetry is what makes .pkg uniquely exposed to
 * this class of after-the-fact drift (ticket propagation, Gatekeeper
 * Configuration Data staleness, certificate state, or any other
 * Apple-side factor) that a same-runner, moments-after-build check cannot
 * observe. This script re-runs the identical assessment against whatever
 * is CURRENTLY live on the release page, on a schedule
 * (verify-macos-installer.yml), so a Gatekeeper regression is caught by CI
 * within a day instead of by the next real user who tries to install.
 *
 * Usage:
 *   bun run scripts/verify-pkg-gatekeeper.ts [tag]   # defaults to the latest release
 *
 * macOS-only (spctl/pkgutil do not exist elsewhere) -- matches build-macos.ts's
 * platform guard.
 */

import { $ } from "bun";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CORE_REPO } from "./update-homebrew-cask";

export interface GatekeeperCheckResult {
  ok: boolean;
  pkgName: string;
  detail: string;
}

/** Picks the .pkg filename out of a directory listing (one name per
 * entry). Throws if more than one entry matches -- today only one macOS
 * .pkg target is ever published per release, so a second match means
 * something changed about what's published and silently picking one would
 * hide that, rather than fail loud the way update-homebrew-cask.ts's
 * resolveArchDigests does on a missing/ambiguous asset. Pure and offline
 * so it's covered by the fast unit suite -- everything else in this file
 * talks to `gh`/`spctl` and is deliberately left untested here, matching
 * update-homebrew-cask.test.ts's convention. */
export function pickPkgFilename(entries: string[]): string | undefined {
  const matches = entries.map((e) => e.trim()).filter((e) => e.endsWith(".pkg"));
  if (matches.length > 1) {
    throw new Error(`expected exactly one .pkg entry, found ${matches.length}: ${matches.join(", ")}`);
  }
  return matches[0];
}

/** Downloads the .pkg for `tag` (or the latest release, if omitted) into
 * `destDir` and returns its local path. */
export async function downloadPublishedPkg(destDir: string, tag?: string): Promise<string> {
  if (tag) {
    // Matches update-homebrew-cask.ts's normalizeVersion/fetchReleaseAssets
    // convention: accept a bare version too, not just a full `vX.Y.Z` tag.
    const normalizedTag = tag.startsWith("v") ? tag : `v${tag}`;
    await $`gh release download ${normalizedTag} --repo ${CORE_REPO} --pattern "NodeSpace_*.pkg" --dir ${destDir} --clobber`.quiet();
  } else {
    await $`gh release download --repo ${CORE_REPO} --pattern "NodeSpace_*.pkg" --dir ${destDir} --clobber`.quiet();
  }
  const out = await $`ls ${destDir}`.text();
  const pkgName = pickPkgFilename(out.split("\n"));
  if (!pkgName) {
    throw new Error(`gh release download reported success but no .pkg landed in ${destDir}`);
  }
  return join(destDir, pkgName);
}

/** Runs the exact assessment a real install attempt is subject to. Returns
 * ok:false (not a throw) on a genuine Gatekeeper rejection -- that is the
 * condition this check exists to detect, not a script error. Throws for
 * anything else (spctl killed by a signal, an unexpected exit code, etc.)
 * so a check that couldn't actually run is never reported as a
 * Gatekeeper verdict. */
export function assessGatekeeperInstall(pkgPath: string): GatekeeperCheckResult {
  const pkgName = pkgPath.split("/").pop() ?? pkgPath;
  const proc = Bun.spawnSync(["spctl", "--assess", "--type", "install", "--verbose", pkgPath], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const detail = `${proc.stdout.toString()}${proc.stderr.toString()}`.trim();
  if (proc.exitCode === 0) {
    return { ok: true, pkgName, detail };
  }
  // spctl's documented exit codes: 0 = accepted, 3 = assessment denied --
  // confirmed empirically against the actual rejected v0.3.0 .pkg this
  // check exists to catch. Any other exit code (bad invocation, spctl
  // itself crashing, killed by a signal) is not a Gatekeeper verdict.
  if (proc.exitCode === 3) {
    return { ok: false, pkgName, detail };
  }
  throw new Error(
    `spctl exited ${proc.exitCode ?? `via signal ${proc.signalCode}`} (expected 0 or 3) -- not a Gatekeeper verdict:\n${detail}`,
  );
}

function usage(): void {
  console.log(`Usage:
  bun run scripts/verify-pkg-gatekeeper.ts [tag]

Re-runs \`spctl --assess --type install\` against the published .pkg release
asset for [tag] (default: the latest release). Exits 1 if Gatekeeper
rejects it.`);
}

async function main(): Promise<void> {
  if (process.argv.includes("--help") || process.argv.includes("-h")) {
    usage();
    return;
  }
  if (process.platform !== "darwin") {
    console.error(
      `error: verify-pkg-gatekeeper only runs on macOS (this machine reports "${process.platform}"; spctl/pkgutil are macOS-only).`,
    );
    process.exit(1);
  }

  const tag = process.argv[2];
  const workDir = mkdtempSync(join(tmpdir(), "nodespace-pkg-gatekeeper-"));
  // process.exit() terminates immediately without running a pending
  // `finally` -- so the exit call is deliberately OUTSIDE the try/finally
  // below, after cleanup has already run, rather than inside it (which
  // would leak workDir on the very path -- a real rejection -- this
  // script exists to report).
  let result: GatekeeperCheckResult;
  try {
    const pkgPath = await downloadPublishedPkg(workDir, tag);
    result = assessGatekeeperInstall(pkgPath);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }

  if (result.ok) {
    console.log(`GATEKEEPER OK: ${result.pkgName} is accepted by \`spctl --assess --type install\`.`);
    return;
  }
  console.error(
    `GATEKEEPER REJECTED: ${result.pkgName} fails \`spctl --assess --type install\` -- a real ` +
      `install attempt with this exact file will be blocked. spctl output:\n${result.detail}`,
  );
  process.exit(1);
}

if (import.meta.main) {
  // Matches scripts/update-homebrew-cask.ts's convention: a bare uncaught
  // rejection here would otherwise print a raw Bun stack trace instead of
  // an operator-facing message, and would look identical to a genuine
  // Gatekeeper rejection in the Actions log.
  try {
    await main();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.error(`CHECK ERROR (not necessarily a Gatekeeper rejection -- the check itself failed to run): ${message}`);
    process.exit(1);
  }
}
