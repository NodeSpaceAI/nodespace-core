import { describe, it, expect, beforeEach } from 'vitest';
import {
  loadDefaultViewPrefs,
  saveDefaultViewPrefs
} from '$lib/components/query/default-view-prefs';
import { resolveEffectiveView } from '$lib/components/query/query-node-model';

describe('default view prefs', () => {
  beforeEach(() => localStorage.clear());

  it('falls back to the table view when nothing is stored', () => {
    expect(loadDefaultViewPrefs('skill')).toEqual({ lastView: 'table' });
  });

  it('round-trips view and group-by per type', () => {
    saveDefaultViewPrefs('skill', { lastView: 'kanban', kanban: { groupBy: 'status' } });
    saveDefaultViewPrefs('task', { lastView: 'list' });
    expect(loadDefaultViewPrefs('skill')).toEqual({
      lastView: 'kanban',
      kanban: { groupBy: 'status' }
    });
    expect(loadDefaultViewPrefs('task')).toEqual({ lastView: 'list' });
  });

  it('round-trips a kanban column order', () => {
    const config = {
      lastView: 'kanban' as const,
      kanban: { groupBy: 'status', columnOrder: { status: ['done', 'open'] } }
    };
    saveDefaultViewPrefs('skill', config);
    expect(loadDefaultViewPrefs('skill')).toEqual(config);
  });

  it('ignores corrupt stored values', () => {
    localStorage.setItem('nodespace:default-view:skill', '{not json');
    expect(loadDefaultViewPrefs('skill')).toEqual({ lastView: 'table' });
    localStorage.setItem('nodespace:default-view:skill', JSON.stringify({ lastView: 'bogus' }));
    expect(loadDefaultViewPrefs('skill')).toEqual({ lastView: 'table' });
  });
});

describe('resolveEffectiveView', () => {
  it('opens kanban in list when nothing can be grouped by', () => {
    expect(resolveEffectiveView('kanban', false)).toBe('list');
  });
  it('keeps every other combination', () => {
    expect(resolveEffectiveView('kanban', true)).toBe('kanban');
    expect(resolveEffectiveView('table', false)).toBe('table');
    expect(resolveEffectiveView('list', false)).toBe('list');
  });
});
