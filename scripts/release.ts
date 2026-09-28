#!/usr/bin/env bun

/**
 * NodeSpace Release Management Script
 *
 * Usage:
 *   bun run release                     # Interactive release (prompts for version)
 *   bun run release v0.1.0              # Create release with specified version
 *   bun run release v0.1.0 --draft      # Create as draft (builds run once published)
 *   bun run release v0.1.0 --skip-perf  # Skip the pre-release performance benchmarks
 *   bun run release:list                # List recent releases
 *   bun run release:watch               # Watch build progress
 */

import { readFileSync, writeFileSync } from "fs";
import path from "path";

const OWNER = "NodeSpaceAI";
const REPO = "nodespace-core";

interface ReleaseConfig {
  version: string;
  title?: string;
  notes?: string;
  draft?: boolean;
  prerelease?: boolean;
}

/**
 * Get current version from tauri.conf.json
 */
function getCurrentVersion(): string {
  const tauriConfigPath = path.join(process.cwd(), "packages/desktop-app/src-tauri/tauri.conf.json");
  const config = JSON.parse(readFileSync(tauriConfigPath, "utf-8"));
  return config.version;
}

/**
 * Update version in tauri.conf.json, Cargo.toml, and both package.json files.
 *
 * tauri.conf.json, packages/desktop-app/package.json, and src-tauri/Cargo.toml
 * are the three files scripts/check-version-sync.ts enforces as canonical --
 * the pre-push gate fails the release commit itself
 * if any of them drift. This function used to update the ROOT package.json
 * instead of packages/desktop-app/package.json, which isn't one of the three
 * check-version-sync.ts actually compares -- every release silently left the
 * real canonical sibling stale until the next push happened to touch it.
 *
 * The root Cargo.toml's [workspace.package] version is also updated here.
 * Every Rust workspace member crate (agent, cli, core, daemon, nlp-engine,
 * nodespace-types, proto) inherits it via `version.workspace = true` instead
 * of hardcoding its own, so this one edit keeps all of them in sync -- this
 * is what nodespaced's `--version` flag and its `get_daemon_version` gRPC RPC
 * report at runtime via `env!("CARGO_PKG_VERSION")`, so leaving it stale here
 * means those surfaces silently lie about the running build's real version.
 * check-version-sync.ts enforces this field the same way as the other three.
 */
function updateVersion(newVersion: string): void {
  // Remove 'v' prefix if present
  const version = newVersion.replace(/^v/, "");

  // Update tauri.conf.json
  const tauriConfigPath = path.join(process.cwd(), "packages/desktop-app/src-tauri/tauri.conf.json");
  const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf-8"));
  tauriConfig.version = version;
  writeFileSync(tauriConfigPath, JSON.stringify(tauriConfig, null, 2) + "\n");
  console.log(`✅ Updated tauri.conf.json to ${version}`);

  // Update Cargo.toml in src-tauri (target version under [package] section)
  const cargoPath = path.join(process.cwd(), "packages/desktop-app/src-tauri/Cargo.toml");
  let cargoContent = readFileSync(cargoPath, "utf-8");
  // More robust regex: only replace version in [package] section, not dependency versions
  cargoContent = cargoContent.replace(
    /(\[package\][\s\S]*?)version = ".*?"/m,
    `$1version = "${version}"`
  );
  writeFileSync(cargoPath, cargoContent);
  console.log(`✅ Updated src-tauri/Cargo.toml to ${version}`);

  // Update the root Cargo.toml's [workspace.package] version -- every other
  // Rust workspace member crate inherits from this field.
  const workspaceCargoPath = path.join(process.cwd(), "Cargo.toml");
  let workspaceCargoContent = readFileSync(workspaceCargoPath, "utf-8");
  workspaceCargoContent = workspaceCargoContent.replace(
    /(\[workspace\.package\][\s\S]*?)version = ".*?"/m,
    `$1version = "${version}"`
  );
  writeFileSync(workspaceCargoPath, workspaceCargoContent);
  console.log(`✅ Updated Cargo.toml [workspace.package] to ${version}`);

  // Update packages/desktop-app/package.json -- the canonical sibling
  // check-version-sync.ts actually checks.
  const appPackageJsonPath = path.join(process.cwd(), "packages/desktop-app/package.json");
  const appPackageJson = JSON.parse(readFileSync(appPackageJsonPath, "utf-8"));
  appPackageJson.version = version;
  writeFileSync(appPackageJsonPath, JSON.stringify(appPackageJson, null, 2) + "\n");
  console.log(`✅ Updated packages/desktop-app/package.json to ${version}`);

  // Update root package.json too, for monorepo-wide consistency -- not part
  // of check-version-sync.ts's canonical set, but there's no reason to leave
  // it stale.
  const packageJsonPath = path.join(process.cwd(), "package.json");
  const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf-8"));
  packageJson.version = version;
  writeFileSync(packageJsonPath, JSON.stringify(packageJson, null, 2) + "\n");
  console.log(`✅ Updated package.json to ${version}`);
}

/**
 * Validate version format
 */
function validateVersion(version: string): boolean {
  const versionRegex = /^v?\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$/;
  return versionRegex.test(version);
}

/**
 * Generate the fixed part of the release notes: the version heading, the
 * downloads table, and the installation blurb.
 *
 * This used to also contain a hand-written "### What's New" section, frozen
 * at the moment it was written (v0.1.4-alpha's table-nodes/SurrealDB-3.x/
 * Intel-Mac feature list) and never updated again -- every release that
 * didn't pass --notes/--notes-file shipped that same stale text to the
 * public GitHub Releases page regardless of what actually changed. There is
 * no reliable way to hand-maintain a changelog fragment that a script only
 * touches once; the actual change list now comes from GitHub itself via
 * `--generate-notes` (see buildReleaseCreateArgs), which derives it from
 * real merged-PR history each time, so it can never go stale here.
 */
function generateReleaseNotes(version: string): string {
  const v = version.replace(/^v/, "");
  return `## NodeSpace ${version}

### Downloads

| Platform | File | Description |
|----------|------|-------------|
| macOS (Apple Silicon) | \`NodeSpace_${v}_aarch64.dmg\` | For M1/M2/M3 Macs -- the only supported macOS target |
| Windows | \`NodeSpace_${v}_x64-setup.exe\` | Windows installer |
| Windows | \`NodeSpace_${v}_x64.msi\` | Windows MSI package |

### Installation

Download the appropriate file for your platform from the assets below.
`;
}

/**
 * Build the argument list for `gh release create`.
 *
 * Pulled out of createRelease() so the notes-source decision -- an
 * operator-supplied --notes/--notes-file vs. GitHub's own generated
 * "What's Changed" list -- can be unit tested without invoking `gh` or
 * touching the network.
 */
function buildReleaseCreateArgs(config: ReleaseConfig): string[] {
  const version = config.version.startsWith("v") ? config.version : `v${config.version}`;
  const title = config.title || `NodeSpace ${version}`;
  const notes = config.notes || generateReleaseNotes(version);

  const args = ["gh", "release", "create", version, "--title", title, "--notes", notes];

  if (!config.notes) {
    // No explicit --notes/--notes-file: let GitHub's release-notes API
    // supply the real "What's Changed" list (merged PRs + contributors
    // since the previous tag). `--notes` and `--generate-notes` combine --
    // gh prepends the downloads table above to GitHub's generated notes
    // rather than replacing it.
    args.push("--generate-notes");
  }

  if (config.draft) args.push("--draft");
  if (config.prerelease) args.push("--prerelease");

  return args;
}

/**
 * Create a GitHub release.
 *
 * Both paths this produces -- a draft that is published later, and a release
 * created non-draft in one shot -- start the release build pipeline, because
 * .github/workflows/release.yml subscribes to the `published` release
 * activity type. It previously subscribed to `created`, which fires only for
 * a release published without having been a draft first; publishing a draft
 * built nothing, silently, and this script printed the opposite. Keep the
 * messages below in sync with that workflow's `on.release.types`.
 */
async function createRelease(config: ReleaseConfig): Promise<void> {
  const version = config.version.startsWith("v") ? config.version : `v${config.version}`;

  console.log(`\n🚀 Creating release ${version}...\n`);

  const args = buildReleaseCreateArgs(config);

  if (config.draft) {
    console.log("📝 Creating as draft (builds start when you publish it)");
  }

  const result = Bun.spawnSync(args, {
    stdout: "pipe",
    stderr: "pipe"
  });

  if (result.exitCode !== 0) {
    const error = result.stderr.toString();
    throw new Error(`Failed to create release: ${error}`);
  }

  const output = result.stdout.toString().trim();
  console.log(`✅ Release created: ${output}`);

  if (!config.draft) {
    console.log("\n🔨 GitHub Actions workflow triggered!");
    console.log("   Builds will run for: macOS (Apple Silicon), Windows, Linux");
    console.log("\n📊 Watch progress:");
    console.log("   bun run release:watch");
    console.log(`   Or visit: https://github.com/${OWNER}/${REPO}/actions`);
  } else {
    console.log("\n📝 Draft release created. Review it, then publish to start builds:");
    console.log(`   gh release edit ${version} --draft=false`);
    console.log("\n📊 Once published, watch progress:");
    console.log("   bun run release:watch");
    console.log(`   Or visit: https://github.com/${OWNER}/${REPO}/actions`);
  }
}

/** Lines of `test:perf` output printed when the benchmarks fail. */
const PERF_OUTPUT_TAIL_LINES = 40;

/**
 * Pull the failing benchmarks out of vitest's output: its "Failed Tests"
 * summary names each one on a `FAIL  <file> > <suite> > <test>` line.
 */
function failingBenchmarks(output: string): string[] {
  return output
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("FAIL "))
    .map((line) => line.slice("FAIL ".length).trim());
}

/**
 * Run the wall-clock performance benchmarks (`test:perf`, full scale).
 *
 * They run here, on the releaser's machine just before a release, and never in
 * the push check or merge gate: under machine contention their timings are
 * unbounded, so a gate would fail them for reasons unrelated to the change.
 * GitHub's shared runners are noisy for the same reason, so release.yml does
 * not run them either. Returns whether they passed; on failure prints the
 * failing benchmarks and the tail of the output.
 */
function runPerfBenchmarks(): boolean {
  console.log("\n⏱️  Running performance benchmarks (test:perf)...");
  const result = Bun.spawnSync(["bun", "run", "--cwd", "packages/desktop-app", "test:perf"], {
    stdout: "pipe",
    stderr: "pipe",
    env: { ...process.env, NO_COLOR: "1" }
  });

  if (result.exitCode === 0) {
    console.log("✅ Performance benchmarks passed");
    return true;
  }

  const output = `${result.stdout.toString()}\n${result.stderr.toString()}`;
  const failures = failingBenchmarks(output);
  console.error("\n❌ Performance benchmarks failed:");
  for (const failure of failures) {
    console.error(`   - ${failure}`);
  }
  console.error(`\n--- last ${PERF_OUTPUT_TAIL_LINES} lines of test:perf output ---`);
  console.error(output.trimEnd().split("\n").slice(-PERF_OUTPUT_TAIL_LINES).join("\n"));
  return false;
}

/**
 * List recent releases
 */
async function listReleases(): Promise<void> {
  const result = Bun.spawnSync(["gh", "release", "list", "--limit", "10"], {
    stdout: "pipe",
    stderr: "pipe"
  });

  if (result.exitCode !== 0) {
    throw new Error("Failed to list releases");
  }

  console.log("\n📋 Recent Releases:\n");
  console.log(result.stdout.toString());
}

/**
 * Watch the latest workflow run
 */
async function watchWorkflow(): Promise<void> {
  console.log("\n👀 Watching latest release workflow...\n");

  const result = Bun.spawnSync(["gh", "run", "watch"], {
    stdout: "inherit",
    stderr: "inherit"
  });

  if (result.exitCode !== 0) {
    console.log("\n⚠️  No active runs or watch failed.");
    console.log(`   Check: https://github.com/${OWNER}/${REPO}/actions`);
  }
}

/**
 * View a specific release
 */
async function viewRelease(version: string): Promise<void> {
  const result = Bun.spawnSync(["gh", "release", "view", version], {
    stdout: "pipe",
    stderr: "pipe"
  });

  if (result.exitCode !== 0) {
    throw new Error(`Release ${version} not found`);
  }

  console.log(result.stdout.toString());
}

/**
 * Delete a release
 */
async function deleteRelease(version: string): Promise<void> {
  console.log(`\n⚠️  Deleting release ${version}...`);

  const result = Bun.spawnSync(["gh", "release", "delete", version, "--yes"], {
    stdout: "pipe",
    stderr: "pipe"
  });

  if (result.exitCode !== 0) {
    throw new Error(`Failed to delete release ${version}`);
  }

  console.log(`✅ Release ${version} deleted`);
}

// CLI Interface
async function main() {
  const args = process.argv.slice(2);
  const command = args[0];

  try {
    switch (command) {
      case "list": {
        await listReleases();
        break;
      }

      case "watch": {
        await watchWorkflow();
        break;
      }

      case "view": {
        const version = args[1];
        if (!version) {
          console.error("Usage: bun run scripts/release.ts view v0.1.0");
          process.exit(1);
        }
        await viewRelease(version);
        break;
      }

      case "delete": {
        const version = args[1];
        if (!version) {
          console.error("Usage: bun run scripts/release.ts delete v0.1.0");
          process.exit(1);
        }
        await deleteRelease(version);
        break;
      }

      case "help":
      case "--help":
      case "-h": {
        console.log(`
🚀 NodeSpace Release Manager

📦 Create a Release:
  bun run release v0.1.0              # Create release (triggers builds)
  bun run release v0.1.0 --draft      # Create draft (builds run once published)
  bun run release v0.1.0 --prerelease # Mark as pre-release
  bun run release v0.1.0 --title "Custom Title"
  bun run release v0.1.0 --notes "Custom release notes"
  bun run release v0.1.0 --notes-file CHANGELOG.md
  bun run release v0.1.0 --skip-perf  # Skip the performance benchmarks

  Creating a release first runs the performance benchmarks (test:perf) on
  this machine, and refuses to release if any fail. They are not part of the
  push check or merge gate, so this is where they run. Pass --skip-perf only
  for a run you know was noisy (e.g. another build was running).

📋 Manage Releases:
  bun run release:list                # List recent releases
  bun run release:watch               # Watch build progress
  bun run release:view v0.1.0         # View release details
  bun run release:delete v0.1.0       # Delete a release

🔧 Version Management:
  bun run release:bump v0.2.0         # Update version in config files

📊 After a release is published (created non-draft, or a draft you publish):
  - GitHub Actions automatically builds for all platforms
  - Installers are attached to the release when builds complete
  - Users can download from: https://github.com/${OWNER}/${REPO}/releases
        `);
        break;
      }

      case "bump": {
        const version = args[1];
        if (!version || !validateVersion(version)) {
          console.error("Usage: bun run scripts/release.ts bump v0.2.0");
          console.error("Version must be in format: v1.2.3 or 1.2.3");
          process.exit(1);
        }
        updateVersion(version);
        console.log("\n💡 Don't forget to commit these changes:");
        console.log("   git add -A && git commit -m 'Bump version to " + version + "'");
        break;
      }

      default: {
        // Default: create release with provided version
        let version = command;

        if (!version) {
          // Show current version and prompt for new one
          const currentVersion = getCurrentVersion();
          console.log(`Current version: ${currentVersion}`);
          console.log("\nUsage: bun run release <version> [options]");
          console.log("\nExamples:");
          console.log("  bun run release v0.1.0");
          console.log("  bun run release v0.1.0 --draft");
          console.log("\nRun 'bun run release --help' for all options");
          process.exit(1);
        }

        if (!validateVersion(version)) {
          console.error(`Invalid version format: ${version}`);
          console.error("Version must be in format: v1.2.3 or 1.2.3");
          process.exit(1);
        }

        const config: ReleaseConfig = { version };

        // Parse flags
        if (args.includes("--draft")) config.draft = true;
        if (args.includes("--prerelease")) config.prerelease = true;

        const titleIndex = args.indexOf("--title");
        if (titleIndex !== -1 && args[titleIndex + 1]) {
          config.title = args[titleIndex + 1];
        }

        const notesIndex = args.indexOf("--notes");
        if (notesIndex !== -1 && args[notesIndex + 1]) {
          config.notes = args[notesIndex + 1];
        }

        const notesFileIndex = args.indexOf("--notes-file");
        if (notesFileIndex !== -1 && args[notesFileIndex + 1]) {
          config.notes = readFileSync(args[notesFileIndex + 1], "utf-8");
        }

        // Benchmarks run first, before the version bump is committed and
        // pushed, so a failure leaves nothing to undo.
        if (args.includes("--skip-perf")) {
          console.log("⚠️  Skipping performance benchmarks (--skip-perf)");
        } else if (!runPerfBenchmarks()) {
          console.error("\nRefusing to release. If the machine was busy, rerun on a quiet one;");
          console.error("if you know the run was noisy, override with --skip-perf.");
          process.exit(1);
        }

        // Update version in config files before creating release
        console.log("📝 Updating version in config files...");
        updateVersion(version);

        // Re-resolve Cargo.lock so its own self-referential workspace-member
        // version entries (nodespace-app, nodespace-cli, nodespace-agent, ...)
        // match the Cargo.toml files updateVersion() just edited. `cargo check`
        // is enough to trigger Cargo's normal lockfile re-sync and is far
        // cheaper than a full build; it does not touch external dependency
        // versions unless a requirement range actually changed.
        console.log("🔒 Re-syncing Cargo.lock...");
        Bun.spawnSync(["cargo", "check", "--workspace"], { stdout: "inherit", stderr: "inherit" });

        // bun.lock carries the same kind of self-referential version entry
        // for packages/desktop-app (its own workspace member record) that
        // updateVersion() just edited in package.json. `bun install` is a
        // fast no-op besides that resync since no actual dependency changed.
        console.log("🔒 Re-syncing bun.lock...");
        Bun.spawnSync(["bun", "install"], { stdout: "inherit", stderr: "inherit" });

        // Check if there are uncommitted changes
        const statusResult = Bun.spawnSync(["git", "status", "--porcelain"], {
          stdout: "pipe"
        });

        if (statusResult.stdout.toString().trim()) {
          console.log("\n⚠️  There are uncommitted changes (version bump).");
          console.log("   Committing version update...\n");

          // Stage only the specific version files to avoid accidentally committing unrelated work
          const versionFiles = [
            "packages/desktop-app/src-tauri/tauri.conf.json",
            "packages/desktop-app/src-tauri/Cargo.toml",
            "packages/desktop-app/package.json",
            "Cargo.toml",
            "Cargo.lock",
            "bun.lock",
            "package.json"
          ];
          for (const file of versionFiles) {
            Bun.spawnSync(["git", "add", file], { stdout: "inherit" });
          }
          Bun.spawnSync(["git", "commit", "-m", `Bump version to ${version}`], { stdout: "inherit" });
          Bun.spawnSync(["git", "push"], { stdout: "inherit" });
        }

        await createRelease(config);
        break;
      }
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`❌ Error: ${message}`);
    process.exit(1);
  }
}

if (import.meta.main) {
  main();
}

export {
  createRelease,
  listReleases,
  watchWorkflow,
  updateVersion,
  failingBenchmarks,
  generateReleaseNotes,
  buildReleaseCreateArgs
};
