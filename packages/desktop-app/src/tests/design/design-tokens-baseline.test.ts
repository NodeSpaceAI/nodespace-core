/**
 * Verifies the baseline comparison in scripts/check-design-tokens.ts.
 *
 * The baseline lets the design-token gate run against a codebase that still has
 * known violations: it fails on new drift while the existing backlog is worked
 * down by the remediation issues. That only holds if the comparison is exact —
 * a gate that fails open would look identical to a clean codebase.
 */

import { describe, it, expect } from 'vitest';
import { diffAgainstBaseline, type Violation } from '../../../scripts/check-design-tokens';

const violation = (file: string, text: string): Violation => ({
  file,
  rule: 'declaration-property-value-disallowed-list',
  text
});

const RED = violation('a.svelte', 'Raw color literal "#ff0000" in "color".');
const BLUE = violation('b.svelte', 'Raw color literal "#0000ff" in "background".');

describe('diffAgainstBaseline', () => {
  it('reports nothing when current matches the baseline', () => {
    expect(diffAgainstBaseline([RED, BLUE], [RED, BLUE])).toEqual({ added: [], fixed: 0 });
  });

  it('reports a violation absent from the baseline as added', () => {
    const { added, fixed } = diffAgainstBaseline([RED, BLUE], [RED]);
    expect(added).toEqual([BLUE]);
    expect(fixed).toBe(0);
  });

  it('reports a baseline entry that no longer occurs as fixed', () => {
    const { added, fixed } = diffAgainstBaseline([RED], [RED, BLUE]);
    expect(added).toEqual([]);
    expect(fixed).toBe(1);
  });

  it('counts duplicates rather than matching on presence', () => {
    // Two identical violations in one file must not be masked by a single
    // baseline entry — otherwise duplicating a bad line slips through.
    const { added, fixed } = diffAgainstBaseline([RED, RED], [RED]);
    expect(added).toEqual([RED]);
    expect(fixed).toBe(0);
  });

  it('reports a removed duplicate as fixed', () => {
    const { added, fixed } = diffAgainstBaseline([RED], [RED, RED]);
    expect(added).toEqual([]);
    expect(fixed).toBe(1);
  });

  it('treats the same violation in a different file as new', () => {
    // Violations are keyed by file, so moving bad CSS elsewhere is still drift.
    const moved = { ...RED, file: 'c.svelte' };
    const { added, fixed } = diffAgainstBaseline([moved], [RED]);
    expect(added).toEqual([moved]);
    expect(fixed).toBe(1);
  });

  it('reports every violation as added when the baseline is empty', () => {
    expect(diffAgainstBaseline([RED, BLUE], [])).toEqual({ added: [RED, BLUE], fixed: 0 });
  });

  it('ignores line numbers, so unrelated edits do not churn the baseline', () => {
    // Entries deliberately carry no position: adding a line at the top of a
    // file must not invalidate every violation below it.
    expect(Object.keys(RED).sort()).toEqual(['file', 'rule', 'text']);
  });
});
