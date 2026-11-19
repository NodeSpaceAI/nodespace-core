// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * One `{name, secret}` entry of an `env` list.
 */
export type EnvSecret = {
  /**
   * The environment variable the child process gets.
   */
  name: string;
  /**
   * The credential reference its value comes from.
   */
  secret: string;
};
