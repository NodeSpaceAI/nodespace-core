/**
 * Database-settings constants
 *
 * @module constants/database-settings
 */

/**
 * The id of the per-database settings singleton. Core seeds exactly one such
 * node in every database under this id; it mirrors Rust's
 * `DATABASE_SETTINGS_NODE_ID` in `node_service`. The node's properties sit flat
 * on `properties`.
 *
 * The schema/nodeType slug is the bare `database-settings`; this instance id is
 * deliberately distinct from it so the two never collide.
 */
export const DATABASE_SETTINGS_NODE_ID = 'database-settings-singleton';
