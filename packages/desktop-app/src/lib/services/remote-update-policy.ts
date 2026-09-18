/**
 * Remote-update policy for SharedNodeStore.
 *
 * Extracted from the skip-while-editing guard inline in `setNode` /
 * `batchSetNodes`. A daemon-broadcast event (`source.type === 'database'`)
 * arriving for a node the user is actively editing — or has unsaved local
 * changes pending — would otherwise overwrite the optimistic store with the
 * *older* server-confirmed state. The optimistic state is authoritative
 * until persistence settles.
 *
 * `decideRemoteUpdate` is a pure function: it takes the incoming node, the
 * existing local node (if any), the update source, and the caller's
 * pre-computed editing state, and returns a decision the caller applies.
 * It does not read `focusManager` or `PersistenceCoordinator` itself — the
 * caller computes `isFocused`/`hasPending` once (each is a live read that
 * can disagree between two reads within the same guard) and passes them in,
 * so this module stays reactivity-free and independently testable.
 *
 * Own-write echo classification (ADR-026 C5 extension): earlier versions of
 * this module guessed whether an incoming broadcast was this client's own
 * write looping back, by comparing its content against the last content this
 * client sent (`isPlausibleOwnEcho`). That guess was inherently racy across an
 * async network round-trip and repeatedly produced false-positive/false-
 * negative conflict toasts — the exact failure mode ADR-026's C5 amendment
 * already rejected for a prior client-side heuristic. The daemon now
 * suppresses a connection's own write echoes before they ever reach
 * `WatchNodes` (`packages/daemon/src/services/node_service.rs`,
 * `x-ns-client-id`-scoped `NodeService::with_client()`), so a `database`-
 * sourced event reaching this module can no longer be this client's own
 * echo of a write made through the SAME gRPC connection. No content
 * comparison is needed or performed here anymore.
 *
 * Sync-service echoes are a separate case the daemon-side fix above does not
 * cover: `nodespace-sync` writes to the local DB in-process via
 * `NodeService::with_client("sync-service")` (ADR-027), not over the gRPC
 * connection the desktop app's `x-ns-client-id` header scopes — so a stale
 * sync-service replay (e.g. during reconnect reconciliation) can still reach
 * this module as a `database`-sourced event whose version is not ahead of
 * the local optimistic version. The `incomingIsNewer` check below guards
 * against exactly that: only a version genuinely ahead of local is treated
 * as a real conflict.
 */

import type { Node } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';

export interface EditingState {
  isFocused: boolean;
  hasPending: boolean;
}

export type RemoteUpdateDecision =
  | { apply: true }
  | {
      apply: false;
      /** Raise a version-mismatch conflict notification (foreign write to an actively-edited node). */
      notifyConflict: boolean;
    };

/**
 * Core remote-update policy. Given the incoming node, the existing local
 * node (undefined if this id has never been seen locally), the update
 * source, and the caller's editing state, decide whether the caller should
 * apply the incoming node to the store.
 *
 * A `database`-sourced update to a node the user is actively editing is
 * never applied — the optimistic local content is always protected. A
 * conflict notification is raised only when the incoming version is
 * strictly newer than the local version (a genuine foreign write); a
 * same-or-older version is a stale broadcast (most commonly a sync-service
 * replay — same-connection echoes are now suppressed daemon-side, see this
 * module's doc comment) and must not raise a phantom notification.
 *
 * ai-chat nodes are exempt from the focus/content-editing skip — see
 * `shouldSkipStaleAiChatUpdate` for that separate guard (version, then
 * message count, plus a same-version-with-a-pending-write check — not
 * focus).
 */
export function decideRemoteUpdate(
  incoming: Node,
  existingNode: Node | undefined,
  source: UpdateSource,
  editingState: EditingState
): RemoteUpdateDecision {
  const isDatabaseSource = source.type === 'database';
  const isActivelyEdited = editingState.isFocused || editingState.hasPending;

  if (!isDatabaseSource || !existingNode || !isActivelyEdited) {
    return { apply: true };
  }

  // Missing/uncomparable versions fall back to notifying (conservative).
  const incomingIsNewer =
    typeof incoming.version !== 'number' ||
    typeof existingNode.version !== 'number' ||
    incoming.version > existingNode.version;

  return { apply: false, notifyConflict: incomingIsNewer };
}

/**
 * ai-chat nodes are never "typed into" — the messages array is written
 * programmatically via updateNode, and the daemon appends assistant replies
 * autonomously. Skipping daemon broadcasts for them (via `decideRemoteUpdate`)
 * would cause version drift: the store stays at the user-send version while
 * the daemon is N+1 ahead, so the next user send hits an OCC conflict.
 * Always accept daemon updates for ai-chat EXCEPT when the incoming snapshot
 * is a stale broadcast racing a newer one.
 *
 * Staleness is decided by VERSION first, message count only as a tiebreak.
 * Message count alone is not a safe staleness signal: a turn that rewrites
 * history rather than appending (a cancelled turn dropping its partial reply,
 * say) legitimately produces a newer snapshot with fewer messages, and
 * count-only comparison would discard it permanently. Version is the
 * authority the daemon actually increments, so:
 *
 *   - incoming version strictly older  → stale, skip.
 *   - incoming version strictly newer  → authoritative, apply.
 *   - versions equal, pending is true  → stale, skip (see `pending` below).
 *   - versions equal (or uncomparable), pending false → fall back to the
 *     message count, which is what distinguishes two broadcasts of the same
 *     generation.
 *
 * This is the same policy the OCC hydration path applies, so the two writers
 * into this store (conflict hydration and daemon broadcast) can no longer
 * disagree about which snapshot wins.
 *
 * `pending` — true when the local node has a write of its own still in
 * flight (`SimplePersistenceCoordinator.hasPending`, or the equivalent
 * point-in-time capture a caller already holds, e.g. `decideRemoteUpdate`'s
 * own `hasPending` param). A property-only optimistic write (e.g. model
 * selection) promotes its fields onto the local node immediately but does
 * NOT bump `.version` — only the write's own RPC response does, once it
 * resolves. So while that write is in flight, an incoming broadcast can
 * legitimately report the SAME version the local node is still sitting at,
 * while actually being the PRE-write snapshot arriving late (an unrelated
 * echo — e.g. the node's own creation broadcast, re-fetched unconditionally
 * — racing the in-flight write). Message count alone cannot always tell
 * these apart (an unset `model`/`provider` is not reflected in the message
 * array at all), so an equal-version snapshot is untrusted outright whenever
 * a local write is still outstanding, rather than relying solely on
 * `decideRemoteUpdate`'s separately-computed `hasPending` check downstream.
 * Worst case this skips an equal-version snapshot that would have been a
 * harmless no-op anyway (the in-flight write's own response carries the
 * same data) — never a wrongly-applied one.
 */
export function shouldSkipStaleAiChatUpdate(
  incoming: Node,
  existingNode: Node | undefined,
  source: UpdateSource,
  pending = false
): boolean {
  if (incoming.nodeType !== 'ai-chat' || source.type !== 'database' || !existingNode) {
    return false;
  }

  const incomingVersion = incoming.version;
  const existingVersion = existingNode.version;
  if (typeof incomingVersion === 'number' && typeof existingVersion === 'number') {
    if (incomingVersion !== existingVersion) {
      return incomingVersion < existingVersion;
    }
    if (pending) {
      return true;
    }
  }

  type AiChatLike = Node & { messages?: unknown[] };
  const incomingMsgs = (incoming as AiChatLike).messages;
  const existingMsgs = (existingNode as AiChatLike).messages;
  const incomingCount = Array.isArray(incomingMsgs) ? incomingMsgs.length : 0;
  const existingCount = Array.isArray(existingMsgs) ? existingMsgs.length : 0;
  return incomingCount < existingCount;
}
