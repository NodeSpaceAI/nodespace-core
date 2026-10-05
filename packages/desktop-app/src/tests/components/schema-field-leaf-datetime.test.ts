/**
 * SchemaFieldLeaf's `datetime` control: a calendar popover for the day and a
 * time input beside it, together editing one stored RFC 3339 instant shown in
 * the user's local time.
 *
 * Stored values are built from local-time parts so the expectations hold in
 * whatever time zone the suite runs.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import type { SchemaField } from '$lib/types/schema-node';

import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () => mockTauriCore());

import SchemaFieldLeaf from '$lib/components/schema/schema-field-leaf.svelte';
import NestedFieldEditor from '$lib/components/schema/nested-field-editor.svelte';

function field(partial: Partial<SchemaField> & { name: string; type: string }): SchemaField {
  return { protection: 'user', indexed: false, friendlyName: partial.name, ...partial };
}

const stored = (...parts: [number, number, number, number, number]) => new Date(...parts).toISOString();

const visitedAt = field({ name: 'visited_at', friendlyName: 'Visited at', type: 'datetime' });

/** Open the day picker and return the calendar cell for a day of the shown month. */
async function openCalendarDay(trigger: Element, isoDay: string): Promise<Element> {
  await fireEvent.click(trigger);
  return waitFor(() => {
    const day = document.querySelector(`[data-bits-day][data-value="${isoDay}"]`);
    expect(day).toBeTruthy();
    return day as Element;
  });
}

afterEach(() => {
  cleanup();
});

describe('SchemaFieldLeaf — datetime', () => {
  it('shows the stored value as its local day and time, not the fallback', () => {
    const { getByText, getByLabelText, queryByText } = render(SchemaFieldLeaf, {
      props: { field: visitedAt, value: stored(2026, 2, 1, 9, 30), onChange: vi.fn(), fieldId: 'f' }
    });
    expect(queryByText(/Unknown field type/)).toBeNull();
    expect(getByText('2026-03-01')).toBeTruthy();
    expect((getByLabelText('Visited at time') as HTMLInputElement).value).toBe('09:30');
  });

  it('shows a placeholder and an empty time for an unset value', () => {
    const { getByText, getByLabelText } = render(SchemaFieldLeaf, {
      props: { field: visitedAt, value: null, onChange: vi.fn(), fieldId: 'f' }
    });
    expect(getByText('Pick a date')).toBeTruthy();
    expect((getByLabelText('Visited at time') as HTMLInputElement).value).toBe('');
  });

  it('writes one RFC 3339 date-time when the time changes, keeping the day', async () => {
    const onChange = vi.fn();
    const { getByLabelText } = render(SchemaFieldLeaf, {
      props: { field: visitedAt, value: stored(2026, 2, 1, 9, 30), onChange, fieldId: 'f' }
    });
    await fireEvent.change(getByLabelText('Visited at time'), { target: { value: '14:45' } });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(stored(2026, 2, 1, 14, 45));
  });

  it('writes nothing when the time is emptied, and shows the stored time again', async () => {
    const onChange = vi.fn();
    const { getByLabelText } = render(SchemaFieldLeaf, {
      props: { field: visitedAt, value: stored(2026, 2, 1, 9, 30), onChange, fieldId: 'f' }
    });
    const time = getByLabelText('Visited at time') as HTMLInputElement;
    await fireEvent.change(time, { target: { value: '' } });
    expect(onChange).not.toHaveBeenCalled();
    expect(time.value).toBe('09:30');
  });

  it('writes one RFC 3339 date-time when the day changes, keeping the time', async () => {
    const onChange = vi.fn();
    const { container } = render(SchemaFieldLeaf, {
      props: { field: visitedAt, value: stored(2026, 2, 1, 9, 30), onChange, fieldId: 'f' }
    });
    const day = await openCalendarDay(container.querySelector('#f') as Element, '2026-03-20');
    await fireEvent.click(day);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(stored(2026, 2, 20, 9, 30));
  });

  it('clears the value when the selected day is clicked again', async () => {
    const onChange = vi.fn();
    const { container } = render(SchemaFieldLeaf, {
      props: { field: visitedAt, value: stored(2026, 2, 1, 9, 30), onChange, fieldId: 'f' }
    });
    const day = await openCalendarDay(container.querySelector('#f') as Element, '2026-03-01');
    await fireEvent.click(day);
    expect(onChange).toHaveBeenCalledWith(null);
  });
});

describe('NestedFieldEditor — datetime', () => {
  it('renders a datetime sub-field of an object with its control', () => {
    const visit = field({ name: 'visit', type: 'object', fields: [visitedAt] });
    const { getByText, getByLabelText, queryByText } = render(NestedFieldEditor, {
      props: { field: visit, value: { visited_at: stored(2026, 2, 1, 9, 30) }, onChange: vi.fn() }
    });
    expect(queryByText(/Unknown field type/)).toBeNull();
    expect(getByText('2026-03-01')).toBeTruthy();
    expect((getByLabelText('Visited at time') as HTMLInputElement).value).toBe('09:30');
  });

  const visits = field({ name: 'visits', friendlyName: 'Visits', type: 'array', itemType: 'datetime' });

  it('adds an item at the current time and renders it as an editable control', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    try {
      vi.setSystemTime(new Date(2026, 9, 5, 14, 30, 0));
      const onChange = vi.fn();
      const { getByText, getByLabelText, rerender } = render(NestedFieldEditor, {
        props: { field: visits, value: [], onChange }
      });
      await fireEvent.click(getByText('Add item'));
      const added = [stored(2026, 9, 5, 14, 30)];
      expect(onChange).toHaveBeenCalledWith(added);

      await rerender({ field: visits, value: added, onChange });
      expect((getByLabelText('Visits 1 time') as HTMLInputElement).value).toBe('14:30');
      await fireEvent.change(getByLabelText('Visits 1 time'), { target: { value: '16:00' } });
      expect(onChange).toHaveBeenLastCalledWith([stored(2026, 9, 5, 16, 0)]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps an item when its selected day is clicked again: an element is removed, never emptied', async () => {
    const onChange = vi.fn();
    const { container } = render(NestedFieldEditor, {
      props: { field: visits, value: [stored(2026, 2, 1, 9, 30)], onChange }
    });
    const day = await openCalendarDay(container.querySelector('#nested-visits-0-0') as Element, '2026-03-01');
    await fireEvent.click(day);
    expect(onChange).not.toHaveBeenCalled();
  });
});
