/**
 * Lookups over `TYPED_CORE_FIELDS`. The table itself is generated from Rust's
 * promoted fields and gated for drift (`bun run gen:types --check`), so these
 * cover only the helpers.
 */

import { describe, it, expect } from 'vitest';
import {
  hasTypedCoreFields,
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

  it('knows which types have typed core fields', () => {
    expect(hasTypedCoreFields('person')).toBe(true);
    expect(hasTypedCoreFields('text')).toBe(false);
    expect(hasTypedCoreFields(undefined)).toBe(false);
  });

  it('leaves the system-managed fields out of the writable keys', () => {
    expect(typedCoreKeys('query')).toContain('executionCount');
    expect(writableTypedCoreKeys('query')).not.toContain('executionCount');
    expect(writableTypedCoreKeys('query')).not.toContain('lastExecuted');
    expect(writableTypedCoreKeys('query')).toContain('viewConfig');
    expect(writableTypedCoreKeys('person')).toEqual(typedCoreKeys('person'));
  });
});
