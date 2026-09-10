import { homedir } from 'node:os';
import { join } from 'node:path';

/**
 * Bash-less MCP clients this package knows how to configure. Claude Desktop
 * is the only one today -- ADR-035 names its Chat tab as the motivating
 * surface (no shell, no `nodespace` CLI reachable any other way). This is
 * deliberately a list, not a hardcoded single case, so a future bash-less
 * client (claude.ai has no local process to configure; a future desktop MCP
 * client would) is an added entry rather than a rewrite.
 */
export type McpClientName = 'claude-desktop';

export interface McpClientConfig {
  name: McpClientName;
  /**
   * Directory whose existence signals the client is installed on this
   * machine -- the same detection shape `agents.ts`'s `detectionDir` uses.
   */
  detectionDir: string;
  /** The JSON config file this client reads its MCP server list from. */
  configPath: string;
  /** Key this package's entry is written under in `configPath`'s `mcpServers` object. */
  serverKey: string;
}

const home = homedir();

/**
 * Claude Desktop's per-OS config directory. `nodespace mcp` (the server this
 * config would point at) only runs on Unix -- `packages/cli`'s `run()` bails
 * immediately on Windows, unconditionally, for every subcommand, since its
 * only transport is a Unix domain socket -- so only macOS/Linux are worth
 * resolving here. A Windows install of this package would still import
 * cleanly (this returns a path, just one nothing here can ever act on).
 */
function claudeDesktopConfigDir(): string {
  if (process.platform === 'darwin') {
    return join(home, 'Library', 'Application Support', 'Claude');
  }
  // Linux community builds of Claude Desktop use the XDG config convention.
  return join(home, '.config', 'Claude');
}

export const MCP_CLIENTS: McpClientConfig[] = [
  {
    name: 'claude-desktop',
    detectionDir: claudeDesktopConfigDir(),
    configPath: join(claudeDesktopConfigDir(), 'claude_desktop_config.json'),
    serverKey: 'nodespace',
  },
];
