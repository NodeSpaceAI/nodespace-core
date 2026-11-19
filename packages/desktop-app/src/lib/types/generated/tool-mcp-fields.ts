// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { EnvSecret } from './env-secret';
import type { HttpAuth } from './http-auth';

/**
 * The `tool-mcp` bucket: a tool on an MCP server.
 */
export type ToolMcpFields = {
  /**
   * `stdio` or `http`.
   */
  transport?: string;
  /**
   * stdio: the executable.
   */
  command?: string;
  /**
   * stdio: one argv element per item.
   */
  args?: Array<string>;
  /**
   * http: the server URL.
   */
  url?: string;
  /**
   * The tool's name on the server.
   */
  remote_tool_name?: string;
  /**
   * stdio: environment entries whose values are credential references.
   */
  env?: Array<EnvSecret>;
  /**
   * http: how the call authenticates.
   */
  auth?: HttpAuth;
  /**
   * Seconds before the call is abandoned.
   */
  timeout_seconds?: number;
};
