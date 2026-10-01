/**
 * Smoke render tests for NestedFieldEditor — the recursive, presentational editor
 * for nested (object/array) schema properties.
 *
 * These verify that an object-of-leaves and an array-of-objects fixture render
 * their controls and structural affordances, and that an edit rebuilds the whole
 * value immutably and emits it via onChange (the editor never touches a store).
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import type { SchemaField } from '$lib/types/schema-node';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import NestedFieldEditor from '$lib/components/schema/nested-field-editor.svelte';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

afterEach(() => {
  cleanup();
});

describe('NestedFieldEditor — object of leaves', () => {
  const objectField = field({
    name: 'address',
    type: 'object',
    fields: [
      field({ name: 'street', friendlyName: 'Street', type: 'text' }),
      field({ name: 'city', friendlyName: 'City', type: 'text' })
    ]
  });

  it('renders a text control per sub-field and labels them', () => {
    const { getByText, container } = render(NestedFieldEditor, {
      props: { field: objectField, value: { street: '1 Main', city: 'Denver' }, onChange: vi.fn() }
    });
    expect(getByText('Street')).toBeTruthy();
    expect(getByText('City')).toBeTruthy();
    const inputs = container.querySelectorAll('input[type="text"]');
    expect(inputs.length).toBe(2);
  });

  it('emits a new object (immutably) when a leaf is edited', async () => {
    const onChange = vi.fn();
    const value = { street: '1 Main', city: 'Denver' };
    const { container } = render(NestedFieldEditor, {
      props: { field: objectField, value, onChange }
    });
    const streetInput = container.querySelector('input[type="text"]') as HTMLInputElement;
    await fireEvent.input(streetInput, { target: { value: '2 Oak' } });
    expect(onChange).toHaveBeenCalledWith({ street: '2 Oak', city: 'Denver' });
    // Input value object was not mutated.
    expect(value).toEqual({ street: '1 Main', city: 'Denver' });
  });

  it('emits the object without a key when a sub-field is deleted', async () => {
    const onChange = vi.fn();
    const { getByLabelText } = render(NestedFieldEditor, {
      props: {
        field: objectField,
        value: { street: '1 Main', city: 'Denver' },
        onChange
      }
    });
    await fireEvent.click(getByLabelText('Remove city'));
    expect(onChange).toHaveBeenCalledWith({ street: '1 Main' });
  });

  it('renders even when the value is null (guards missing values)', () => {
    const { getByText } = render(NestedFieldEditor, {
      props: { field: objectField, value: null, onChange: vi.fn() }
    });
    expect(getByText('Street')).toBeTruthy();
  });
});

describe('NestedFieldEditor — array of objects', () => {
  const arrayField = field({
    name: 'contacts',
    type: 'array',
    itemType: 'object',
    itemFields: [field({ name: 'email', type: 'text' })]
  });

  it('renders an Item row per element plus an Add item control', () => {
    const { getByText } = render(NestedFieldEditor, {
      props: {
        field: arrayField,
        value: [{ email: 'a@x.com' }, { email: 'b@x.com' }],
        onChange: vi.fn()
      }
    });
    expect(getByText('Item 1')).toBeTruthy();
    expect(getByText('Item 2')).toBeTruthy();
    expect(getByText('Add item')).toBeTruthy();
  });

  it('appends an empty object when Add item is clicked', async () => {
    const onChange = vi.fn();
    const { getByText } = render(NestedFieldEditor, {
      props: { field: arrayField, value: [{ email: 'a@x.com' }], onChange }
    });
    await fireEvent.click(getByText('Add item'));
    expect(onChange).toHaveBeenCalledWith([{ email: 'a@x.com' }, {}]);
  });

  it('emits the array without an element when it is deleted', async () => {
    const onChange = vi.fn();
    const { getByLabelText } = render(NestedFieldEditor, {
      props: {
        field: arrayField,
        value: [{ email: 'a@x.com' }, { email: 'b@x.com' }],
        onChange
      }
    });
    await fireEvent.click(getByLabelText('Remove item 1'));
    expect(onChange).toHaveBeenCalledWith([{ email: 'b@x.com' }]);
  });
});

describe('NestedFieldEditor — array of scalars', () => {
  const scalarArrayField = field({ name: 'tags', type: 'array', itemType: 'text' });

  it('replaces the element at an index on edit', async () => {
    const onChange = vi.fn();
    const { container } = render(NestedFieldEditor, {
      props: { field: scalarArrayField, value: ['x', 'y'], onChange }
    });
    const inputs = container.querySelectorAll('input[type="text"]');
    expect(inputs.length).toBe(2);
    await fireEvent.input(inputs[1] as HTMLInputElement, { target: { value: 'z' } });
    expect(onChange).toHaveBeenCalledWith(['x', 'z']);
  });

  it('appends an empty string scalar on Add item', async () => {
    const onChange = vi.fn();
    const { getByText } = render(NestedFieldEditor, {
      props: { field: scalarArrayField, value: ['x'], onChange }
    });
    await fireEvent.click(getByText('Add item'));
    expect(onChange).toHaveBeenCalledWith(['x', '']);
  });
});
