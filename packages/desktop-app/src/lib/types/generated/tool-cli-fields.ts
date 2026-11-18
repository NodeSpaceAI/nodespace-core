// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { EnvSecret } from './env-secret';

/**
 * The `tool-cli` bucket: a command-line program.
 */
export type ToolCliFields = {
  /**
   * An executable name or absolute path.
   */
  program?: string;
  /**
   * One argv element per item: a literal, or exactly one `{name}`.
   */
  args?: Array<string>;
  /**
   * The only environment the child gets, besides a fixed base list.
   */
  env?: Array<EnvSecret>;
  /**
   * Seconds before the call is killed.
   */
  timeout_seconds?: number;
  /**
   * Cap on the output, in bytes.
   */
  max_output_bytes?: number;
};
