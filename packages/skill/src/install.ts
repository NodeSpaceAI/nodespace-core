#!/usr/bin/env node
// Invoked directly via `bun <path-to-this-file> <command> [agent]` — the
// desktop app's bundled installer runs it this way (see
// packages/desktop-app/src-tauri/src/skill_setup.rs), and it's the same
// invocation for a manual run from a source checkout. Never `npx`/`npm`:
// `@nodespaceai/skill` is not published to npm (the public
// NodeSpaceAI/nodespace-skill repo is the distribution channel for external
// harnesses that want the skill outside the app — see packages/skill/README.md).
//
// Also compiled directly (`bun build --compile`) into a standalone binary
// with no bun/node dependency at all — see the desktop app's
// `resolve_installer_path`/`run_skill_installer` in skill_setup.rs. `main()`
// stays behind `import.meta.main` so this module is safely importable for
// unit tests (of `extractResourceRoot`) without triggering the CLI's own
// argv parsing and `process.exit` calls as a side effect of the import.
import { install, uninstall, checkInstalled } from './installer.js';
import type { AgentName, InstallResult } from './types.js';
import { AGENTS } from './agents.js';
import { installMcp, uninstallMcp, checkMcpInstalled } from './mcp-installer.js';
import type { McpClientName } from './mcp-clients.js';
import { MCP_CLIENTS } from './mcp-clients.js';

/**
 * The reason text printed on a "⚠ agent: ..." line -- pulled out to a pure
 * function (like `extractResourceRoot` below) so it's unit-testable without
 * spawning the CLI. `result.skipReason` distinguishes a real precedence
 * decision (Claude Code's plugin marketplace already owns the skill) from
 * the generic case (the agent was detected but the package has nothing to
 * install for it), which need different messages -- the first is expected
 * and not actionable, the second is a packaging bug.
 */
export function skipReasonText(result: InstallResult): string {
  return result.skipReason === 'plugin-managed'
    ? 'already installed via the Claude Code plugin marketplace, not overwriting'
    : 'detected but no files to install (package may be incomplete)';
}

/**
 * Pull `--resource-root <path>` out of argv, wherever it appears, and return
 * the remaining positional args separately. Needed for the compiled
 * standalone-binary distribution (see `installer.ts`'s `PACKAGE_ROOT` doc
 * comment): a `bun build --compile` executable has no source-relative
 * sibling directory containing SKILL.md/shims/references the way a plain
 * `dist/install.js` does, so the caller (skill_setup.rs, for the compiled
 * binary; a person, for a manual run) must say where those files actually
 * are. Absent entirely for the existing `bun`/`node` + dist/install.js
 * invocation, which keeps resolving PACKAGE_ROOT from its own file location
 * exactly as before.
 */
export function extractResourceRoot(argv: string[]): { rest: string[]; resourceRoot?: string } {
  const rest: string[] = [];
  let resourceRoot: string | undefined;
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--resource-root') {
      resourceRoot = argv[++i];
    } else {
      rest.push(argv[i]);
    }
  }
  return { rest, resourceRoot };
}

/**
 * The reason text printed on a "⚠ client: ..." line for an MCP
 * install/uninstall outcome, parallel to [`skipReasonText`] above.
 */
export function mcpSkipReasonText(reason: string | undefined): string {
  return reason ?? 'detected but nothing to configure';
}

function main(): void {
  const { rest: positional, resourceRoot } = extractResourceRoot(process.argv.slice(2));
  const command = positional[0];
  const agentArg = positional[1] as AgentName | undefined;

  const validAgents = AGENTS.map(a => a.name);
  const validMcpClients = MCP_CLIENTS.map(c => c.name);

  function isValidAgent(name: string): name is AgentName {
    return validAgents.includes(name as AgentName);
  }

  function isValidMcpClient(name: string): name is McpClientName {
    return validMcpClients.includes(name as McpClientName);
  }

  function printUsage(): void {
    console.log(`Usage: bun install.js <command> [agent|client] [--resource-root <path>]

Commands:
  install [agent]        Install NodeSpace skill for detected (or specified) agents
  uninstall [agent]      Remove NodeSpace skill from detected (or specified) agents
  status [agent]         Report which (of the specified, or every configured) agents
                          actually have SKILL.md on disk right now -- a pure
                          filesystem check, no install/uninstall side effects
  mcp-install [client]   Configure a detected (or specified) bash-less MCP
                          client to launch \`nodespace mcp\`
  mcp-uninstall [client] Remove NodeSpace's entry from a detected (or
                          specified) MCP client's config
  mcp-status [client]    Report which (of the specified, or every configured)
                          MCP clients currently have a config entry -- a pure
                          filesystem check, no install/uninstall side effects

Agents: ${validAgents.join(', ')}
MCP clients: ${validMcpClients.join(', ')}

--resource-root <path>  Where SKILL.md/shims/references actually live. Only
                         needed by the compiled standalone binary distribution
                         — a plain dist/install.js run finds these next to
                         itself automatically. Ignored by the mcp-* commands,
                         which have no such resources.

Examples:
  bun install.js install
  bun install.js install claude-code
  bun install.js uninstall
  bun install.js mcp-install
  nodespace-skill-installer install --resource-root /path/to/resources/skill`);
  }

  if (!command || command === '--help' || command === '-h') {
    printUsage();
    process.exit(0);
  }

  const isMcpCommand = command.startsWith('mcp-');
  const mcpClientArg = positional[1] as McpClientName | undefined;

  if (!isMcpCommand && agentArg && !isValidAgent(agentArg)) {
    console.error(`Unknown agent: ${agentArg}`);
    console.error(`Valid agents: ${validAgents.join(', ')}`);
    process.exit(1);
  }
  if (isMcpCommand && mcpClientArg && !isValidMcpClient(mcpClientArg)) {
    console.error(`Unknown MCP client: ${mcpClientArg}`);
    console.error(`Valid MCP clients: ${validMcpClients.join(', ')}`);
    process.exit(1);
  }

  const targetAgents = agentArg ? [agentArg] : undefined;
  const targetMcpClients = mcpClientArg ? [mcpClientArg] : undefined;

  if (command === 'install') {
    const results = resourceRoot ? install(targetAgents, resourceRoot) : install(targetAgents);

    if (results.length === 0) {
      console.log('No supported agents detected.');
      console.log(`Checked: ${validAgents.join(', ')}`);
      console.log('To install manually, specify an agent: bun install.js install <agent>');
      process.exit(0);
    }

    const seen = new Set<string>();
    for (const result of results) {
      seen.add(result.agent);
      if (result.installed.length > 0) {
        console.log(`✓ ${result.agent}: installed ${result.installed.length} file(s)`);
        for (const file of result.installed) {
          console.log(`  → ${file}`);
        }
      } else {
        // A warning, not a failure -- see the process-exit note below. On
        // stdout, not stderr: the desktop app's installer wrapper
        // (skill_setup.rs's parse_installer_output) parses this exact line
        // to report the skipped-with-reason state, and only reads stdout.
        console.log(`⚠ ${result.agent}: ${skipReasonText(result)}`);
      }
    }

    // Only when auto-detecting (no explicit agent): report every configured
    // agent that simply isn't present, so a caller parsing this output can
    // tell "not present" (unremarkable -- the normal case for a harness the
    // user doesn't use) apart from "detected but skipped" (the ⚠ case
    // above, a real warning) instead of both reading as the same silence.
    if (!agentArg) {
      for (const name of validAgents) {
        if (!seen.has(name)) {
          console.log(`  ${name}: not detected`);
        }
      }
    }

    // Exits 0 even when some detected agents had nothing to install --
    // that's a per-agent warning (on stdout above, alongside the ✓ lines,
    // specifically so a caller parsing this output sees it), not a reason
    // to fail the whole invocation when other agents installed
    // successfully, or even when none did: the situation is fully reported
    // either way, and a caller parsing this output needs the exit code to
    // mean "the installer ran to completion," not "every single agent
    // succeeded."
  } else if (command === 'uninstall') {
    const results = uninstall(targetAgents);

    if (results.length === 0) {
      console.log('No installed NodeSpace skills found.');
      process.exit(0);
    }

    for (const result of results) {
      if (result.removed.length > 0) {
        console.log(`✓ ${result.agent}: removed ${result.removed.length} file(s)`);
      } else {
        console.log(`  ${result.agent}: nothing to remove`);
      }
    }
  } else if (command === 'status') {
    // "✓ agent: present" reuses the exact marker/format the desktop app's
    // parse_installer_output already parses for the install command's
    // installed-agent lines, so skill_setup.rs's revalidation can share that
    // same parser instead of needing a second one for this command.
    const present = new Set(checkInstalled(targetAgents));
    const checked = targetAgents ?? validAgents;
    for (const name of checked) {
      if (present.has(name)) {
        console.log(`✓ ${name}: present`);
      } else {
        console.log(`  ${name}: not present`);
      }
    }
  } else if (command === 'mcp-install') {
    const results = installMcp(targetMcpClients);

    if (results.length === 0) {
      console.log('No supported bash-less MCP clients detected.');
      console.log(`Checked: ${validMcpClients.join(', ')}`);
      console.log('To configure one manually, specify a client: bun install.js mcp-install <client>');
      process.exit(0);
    }

    const seen = new Set<string>();
    for (const result of results) {
      seen.add(result.client);
      if (result.installed) {
        // On stdout, not stderr -- `commands::mcp::install` (the Rust
        // caller) parses this exact "✓ client: ..." / "⚠ client: reason"
        // contract via the same `parse_installer_output` the skill
        // installer's own `install`/`uninstall`/`status` already use.
        console.log(`✓ ${result.client}: MCP config written`);
      } else {
        console.log(`⚠ ${result.client}: ${mcpSkipReasonText(result.skipReason)}`);
      }
    }
    if (!mcpClientArg) {
      for (const name of validMcpClients) {
        if (!seen.has(name)) {
          console.log(`  ${name}: not detected`);
        }
      }
    }
  } else if (command === 'mcp-uninstall') {
    const results = uninstallMcp(targetMcpClients);

    if (results.length === 0) {
      console.log('No MCP client configs found.');
      process.exit(0);
    }

    for (const result of results) {
      if (result.removed) {
        console.log(`✓ ${result.client}: MCP config removed`);
      } else {
        console.log(`  ${result.client}: ${result.skipReason ?? 'nothing to remove'}`);
      }
    }
  } else if (command === 'mcp-status') {
    const present = new Set(checkMcpInstalled(targetMcpClients));
    const checked = targetMcpClients ?? validMcpClients;
    for (const name of checked) {
      if (present.has(name)) {
        console.log(`✓ ${name}: present`);
      } else {
        console.log(`  ${name}: not present`);
      }
    }
  } else {
    console.error(`Unknown command: ${command}`);
    printUsage();
    process.exit(1);
  }
}

if (import.meta.main) {
  main();
}
