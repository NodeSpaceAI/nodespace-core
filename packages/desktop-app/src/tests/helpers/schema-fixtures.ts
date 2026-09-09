/**
 * Shared schema fixtures for protection-level filtering tests.
 *
 * Field shapes here are copied from `packages/core/src/models/core_schemas.rs`
 * — names, friendly names, types and protection levels all match the real core
 * schemas, in the real declaration order. They exist so the tests that guard
 * `isUserVisibleField` at its various call sites (table columns, relationship
 * column offerings) assert against what production actually ships rather than
 * a convenient approximation.
 *
 * Kept in one place deliberately: these fixtures were duplicated across two
 * test files first, and the copies drifted from the Rust source on four `type`
 * values before anyone noticed. One home means one place to re-sync when the
 * core schemas change.
 */
import type { SchemaField, SchemaNode } from '$lib/types/schema-node';

/** Build a SchemaField, defaulting the attributes most fixtures don't care about. */
export function field(
  partial: Partial<SchemaField> & { name: string; type: string }
): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

/** Build a SchemaNode around a field list, with plausible envelope metadata. */
export function schemaWith(id: string, isCore: boolean, fields: SchemaField[]): SchemaNode {
  return {
    id,
    content: id,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore,
    schemaVersion: 1,
    fields
  };
}

/** `person` — 3 visible, no system fields (core_schemas.rs, the `person` SchemaNode). */
export const PERSON_FIELDS: SchemaField[] = [
  field({ name: 'first_name', friendlyName: 'First name', type: 'string', protection: 'core' }),
  field({ name: 'last_name', friendlyName: 'Last name', type: 'string', protection: 'core' }),
  field({ name: 'email', friendlyName: 'Email', type: 'string', protection: 'core' })
];

/** `person`'s field names that should survive a user-visibility filter. */
export const PERSON_VISIBLE_NAMES = ['first_name', 'last_name', 'email'];

/**
 * `ai-chat`'s top-level fields — 4 visible, 6 system (core_schemas.rs, the
 * `ai-chat` SchemaNode). The worst case for a protection leak: `capture:transcript`
 * is raw PTY scrollback, documented there as possibly containing secrets, tokens
 * and absolute paths.
 */
export const AI_CHAT_FIELDS: SchemaField[] = [
  field({ name: 'provider', friendlyName: 'Provider', type: 'enum', protection: 'core' }),
  field({ name: 'model', friendlyName: 'Model', type: 'text', protection: 'core' }),
  field({ name: 'status', friendlyName: 'Conversation status', type: 'enum', protection: 'core' }),
  field({
    name: 'last_active',
    friendlyName: 'Last active',
    type: 'datetime',
    protection: 'system'
  }),
  field({
    name: 'context_tokens',
    friendlyName: 'Context tokens',
    type: 'number',
    protection: 'system'
  }),
  field({
    name: 'created_nodes',
    friendlyName: 'Created nodes',
    type: 'array',
    protection: 'system'
  }),
  field({ name: 'messages', friendlyName: 'Messages', type: 'array', protection: 'core' }),
  field({
    name: 'capture:session_id',
    friendlyName: 'Session id',
    type: 'text',
    protection: 'system'
  }),
  field({
    name: 'capture:transcript',
    friendlyName: 'Transcript',
    type: 'text',
    protection: 'system'
  }),
  field({ name: 'capture:summary', friendlyName: 'Summary', type: 'text', protection: 'system' })
];

/**
 * `ai-chat`'s field names that should survive a user-visibility filter, in
 * schema order — `messages` still follows `status` despite three system fields
 * being removed from between them.
 */
export const AI_CHAT_VISIBLE_NAMES = ['provider', 'model', 'status', 'messages'];

/** `ai-chat`'s system field names, every one of which must be filtered out. */
export const AI_CHAT_SYSTEM_NAMES = [
  'last_active',
  'context_tokens',
  'created_nodes',
  'capture:session_id',
  'capture:transcript',
  'capture:summary'
];

/**
 * The `friendlyName` of each `ai-chat` system field — what a leaked one would
 * actually read as in a column header or picker, for surfaces that assert on
 * rendered labels rather than field names.
 */
export const AI_CHAT_SYSTEM_LABELS = [
  'Last active',
  'Context tokens',
  'Created nodes',
  'Session id',
  'Transcript',
  'Summary'
];
