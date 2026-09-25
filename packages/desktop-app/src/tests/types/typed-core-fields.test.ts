/**
 * `TYPED_CORE_FIELDS` is the frontend mirror of Rust's `promoted_fields`
 * (`packages/nodespace-types/src/convert.rs`). The two are hand-synced across
 * the language boundary, so this reads the Rust source and asserts they list
 * the same `(storage key, wire key)` pairs per type. A drift would make the
 * dev-proxy transport deliver a different shape from Tauri IPC, and the store
 * would route a field the backend treats as an extension field (or vice
 * versa). The Rust side is in turn pinned to the core schemas by
 * `promoted_fields_match_each_typed_core_schema` (core_schemas.rs).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { TYPED_CORE_FIELDS, typedCoreField } from '$lib/types/typed-core-fields';

/** Parse `promoted_fields`' match arms into `{ nodeType: [[storage, wire], ...] }`. */
function rustPromotedFields(): Record<string, Array<[string, string]>> {
  // __dirname-relative, not cwd-relative, so it resolves from any runner cwd.
  const source = readFileSync(
    resolve(__dirname, '../../../../nodespace-types/src/convert.rs'),
    'utf8'
  );
  const start = source.indexOf('pub fn promoted_fields(');
  expect(start, 'promoted_fields not found in convert.rs').toBeGreaterThan(-1);
  const body = source.slice(start, source.indexOf('_ => &[]', start));

  const result: Record<string, Array<[string, string]>> = {};
  const armRe = /"([a-z-]+)"\s*=>\s*&\[([\s\S]*?)\]/g;
  for (const arm of body.matchAll(armRe)) {
    const pairs = [...arm[2].matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map(
      (m) => [m[1], m[2]] as [string, string]
    );
    result[arm[1]] = pairs;
  }
  return result;
}

describe('TYPED_CORE_FIELDS', () => {
  it('lists exactly the pairs Rust promoted_fields lists, per type', () => {
    const rust = rustPromotedFields();
    const ts = Object.fromEntries(
      Object.entries(TYPED_CORE_FIELDS).map(([type, fields]) => [
        type,
        fields.map((f) => [f.storage, f.wire] as [string, string])
      ])
    );

    expect(Object.keys(rust).sort()).toEqual(['person', 'project', 'task']);
    expect(ts).toEqual(rust);
  });

  it('resolves a field by either spelling, and nothing for an extension field', () => {
    expect(typedCoreField('task', 'due_date')?.wire).toBe('dueDate');
    expect(typedCoreField('task', 'dueDate')?.storage).toBe('due_date');
    expect(typedCoreField('task', 'custom:store')).toBeUndefined();
    expect(typedCoreField('invoice', 'status')).toBeUndefined();
  });
});
