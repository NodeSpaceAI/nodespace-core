import { describe, expect, it } from 'vitest';
import { declaredFields, type SchemaField, type SchemaNode } from '$lib/types/schema-node';

const field = (name: string): SchemaField =>
  ({ name, friendlyName: name, type: 'text', protection: 'user', indexed: false }) as SchemaField;

describe('declaredFields', () => {
  it('lists the shipped fields, then the custom fields of a built-in type', () => {
    const schema = { fields: [field('status')], customFields: [field('effort')] } as SchemaNode;
    expect(declaredFields(schema).map((f) => f.name)).toEqual(['status', 'effort']);
  });

  it('is the shipped fields alone when the type has no custom fields, and empty for no schema', () => {
    expect(declaredFields({ fields: [field('a')] } as SchemaNode).map((f) => f.name)).toEqual(['a']);
    expect(declaredFields(null)).toEqual([]);
  });
});
