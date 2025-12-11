#!/usr/bin/env bun

/**
 * NodeSpace Release Management Script
 *
 * Usage:
 *   bun run release                     # Interactive release (prompts for version)
 *   bun run release v0.1.0              # Create release with specified version
 *   bun run release v0.1.0 --draft      # Create as draft (doesn't trigger builds)
 *   bun run release:list                # List recent releases
 *   bun run release:watch               # Watch build progress
 */

import { readFileSync, writeFileSync } from "fs";
import path from "path";

const OWNER = "malibio";
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
 * Update version in tauri.conf.json and Cargo.toml
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

  // Update root package.json
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
| macOS (Apple Silicon) | \`NodeSpace_${v}_aarch64.dmg\` | For M1/M2/M3 Macs |
| macOS (Intel) | \`NodeSpace_${v}_x64.dmg\` | For Intel Macs |
| Windows | \`NodeSpace_${v}_x64-setup.exe\` | Windows installer |
| Windows | \`NodeSpace_${v}_x64.msi\` | Windows MSI package |
| Linux | \`NodeSpace_${v}_amd64.deb\` | Debian/Ubuntu package |
| Linux | \`NodeSpace_${v}_amd64.AppImage\` | Universal Linux binary |

### What's New

<!-- Add release highlights here -->

### Installation

Download the appropriate file for your platform from the assets below.
`;
}

/**
 * Create a GitHub release
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
    console.log("📝 Creating as draft (won't trigger builds until published)");
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
    console.log("   Builds will run for: macOS (ARM + Intel), Windows, Linux");
    console.log("\n📊 Watch progress:");
    console.log("   bun run release:watch");
    console.log(`   Or visit: https://github.com/${OWNER}/${REPO}/actions`);
  } else {
    console.log("\n📝 Draft release created.");
    console.log("   Publish it to trigger builds:");
    console.log(`   gh release edit ${version} --draft=false`);
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
  bun run release v0.1.0 --draft      # Create draft release
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

📊 After creating a release:
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
