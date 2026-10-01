// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.
import type { RelationshipHop } from './relationship-hop';

/**
 * A walk through the graph: the hops to follow, in order, from a starting
 * node. On the wire it is the list of hops: `["child_of", "has_child"]`.
 *
 * A path names relationships only. Whether a name is valid, and which stored
 * edges it means, depends on the schemas: resolving a path against them
 * yields a [`ResolvedPath`].
 */
export type RelationshipPath = Array<RelationshipHop>;
