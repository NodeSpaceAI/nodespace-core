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
    nodeType: 'schema' as const,
    lifecycleStatus: 'active' as const,
    properties: {},
    id,
    content: id,
    createdAt: '2026-01-01T00:00:00Z',
    modifiedAt: '2026-01-01T00:00:00Z',
    version: 1,
    isCore,
    schemaVersion: 1,
    relationships: [],
    fields
  };
}

/** `person` — 3 visible, no system fields (core_schemas.rs, the `person` SchemaNode). */
export const PERSON_FIELDS: SchemaField[] = [
  field({ name: 'first_name', friendlyName: 'First name', type: 'text', protection: 'core' }),
  field({ name: 'last_name', friendlyName: 'Last name', type: 'text', protection: 'core' }),
  field({ name: 'email', friendlyName: 'Email', type: 'text', protection: 'core' })
];

/** `person`'s field names that should survive a user-visibility filter. */
export const PERSON_VISIBLE_NAMES = ['first_name', 'last_name', 'email'];

/**
 * `ai-chat-native`'s fields, the inherited `ai-chat` base fields first — 5
 * visible, 3 system (core_schemas.rs, the `ai-chat` and `ai-chat-native`
 * SchemaNodes).
 */
export const AI_CHAT_FIELDS: SchemaField[] = [
  field({ name: 'agent', friendlyName: 'Agent', type: 'text', protection: 'core' }),
  field({ name: 'model', friendlyName: 'Model', type: 'text', protection: 'core' }),
  field({ name: 'summary', friendlyName: 'Summary', type: 'text', protection: 'system' }),
  field({
    name: 'last_active',
    friendlyName: 'Last active',
    type: 'datetime',
    protection: 'system'
  }),
  field({ name: 'provider', friendlyName: 'Provider', type: 'enum', protection: 'core' }),
  field({ name: 'turn_status', friendlyName: 'Turn status', type: 'enum', protection: 'core' }),
  field({
    name: 'context_tokens',
    friendlyName: 'Context tokens',
    type: 'number',
    protection: 'system'
  }),
  field({ name: 'messages', friendlyName: 'Messages', type: 'array', protection: 'core' })
];

/**
 * `ai-chat-native`'s field names that should survive a user-visibility filter,
 * in schema order — `provider` still follows `model` despite two system fields
 * being removed from between them.
 */
export const AI_CHAT_VISIBLE_NAMES = ['agent', 'model', 'provider', 'turn_status', 'messages'];

/** `ai-chat-native`'s system field names, every one of which must be filtered out. */
export const AI_CHAT_SYSTEM_NAMES = ['summary', 'last_active', 'context_tokens'];

/**
 * The `friendlyName` of each `ai-chat-native` system field — what a leaked one
 * would actually read as in a column header or picker, for surfaces that assert
 * on rendered labels rather than field names.
 */
export const AI_CHAT_SYSTEM_LABELS = ['Summary', 'Last active', 'Context tokens'];

/**
 * `ai-chat-pty`'s fields, the inherited `ai-chat` base fields first. The worst
 * case for a protection leak: `transcript` is raw PTY scrollback, documented
 * there as possibly containing secrets, tokens and absolute paths.
 */
export const AI_CHAT_PTY_FIELDS: SchemaField[] = [
  field({ name: 'agent', friendlyName: 'Agent', type: 'text', protection: 'core' }),
  field({ name: 'model', friendlyName: 'Model', type: 'text', protection: 'core' }),
  field({ name: 'summary', friendlyName: 'Summary', type: 'text', protection: 'system' }),
  field({
    name: 'last_active',
    friendlyName: 'Last active',
    type: 'datetime',
    protection: 'system'
  }),
  field({
    name: 'session_status',
    friendlyName: 'Session status',
    type: 'enum',
    protection: 'core'
  }),
  field({ name: 'session_id', friendlyName: 'Session id', type: 'text', protection: 'system' }),
  field({ name: 'transcript', friendlyName: 'Transcript', type: 'text', protection: 'system' }),
  field({ name: 'exit_code', friendlyName: 'Exit code', type: 'number', protection: 'system' })
];

/** `ai-chat-pty`'s system field labels, every one of which must be filtered out. */
export const AI_CHAT_PTY_SYSTEM_LABELS = [
  'Summary',
  'Last active',
  'Session id',
  'Transcript',
  'Exit code'
];
