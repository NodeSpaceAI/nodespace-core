// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The `auth` object of an http binding.
 */
export type HttpAuth = {
  /**
   * `none`, `bearer`, `basic` (REST only) or `header`.
   */
  scheme: string;
  /**
   * The header that carries the credential, for `header`.
   */
  header_name?: string;
  /**
   * The credential reference, for every scheme but `none`.
   */
  secret?: string;
};
