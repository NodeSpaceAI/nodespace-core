/**
 * Lookups over `TYPED_CORE_FIELDS`. The table itself is generated from Rust's
 * promoted fields and gated for drift (`bun run gen:types --check`), so these
 * cover only the helpers.
 */

import { describe, it, expect } from 'vitest';
import {
  hasTypedUpdate,
  typedCoreField,
  typedCoreKeys,
  writableTypedCoreKeys
} from '$lib/types/typed-core-fields';

describe('typed core field lookups', () => {
  it('resolves a field by either spelling, and nothing for an extension field', () => {
    expect(typedCoreField('task', 'due_date')?.wire).toBe('dueDate');
    expect(typedCoreField('task', 'dueDate')?.storage).toBe('due_date');
    expect(typedCoreField('task', 'custom:store')).toBeUndefined();
    expect(typedCoreField('invoice', 'status')).toBeUndefined();
  });

  it('knows which types are written through a typed update', () => {
    for (const type of ['task', 'person', 'project', 'query']) {
      expect(hasTypedUpdate(type), type).toBe(true);
    }
    expect(hasTypedUpdate('text')).toBe(false);
    expect(hasTypedUpdate(undefined)).toBe(false);
  });

  it('promotes the chat subtypes\' fields without a typed update', () => {
    // The chat family travels typed but is written as `properties` patches
    // keyed by the storage name.
    for (const type of ['ai-chat-native', 'ai-chat-pty']) {
      expect(hasTypedUpdate(type), type).toBe(false);
      expect(typedCoreField(type, 'agent')?.wire, type).toBe('agent');
    }
    expect(typedCoreField('ai-chat-native', 'turn_status')?.wire).toBe('turnStatus');
    expect(typedCoreField('ai-chat-pty', 'session_status')?.wire).toBe('sessionStatus');
    // A native chat has no session state, and a terminal chat no messages.
    expect(typedCoreField('ai-chat-native', 'session_status')).toBeUndefined();
    expect(typedCoreField('ai-chat-pty', 'messages')).toBeUndefined();
  });

  it('leaves the system-managed fields out of the writable keys', () => {
    expect(typedCoreKeys('query')).toContain('executionCount');
    expect(writableTypedCoreKeys('query')).not.toContain('executionCount');
    expect(writableTypedCoreKeys('query')).not.toContain('lastExecuted');
    expect(writableTypedCoreKeys('query')).toContain('viewConfig');
    expect(writableTypedCoreKeys('person')).toEqual(typedCoreKeys('person'));
  });
});
