/**
 * TableView column derivation — protection-level filtering.
 *
 * TableView used to iterate `schema.fields` unconditionally, so every
 * `protection: 'system'` field became a user-facing column — surfacing six
 * columns on ai-chat, including `capture:transcript`, raw PTY scrollback
 * documented as possibly containing secrets, tokens and absolute paths.
 *
 * The detail form already filtered these out (see
 * generic-schema-form-protection-filter.test.ts), so the two views disagreed.
 * These tests mirror that file and pin the table side, using the real core
 * field shapes from the shared fixtures in `../helpers/schema-fixtures`.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import TableView from '$lib/components/query/table-view.svelte';
import {
  field,
  schemaWith,
  PERSON_FIELDS,
  AI_CHAT_FIELDS,
  AI_CHAT_SYSTEM_LABELS
} from '../helpers/schema-fixtures';

function headerTexts(container: HTMLElement): string[] {
  return [...container.querySelectorAll('thead th')].map((th) => th.textContent?.trim() ?? '');
}

afterEach(() => {
  cleanup();
});

describe('TableView — protection-level filtering', () => {
  it('renders all of person’s fields — none are system-protected', () => {
    const { container, getByText } = render(TableView, {
      props: {
        nodeIds: [],
        schema: schemaWith('person', true, PERSON_FIELDS),
        fieldSchemaMap: new Map(),
        onRowClick: vi.fn()
      }
    });

    expect(getByText('First name')).toBeTruthy();
    expect(getByText('Last name')).toBeTruthy();
    expect(getByText('Email')).toBeTruthy();
    // The leading '' column is the content/title link column.
    expect(headerTexts(container)).toEqual(['', 'First name', 'Last name', 'Email']);
  });

  it('omits all six of ai-chat’s system fields, transcript included', () => {
    const { container, queryByText } = render(TableView, {
      props: {
        nodeIds: [],
        schema: schemaWith('ai-chat', true, AI_CHAT_FIELDS),
        fieldSchemaMap: new Map(),
        onRowClick: vi.fn()
      }
    });

    for (const label of AI_CHAT_SYSTEM_LABELS) {
      expect(queryByText(label)).toBeNull();
    }
    // Non-system fields survive, in schema order — note `messages` still follows
    // `status` even though three system fields were removed from between them.
    expect(headerTexts(container)).toEqual([
      '',
      'Provider',
      'Model',
      'Conversation status',
      'Messages'
    ]);
  });

  it('applies the same filter to a user-defined (schema-only) type', () => {
    const { container, queryByText } = render(TableView, {
      props: {
        nodeIds: [],
        schema: schemaWith('venue', false, [
          field({ name: 'capacity', friendlyName: 'Capacity', type: 'number' }),
          field({
            name: '_internal_marker',
            friendlyName: 'Internal marker',
            type: 'boolean',
            protection: 'system'
          }),
          field({ name: 'address', friendlyName: 'Address', type: 'string' })
        ]),
        fieldSchemaMap: new Map(),
        onRowClick: vi.fn()
      }
    });

    expect(queryByText('Internal marker')).toBeNull();
    expect(headerTexts(container)).toEqual(['', 'Capacity', 'Address']);
  });

  it('renders only the content column when every schema field is system-protected', () => {
    const { container } = render(TableView, {
      props: {
        nodeIds: [],
        schema: schemaWith('all-system', false, [
          field({ name: 'a', friendlyName: 'A', type: 'string', protection: 'system' }),
          field({ name: 'b', friendlyName: 'B', type: 'string', protection: 'system' })
        ]),
        fieldSchemaMap: new Map(),
        onRowClick: vi.fn()
      }
    });

    expect(headerTexts(container)).toEqual(['']);
  });

  it('leaves a schema with no system fields completely unchanged', () => {
    const { container } = render(TableView, {
      props: {
        nodeIds: [],
        schema: schemaWith('task', true, [
          field({ name: 'status', friendlyName: 'Status', type: 'enum', protection: 'core' }),
          field({ name: 'priority', friendlyName: 'Priority', type: 'enum', protection: 'core' }),
          field({ name: 'due_date', friendlyName: 'Due date', type: 'date', protection: 'core' })
        ]),
        fieldSchemaMap: new Map(),
        onRowClick: vi.fn()
      }
    });

    expect(headerTexts(container)).toEqual(['', 'Status', 'Priority', 'Due date']);
  });
});
