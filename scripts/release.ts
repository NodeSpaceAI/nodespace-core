#!/usr/bin/env bun

/**
 * NodeSpace Release Management Script
 *
 * Usage:
 *   bun run release                     # Interactive release (prompts for version)
 *   bun run release v0.1.0              # Create release with specified version
 *   bun run release v0.1.0 --draft      # Create as draft (builds run once published)
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
 * Generate release notes
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

### What's New in v0.1.4-alpha

#### New Node Types
- **Table nodes** — create structured tables inside your graph
- **Horizontal line nodes** — visual separators for organizing content
- **Checkbox nodes** — document-local checklist items distinct from tasks

#### Rich Content Rendering
- **Syntax highlighting** for code blocks with theme-aware colors
- **Mermaid diagram rendering** — inline SVG diagrams in notes

#### App Configuration & Database Management
- New configuration UI for managing databases
- Hot-swappable database connections — switch databases without restarting

#### Smart Navigation
- Non-root node links now scroll to the target node
- Fixed task node viewer opening from nested links

#### Performance & Stability
- ~1000x faster node updates in embedded SurrealDB
- **SurrealDB 3.x upgrade**
- Fixed Metal GPU crashes on app quit (macOS)
- Fixed CREATE operation race condition on rapid typing

#### Backlinks
- Immediate backlinks reactivity when creating @mentions
- Backlinks panel component with full test coverage

### Installation

Download the appropriate file for your platform from the assets below.
`;
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
  const title = config.title || `NodeSpace ${version}`;
  const notes = config.notes || generateReleaseNotes(version);

  console.log(`\n🚀 Creating release ${version}...\n`);

  // Build gh release create command
  const args = ["gh", "release", "create", version, "--title", title, "--notes", notes];

  if (config.draft) {
    args.push("--draft");
    console.log("📝 Creating as draft (builds start when you publish it)");
  }

  if (config.prerelease) {
    args.push("--prerelease");
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

export { createRelease, listReleases, watchWorkflow, updateVersion };
