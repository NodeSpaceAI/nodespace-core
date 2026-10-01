/**
 * NestedPropertyModal — the shared dialog wrapper around NestedFieldEditor.
 *
 * The modal is deliberately persistence-free: it renders the `value` the caller
 * hands it and reports every rebuilt value back through `onPersist`. That is
 * what lets one modal serve forms that store properties flat, under
 * `properties.task`, and under `properties[nodeType]`.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, screen, fireEvent } from '@testing-library/svelte';
import type { SchemaField } from '$lib/types/schema-node';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import NestedPropertyModal from '$lib/components/schema/nested-property-modal.svelte';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

const addressField = field({
  name: 'address',
  type: 'object',
  fields: [
    field({ name: 'street', friendlyName: 'Street', type: 'text' }),
    field({ name: 'city', friendlyName: 'City', type: 'text' })
  ]
});

afterEach(() => {
  cleanup();
});

describe('NestedPropertyModal', () => {
  it('renders the caller-supplied value rather than reading a store', () => {
    render(NestedPropertyModal, {
      props: {
        open: true,
        field: addressField,
        value: { street: '1 Main', city: 'Denver' },
        onPersist: vi.fn()
      }
    });

    expect((screen.getByLabelText('Street') as HTMLInputElement).value).toBe('1 Main');
    expect((screen.getByLabelText('City') as HTMLInputElement).value).toBe('Denver');
  });

  it('hands the whole rebuilt value to onPersist on every edit', async () => {
    const onPersist = vi.fn();
    render(NestedPropertyModal, {
      props: { open: true, field: addressField, value: { city: 'Denver' }, onPersist }
    });

    await fireEvent.input(screen.getByLabelText('Street'), { target: { value: '1 Main' } });

    expect(onPersist).toHaveBeenCalledWith({ city: 'Denver', street: '1 Main' });
  });

  it('renders nothing while closed', () => {
    render(NestedPropertyModal, {
      props: { open: false, field: addressField, value: {}, onPersist: vi.fn() }
    });

    expect(screen.queryByLabelText('Street')).toBeNull();
  });
});
