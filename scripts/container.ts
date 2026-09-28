#!/usr/bin/env bun

/**
 * NodeSpace Container Management
 *
 * Automates Docker container operations for parallel development
 */

import { $ } from "bun";
import { existsSync } from "fs";

const CONTAINER_IMAGE = "claude-code-dev:authenticated";
const CONTAINER_PREFIX = "nodespace-dev";

async function detectContainerRuntime(): Promise<{ runtime: string; available: boolean }> {
  // Check Colima first (preferred for macOS)
  try {
    const colimaResult = await $`colima status`.quiet();
    if (colimaResult.exitCode === 0) {
      return { runtime: "Colima", available: true };
    }
  } catch {
    // Colima not installed or not running — fall through to the next runtime.
  }

  // Check Docker Desktop
  try {
    const dockerResult = await $`docker info`.quiet();
    if (dockerResult.exitCode === 0) {
      return { runtime: "Docker Desktop", available: true };
    }
  } catch {
    // Docker Desktop not installed or not running — fall through to the next runtime.
  }

  // Check Podman
  try {
    const podmanResult = await $`podman info`.quiet();
    if (podmanResult.exitCode === 0) {
      return { runtime: "Podman", available: true };
    }
  } catch {
    // Podman not installed or not running — no supported runtime found.
  }

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

async function runContainer(options: { name?: string, persistent?: boolean } = {}): Promise<void> {
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

  // Fully isolated container - no host token passing

  // Get current terminal dimensions
  let terminalCols = "120";
  let terminalLines = "40";

  try {
    // Try to get actual terminal dimensions from tput
    const colsResult = await $`tput cols`.quiet();
    const linesResult = await $`tput lines`.quiet();

    if (colsResult.exitCode === 0 && linesResult.exitCode === 0) {
      terminalCols = colsResult.stdout.toString().trim();
      terminalLines = linesResult.stdout.toString().trim();
      console.log(`📐 Detected terminal size: ${terminalCols}x${terminalLines}`);
    } else {
      console.log(`📐 Using default terminal size: ${terminalCols}x${terminalLines}`);
    }
  } catch {
    console.log(`📐 Using fallback terminal size: ${terminalCols}x${terminalLines}`);
  }

  // Build container run command
  const cmd = useDocker ? "docker" : "podman";
  const dockerCmd = [
    cmd, "run", "-it",
    ...(options.persistent ? [] : ["--rm"]),
    "--name", containerName,
  ];

  // Add terminal environment variables
  dockerCmd.push("-e", `COLUMNS=${terminalCols}`);
  dockerCmd.push("-e", `LINES=${terminalLines}`);
  dockerCmd.push("-e", `TERM=xterm-256color`);

  // No GitHub token - authenticate inside container with 'gh auth login'

  // No host directory mappings - fully isolated container


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

async function saveContainer(containerName?: string): Promise<void> {
  const runtime = await detectContainerRuntime();
  if (!runtime.available) {
    console.error("❌ No container runtime available");
    return;
  }

  const cmd = runtime.runtime !== "Podman" ? "docker" : "podman";

  try {
    // Get container name if not specified (check both running and stopped)
    let targetContainer = containerName;
    if (!targetContainer) {
      const result = await $`${cmd} ps -a --filter name=${CONTAINER_PREFIX} --format "{{.Names}}"`.quiet();
      const containers = result.stdout.toString().trim().split('\n').filter(name => name);

      if (containers.length === 0) {
        console.error("❌ No NodeSpace containers found to save");
        return;
      } else if (containers.length > 1) {
        console.error("❌ Multiple containers found. Specify which one to save:");
        containers.forEach(name => console.log(`  ${name}`));
        return;
      }
      targetContainer = containers[0];
    }

    const newImageName = `${CONTAINER_IMAGE}:authenticated`;
    console.log(`💾 Saving container ${targetContainer} as ${newImageName}...`);

    await $`${cmd} commit ${targetContainer} ${newImageName}`;
    console.log(`✅ Container saved! Use CONTAINER_IMAGE=${newImageName} to run authenticated containers`);
    console.log(`   Or update CONTAINER_IMAGE in the script to use by default`);
  } catch (error) {
    console.error("❌ Failed to save container:", error);
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

async function resizeContainer(containerName?: string, cols?: string, lines?: string): Promise<void> {
  const runtime = await detectContainerRuntime();
  if (!runtime.available) {
    console.error("❌ No container runtime available");
    return;
  }

  const cmd = runtime.runtime !== "Podman" ? "docker" : "podman";
  const targetCols = cols || "120";
  const targetLines = lines || "40";

  try {
    // Get running container name if not specified
    let targetContainer = containerName;
    if (!targetContainer) {
      const result = await $`${cmd} ps --filter name=${CONTAINER_PREFIX} --format "{{.Names}}"`.quiet();
      const runningContainers = result.stdout.toString().trim().split('\n').filter(name => name);

      if (runningContainers.length === 0) {
        console.error("❌ No running NodeSpace containers found to resize");
        return;
      } else if (runningContainers.length > 1) {
        console.error("❌ Multiple containers running. Specify which one to resize:");
        runningContainers.forEach(name => console.log(`  ${name}`));
        return;
      }
      targetContainer = runningContainers[0];
    }

    console.log(`📐 Resizing container ${targetContainer} to ${targetCols}x${targetLines}...`);

    // Execute resize command inside the container
    await $`${cmd} exec ${targetContainer} bash -c "stty cols ${targetCols} rows ${targetLines} && export COLUMNS=${targetCols} && export LINES=${targetLines} && echo 'Terminal resized to ${targetCols}x${targetLines}'"`;

    console.log(`✅ Container terminal resized successfully!`);
    console.log(`   Use 'resize_terminal ${targetCols} ${targetLines}' inside the container for future sessions`);
  } catch (error) {
    console.error("❌ Failed to resize container terminal:", error);
  }
}

function showHelp(): void {
  console.log(`
🐳 NodeSpace Container Management

Commands:
  bun run container:build              Build the development container image
  bun run container:run                Run a new development container
  bun run container:save [name]        Save running container as authenticated image
  bun run container:list               List all NodeSpace containers
  bun run container:stop [name]        Stop container(s)
  bun run container:resize [name] [cols] [lines]  Resize container terminal
  bun run container:help               Show this help

Examples:
  bun run container:build                        # Build the image
  bun run container:run                          # Start new container (auto-detects terminal size)
  bun run container:resize                       # Resize current container to 120x40
  bun run container:resize mycontainer 100 50   # Resize specific container
  bun run container:save                         # Save current container with auth
  bun run container:stop nodespace-dev-123      # Stop specific container
  bun run container:stop                         # Stop all containers

Terminal Sizing:
  • Container automatically detects host terminal size on startup
  • If terminal looks wrong, use 'resize_terminal [cols] [rows]' inside container
  • Or use 'bun run container:resize' from host to fix running container

Workflow for authenticated containers:
  1. bun run container:run                   # Start container
  2. claude auth login                       # Authenticate inside container
  3. bun run container:save                  # Save as authenticated image
  4. Update CONTAINER_IMAGE to use saved image

Each container:
  • Has independent nodespace-core clone
  • Includes all development tools (Rust, Bun, Claude Code, GitHub CLI)
  • Can run simultaneously for parallel development
  • Automatically passes terminal size from host
  • Does NOT pass GitHub credentials from host (fully isolated) — see
    "Workflow for authenticated containers" above to authenticate inside one
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
  case "run:setup":
    await runContainer({ persistent: true });
    break;
  case "save":
    await saveContainer(process.argv[3]);
    break;
  case "list":
    await listContainers();
    break;
  case "stop":
    await stopContainers(process.argv[3]);
    break;
  case "resize":
    await resizeContainer(process.argv[3], process.argv[4], process.argv[5]);
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