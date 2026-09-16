/**
 * Maps a gRPC-js `ServiceError`'s trailer metadata into the structured
 * `code`/`conflictData` shape the Tauri command layer derives from the same
 * daemon trailers.
 *
 * The daemon (packages/daemon/src/services/node_service.rs, `ops_error_to_status`)
 * attaches two ASCII trailer keys to specific tonic::Status codes:
 *
 *   - `Aborted` + `x-version-conflict` — a JSON payload
 *     `{ node_id, expected, actual, current_node }` describing an OCC conflict.
 *   - `FailedPrecondition` + `x-subtree-inaccessible-count` — the number of
 *     nodes in a refused cascade-delete subtree the actor cannot read
 *     (ADR-041). `FailedPrecondition` also fires from unrelated paths
 *     (node-create/schema validation), so the metadata key's presence — not
 *     the status code alone — is what marks a genuine subtree-access refusal.
 *   - `FailedPrecondition` + `x-play-rule-rejected` — a JSON payload
 *     `{ node_id, play_id, rule_id, message }` describing a synchronous
 *     `RuleClass::Invariant` rule's `reject` action (ADR-060 §2) vetoing a
 *     `create_node`/`update_node` call. Same `FailedPrecondition`-is-shared
 *     caveat as the subtree case above — gated on the metadata key, not the
 *     status code alone, and checked independently of it (the two never
 *     both appear on one status, but neither implies the other's absence).
 *
 * `status_to_command_error` (packages/desktop-app/src-tauri/src/commands/nodes.rs)
 * reads those same trailers over tonic on the Tauri path to build
 * `VERSION_CONFLICT` / `SUBTREE_ACCESS_DENIED` / `PLAY_RULE_REJECTED`
 * `CommandError`s. This module is the dev-proxy/gRPC-js-side mirror of that
 * logic, so a live refusal reached through `bun run dev:browser` carries the
 * same `code`/`conflictData` shape a refusal reached through the Tauri
 * command layer does — which is what
 * `isSubtreeAccessDenied`/`isVersionConflict`/`isPlayRuleRejected`
 * (packages/desktop-app/src/lib/types/errors.ts) structurally match against,
 * regardless of transport.
 */

import * as grpc from '@grpc/grpc-js';

export interface MappedGrpcError {
  code: string;
  conflictData?: unknown;
}

/** First value for `key` in gRPC-js trailer `metadata`, decoded to a string. */
function firstMetadataString(metadata: grpc.Metadata, key: string): string | undefined {
  const [value] = metadata.get(key);
  if (value === undefined) return undefined;
  return typeof value === 'string' ? value : value.toString('utf8');
}

function safeJsonParse(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

/**
 * Derive `{ code, conflictData }` from a gRPC-js `ServiceError`, mirroring
 * `status_to_command_error`'s trailer inspection exactly:
 *
 *  - `ABORTED` → code `VERSION_CONFLICT`. `conflictData` is the parsed
 *    `x-version-conflict` JSON payload when present and parseable, otherwise
 *    omitted (matches the Rust side: the code is set unconditionally on
 *    `Aborted`, the payload only when the metadata parses).
 *  - `FAILED_PRECONDITION` with a parseable `x-subtree-inaccessible-count`
 *    metadata value → code `SUBTREE_ACCESS_DENIED`, `conflictData` =
 *    `{ inaccessibleCount }`.
 *  - `FAILED_PRECONDITION` with a parseable `x-play-rule-rejected` metadata
 *    value → code `PLAY_RULE_REJECTED`, `conflictData` = the parsed JSON
 *    payload (`{ node_id, play_id, rule_id, message }`).
 *  - A `FAILED_PRECONDITION` matching neither metadata key falls through to
 *    the generic mapping below — it is neither kind of refusal.
 *  - Everything else keeps the pre-existing dev-proxy behavior: the generic
 *    gRPC status name (`grpc.status[code]`), no `conflictData`.
 */
export function mapGrpcError(err: grpc.ServiceError): MappedGrpcError {
  const metadata = err.metadata;

  if (err.code === grpc.status.ABORTED) {
    const raw = firstMetadataString(metadata, 'x-version-conflict');
    const conflictData = raw !== undefined ? safeJsonParse(raw) : undefined;
    return conflictData !== undefined
      ? { code: 'VERSION_CONFLICT', conflictData }
      : { code: 'VERSION_CONFLICT' };
  }

  if (err.code === grpc.status.FAILED_PRECONDITION) {
    const subtreeRaw = firstMetadataString(metadata, 'x-subtree-inaccessible-count');
    // Match Rust's `str::parse::<u64>()` exactly: the whole trailer value must
    // be plain ASCII digits — no sign, no leading '+', no trailing garbage.
    // The daemon only ever emits `inaccessible_count.to_string()` (a bare
    // unsigned decimal), so this never rejects a real trailer; it just keeps
    // a malformed/adversarial one from silently coercing (e.g. Number.parseInt
    // would accept "+3", "3abc", or "-1", none of which Rust's parser would).
    if (subtreeRaw !== undefined && /^\d+$/.test(subtreeRaw)) {
      const inaccessibleCount = Number.parseInt(subtreeRaw, 10);
      if (Number.isFinite(inaccessibleCount)) {
        return { code: 'SUBTREE_ACCESS_DENIED', conflictData: { inaccessibleCount } };
      }
    }

    const rejectedRaw = firstMetadataString(metadata, 'x-play-rule-rejected');
    if (rejectedRaw !== undefined) {
      const conflictData = safeJsonParse(rejectedRaw);
      if (conflictData !== undefined) {
        return { code: 'PLAY_RULE_REJECTED', conflictData };
      }
    }
  }

  return { code: grpc.status[err.code] ?? 'UNKNOWN' };
}
