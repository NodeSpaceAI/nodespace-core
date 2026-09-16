/**
 * search-pane component.
 *
 * The Search view opened from the sidebar's Search nav item. Queries the
 * daemon's merged node search (`search_roots` — title/keyword matches and
 * semantic matches combined server-side into one ranked list) and opens a node
 * tab when a result is clicked. Regression coverage for the dead-Search-nav
 * bug.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/svelte';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

import SearchPane from '$lib/components/search/search-pane.svelte';
import { navigationStore, resetTabState } from '$lib/stores/navigation.svelte';

describe('SearchPane', () => {
  beforeEach(() => {
    resetTabState();
    mockInvoke.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('prompts the user to search before any query is entered', () => {
    mockInvoke.mockResolvedValue([]);
    const { getByPlaceholderText, getByText } = render(SearchPane);
    expect(getByPlaceholderText('Search nodes…')).toBeTruthy();
    expect(getByText('Type to search your nodes.')).toBeTruthy();
  });

  it('queries search_roots on Enter and lists the results', async () => {
    mockInvoke.mockResolvedValue([
      { id: 'n1', nodeType: 'text', content: 'Alpha doc' },
      { id: 'n2', nodeType: 'text', content: 'Beta doc' }
    ]);
    const { getByPlaceholderText, findByText } = render(SearchPane);

    const input = getByPlaceholderText('Search nodes…');
    await fireEvent.input(input, { target: { value: 'doc' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(await findByText('Alpha doc')).toBeTruthy();
    expect(await findByText('Beta doc')).toBeTruthy();
    expect(mockInvoke).toHaveBeenCalledWith('search_roots', {
      params: { query: 'doc', limit: 25 }
    });
  });

  it('surfaces a task node matched by title, which no embedding could have ranked', async () => {
    // Tasks are non-embeddable (ADR-029), so this row can only have come from
    // the keyword half of the merged search. Before the merge, the search box
    // reached an embeddings-only path and an exact task title returned nothing.
    mockInvoke.mockResolvedValue([
      { id: 'task-1', nodeType: 'task', content: 'Draft Q3 architecture review' },
      { id: 'n2', nodeType: 'text', content: 'Notes from the Q3 planning session' }
    ]);
    const { getByPlaceholderText, findByText } = render(SearchPane);

    const input = getByPlaceholderText('Search nodes…');
    await fireEvent.input(input, { target: { value: 'Draft Q3 architecture review' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(await findByText('Draft Q3 architecture review')).toBeTruthy();
    expect(await findByText('Notes from the Q3 planning session')).toBeTruthy();
  });

  it('renders the merged list in the order the backend ranked it', async () => {
    // The backend returns one already-ranked list (keyword hits banded above
    // semantic hits). The pane must not re-sort or regroup it.
    mockInvoke.mockResolvedValue([
      { id: 'exact', nodeType: 'task', content: 'Migration plan' },
      { id: 'partial', nodeType: 'text', content: 'Migration plan review notes' },
      { id: 'semantic', nodeType: 'text', content: 'Cutover sequencing decisions' }
    ]);
    const { getByPlaceholderText, findByText, container } = render(SearchPane);

    const input = getByPlaceholderText('Search nodes…');
    await fireEvent.input(input, { target: { value: 'Migration plan' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await findByText('Migration plan');
    const rendered = [...container.querySelectorAll('.result-list .result-title')].map((el) =>
      el.textContent?.trim()
    );
    expect(rendered).toEqual([
      'Migration plan',
      'Migration plan review notes',
      'Cutover sequencing decisions'
    ]);
  });

  it('opens a node tab when a result is clicked', async () => {
    mockInvoke.mockResolvedValue([{ id: 'n1', nodeType: 'text', content: 'Alpha doc' }]);
    const { getByPlaceholderText, findByText } = render(SearchPane);

    const input = getByPlaceholderText('Search nodes…');
    await fireEvent.input(input, { target: { value: 'alpha' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await fireEvent.click(await findByText('Alpha doc'));

    const opened = navigationStore.state.tabs.find((t) => t.content?.nodeId === 'n1');
    expect(opened).toBeTruthy();
    expect(opened?.type).toBe('node');
  });

  it('shows an empty state when there are no matches', async () => {
    mockInvoke.mockResolvedValue([]);
    const { getByPlaceholderText, findByText } = render(SearchPane);

    const input = getByPlaceholderText('Search nodes…');
    await fireEvent.input(input, { target: { value: 'zzz' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    expect(await findByText(/No results for/)).toBeTruthy();
  });

  it('surfaces the real CommandError message when search_roots rejects with a plain object, not "[object Object]"', async () => {
    // What Tauri hands back for a Rust `Result<_, CommandError>` `Err` — a
    // plain object, never an Error instance.
    mockInvoke.mockRejectedValue({
      message: 'Search index is still building',
      code: 'INDEX_NOT_READY'
    });
    const { getByPlaceholderText, findByText } = render(SearchPane);

    const input = getByPlaceholderText('Search nodes…');
    await fireEvent.input(input, { target: { value: 'doc' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    const status = await findByText('Search index is still building');
    expect(status.textContent).not.toContain('[object Object]');
  });
});
