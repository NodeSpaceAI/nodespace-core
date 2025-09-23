#!/usr/bin/env bun

/**
 * NodeSpace Container Management
 *
 * Automates Docker container operations for parallel development
 */

import { $ } from "bun";
import { existsSync } from "fs";
import { join } from "path";

const CONTAINER_IMAGE = "claude-code-dev";
const CONTAINER_PREFIX = "nodespace-dev";

interface ContainerInfo {
  id: string;
  name: string;
  status: string;
  image: string;
}

async function detectContainerRuntime(): Promise<{ runtime: string; available: boolean }> {
  // Check Colima first (preferred for macOS)
  try {
    const colimaResult = await $`colima status`.quiet();
    if (colimaResult.exitCode === 0) {
      return { runtime: "Colima", available: true };
    }
  } catch {}

  // Check Docker Desktop
  try {
    const dockerResult = await $`docker info`.quiet();
    if (dockerResult.exitCode === 0) {
      return { runtime: "Docker Desktop", available: true };
    }
  } catch {}

  // Check Podman
  try {
    const podmanResult = await $`podman info`.quiet();
    if (podmanResult.exitCode === 0) {
      return { runtime: "Podman", available: true };
    }
  } catch {}

  return { runtime: "none", available: false };
}

async function checkImageExists(useDocker: boolean = true): Promise<boolean> {
  try {
    const cmd = useDocker ? "docker" : "podman";
    const result = await $`${cmd} images -q ${CONTAINER_IMAGE}`.quiet();
    return result.stdout.toString().trim().length > 0;
  } catch {
    return false;
  }
}

async function buildImage(useDocker: boolean = true): Promise<void> {
  const cmd = useDocker ? "docker" : "podman";
  console.log(`🔨 Building Claude Code development container with ${cmd}...`);

  if (!existsSync("Dockerfile.dev")) {
    console.error("❌ Dockerfile.dev not found in current directory");
    process.exit(1);
  }

  try {
    await $`${cmd} build -f Dockerfile.dev -t ${CONTAINER_IMAGE} .`;
    console.log("✅ Container image built successfully!");
  } catch (error) {
    console.error("❌ Failed to build container:", error);
    process.exit(1);
  }
}

async function runContainer(options: { name?: string } = {}): Promise<void> {
  const runtime = await detectContainerRuntime();
  if (!runtime.available) {
    console.error("❌ No container runtime available. Please install one of:");
    console.error("  • Colima: brew install colima && colima start");
    console.error("  • Docker Desktop: https://docker.com/products/docker-desktop");
    console.error("  • Podman: brew install podman && podman machine init && podman machine start");
    process.exit(1);
  }

  console.log(`🐳 Using ${runtime.runtime} as container runtime`);
  const useDocker = runtime.runtime !== "Podman";

  const imageExists = await checkImageExists(useDocker);
  if (!imageExists) {
    console.log("📦 Container image not found. Building...");
    await buildImage(useDocker);
  }

  // Generate unique container name
  const timestamp = Date.now();
  const containerName = options.name || `${CONTAINER_PREFIX}-${timestamp}`;

  // Get GitHub token and config paths
  const githubToken = process.env.GITHUB_TOKEN;
  const homeDir = process.env.HOME;

  if (!githubToken) {
    console.warn("⚠️  GITHUB_TOKEN not set. Container won't have GitHub access.");
  }

  // Build container run command
  const cmd = useDocker ? "docker" : "podman";
  const dockerCmd = [
    cmd, "run", "-it", "--rm",
    "--name", containerName,
  ];

  // Add GitHub token if available
  if (githubToken) {
    dockerCmd.push("-e", `GITHUB_TOKEN=${githubToken}`);
  }

  // Mount GitHub CLI config if it exists
  const ghConfigPath = join(homeDir!, ".config", "gh");
  if (existsSync(ghConfigPath)) {
    dockerCmd.push("-v", `${ghConfigPath}:/root/.config/gh:ro`);
  }

  // Mount git config if it exists
  const gitConfigPath = join(homeDir!, ".gitconfig");
  if (existsSync(gitConfigPath)) {
    dockerCmd.push("-v", `${gitConfigPath}:/root/.gitconfig:ro`);
  }

  dockerCmd.push(CONTAINER_IMAGE);

  console.log(`🚀 Starting container: ${containerName}`);
  console.log("📝 Inside container, run: git checkout -b feature/your-feature && claude");

  try {
    await $`${dockerCmd}`;
  } catch (error) {
    console.error("❌ Failed to run container:", error);
    process.exit(1);
  }
}

async function listContainers(): Promise<void> {
  const runtime = await detectContainerRuntime();
  if (!runtime.available) {
    console.error("❌ No container runtime available");
    return;
  }

  const cmd = runtime.runtime !== "Podman" ? "docker" : "podman";

  try {
    const result = await $`${cmd} ps -a --filter name=${CONTAINER_PREFIX} --format "table {{.ID}}\\t{{.Names}}\\t{{.Status}}\\t{{.Image}}"`.quiet();

    console.log(`📋 NodeSpace Development Containers (${runtime.runtime}):`);
    if (result.stdout.toString().trim()) {
      console.log(result.stdout.toString());
    } else {
      console.log("📭 No NodeSpace development containers found");
    }
  } catch (error) {
    console.error("❌ Failed to list containers:", error);
  }
}

async function stopContainers(containerName?: string): Promise<void> {
  const runtime = await detectContainerRuntime();
  if (!runtime.available) {
    console.error("❌ No container runtime available");
    return;
  }

  const cmd = runtime.runtime !== "Podman" ? "docker" : "podman";

  try {
    if (containerName) {
      await $`${cmd} stop ${containerName}`;
      console.log(`🛑 Stopped container: ${containerName}`);
    } else {
      // Stop all NodeSpace containers
      const result = await $`${cmd} ps -q --filter name=${CONTAINER_PREFIX}`.quiet();
      const containerIds = result.stdout.toString().trim().split('\n').filter(id => id);

      if (containerIds.length > 0) {
        await $`${cmd} stop ${containerIds}`;
        console.log(`🛑 Stopped ${containerIds.length} container(s)`);
      } else {
        console.log("📭 No running NodeSpace containers to stop");
      }
    }
  } catch (error) {
    console.error("❌ Failed to stop containers:", error);
  }
}

function showHelp(): void {
  console.log(`
🐳 NodeSpace Container Management

Commands:
  bun run container:build         Build the development container image
  bun run container:run           Run a new development container
  bun run container:list          List all NodeSpace containers
  bun run container:stop [name]   Stop container(s)
  bun run container:help          Show this help

Examples:
  bun run container:build                    # Build the image
  bun run container:run                      # Start new container
  bun run container:stop nodespace-dev-123  # Stop specific container
  bun run container:stop                     # Stop all containers

Each container:
  • Has independent nodespace-core clone
  • Includes all development tools (Rust, Bun, Claude Code, GitHub CLI)
  • Can run simultaneously for parallel development
  • Automatically passes GitHub credentials from host
`);
}

// Main execution
const command = process.argv[2];

switch (command) {
  case "build":
    await buildImage();
    break;
  case "run":
    await runContainer();
    break;
  case "list":
    await listContainers();
    break;
  case "stop":
    await stopContainers(process.argv[3]);
    break;
  case "help":
  case undefined:
    showHelp();
    break;
  default:
    console.error(`❌ Unknown command: ${command}`);
    showHelp();
    process.exit(1);
}