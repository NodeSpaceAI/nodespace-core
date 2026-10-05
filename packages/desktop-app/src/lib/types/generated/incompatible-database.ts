// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

/**
 * The daemon's default database was created by a different version of
 * NodeSpace and does not match this version's schema. NodeSpace does
 * not migrate databases, so the only way forward is to move the file aside
 * and start with a fresh one.
 */
export type IncompatibleDatabase = {
  /**
   * Absolute path of the database file the daemon refused to open.
   */
  databasePath: string;
  /**
   * Which tables or core types differ and how, for logs and support. Not
   * meant to be shown to a user as the headline.
   */
  detail: string;
  /**
   * When the daemon refused the database, RFC 3339.
   */
  detectedAt: string;
};
