import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { InstallReport, StepReport } from '$lib/services/methodology-service';
import {
  failureMessage,
  installMethodology,
  listMethodologies,
  renamedIds,
  summarizeReport
} from '$lib/services/methodology-service';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { invoke } = await import('@tauri-apps/api/core');
const mockInvoke = vi.mocked(invoke);

function report(steps: StepReport[], success = true): InstallReport {
  return { recipeId: 'linear', steps, success };
}

const created = (label: string, id: string): StepReport => ({
  label,
  outcome: { kind: 'created', id }
});
const suffixed = (label: string, requested: string, createdId: string): StepReport => ({
  label,
  outcome: { kind: 'suffixed', requested, created: createdId }
});
const failed = (label: string, message: string): StepReport => ({
  label,
  outcome: { kind: 'failed', message }
});
const skipped = (label: string): StepReport => ({ label, outcome: { kind: 'skipped' } });

beforeEach(() => {
  vi.clearAllMocks();
});

describe('listMethodologies', () => {
  it('returns what the backend offers', async () => {
    mockInvoke.mockResolvedValue([{ id: 'linear', name: 'Linear-style', description: 'x' }]);
    await expect(listMethodologies()).resolves.toHaveLength(1);
    expect(mockInvoke).toHaveBeenCalledWith('list_methodologies');
  });
});

describe('installMethodology', () => {
  it('passes the id through in the shape the command expects', async () => {
    mockInvoke.mockResolvedValue(report([created('Create `issue` schema', 'issue')]));
    await installMethodology('linear');
    expect(mockInvoke).toHaveBeenCalledWith('install_methodology', { methodologyId: 'linear' });
  });

  // A partial install is a resolved promise carrying success: false. If this
  // rejected instead, the report — the only record of what landed — would be
  // lost at exactly the moment the user needs it.
  it('resolves rather than rejecting when a step failed', async () => {
    mockInvoke.mockResolvedValue(
      report([created('a', 'a'), failed('b', 'nope'), skipped('c')], false)
    );
    const result = await installMethodology('linear');
    expect(result.success).toBe(false);
    expect(failureMessage(result)).toBe('b: nope');
  });
});

describe('renamedIds', () => {
  it('reports every re-keyed id', () => {
    const r = report([
      created('Create `issue` schema', 'issue'),
      suffixed('Create `cycle` schema', 'cycle', 'cycle__2')
    ]);
    expect(renamedIds(r)).toEqual([{ requested: 'cycle', created: 'cycle__2' }]);
  });

  it('is empty when nothing collided', () => {
    expect(renamedIds(report([created('a', 'a')]))).toEqual([]);
  });
});

describe('summarizeReport', () => {
  it('states the count when everything landed cleanly', () => {
    expect(summarizeReport(report([created('a', 'a'), created('b', 'b')]))).toBe(
      'Installed 2 items.'
    );
  });

  // The acceptance criterion: a collision is disclosed, never silent. A user
  // whose own `cycle` was left alone has to be told the new one is called
  // something else, or they will look for fields on the wrong type.
  it('names both ids and says existing types were untouched', () => {
    const summary = summarizeReport(
      report([created('Create `issue` schema', 'issue'), suffixed('Create `cycle` schema', 'cycle', 'cycle__2')])
    );
    expect(summary).toContain('cycle → cycle__2');
    expect(summary).toContain('left unchanged');
  });

  it('lists several renames together', () => {
    const summary = summarizeReport(
      report([suffixed('a', 'issue', 'issue__2'), suffixed('b', 'cycle', 'cycle__2')])
    );
    expect(summary).toContain('issue → issue__2');
    expect(summary).toContain('cycle → cycle__2');
  });

  it('says how far a partial install got', () => {
    const summary = summarizeReport(
      report([created('a', 'a'), failed('b', 'boom'), skipped('c')], false)
    );
    expect(summary).toContain('Stopped after 1 of 3 steps');
    expect(summary).toContain('boom');
  });

  it('distinguishes an install where nothing landed at all', () => {
    const summary = summarizeReport(report([failed('a', 'boom'), skipped('b')], false));
    expect(summary).toContain('Nothing was installed');
    expect(summary).not.toContain('Stopped after');
  });
});
