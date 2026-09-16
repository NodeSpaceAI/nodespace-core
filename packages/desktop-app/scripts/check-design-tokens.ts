#!/usr/bin/env bun

/**
 * Design-token lint gate.
 *
 * Runs Stylelint (stylelint.config.js) over the CSS in this package, including
 * the `<style>` blocks inside .svelte files that ESLint cannot parse, and
 * compares the result against a recorded baseline of known violations.
 *
 * Why a baseline: the design-system audit found ~250 pre-existing violations
 * across ~44 files. Failing the mandatory quality gate on all of them would
 * block every unrelated change until the remediation issues land. Instead the
 * gate fails only on violations that are *not* in the baseline, so new drift is
 * caught immediately while the existing backlog is worked down separately.
 *
 * The baseline is a ratchet: it is an error for it to contain entries that no
 * longer violate, so fixing a file forces the baseline down and the count can
 * never silently grow back.
 *
 *   bun run quality:design-tokens           # check against baseline
 *   bun run quality:design-tokens --update  # re-record the baseline
 *   bun run quality:design-tokens --list    # print current violations
 */

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import stylelint from 'stylelint';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const baselineFile = path.join(packageRoot, 'design-tokens-baseline.json');

/** A violation identified by location-independent fields, so that edits
 *  elsewhere in a file do not invalidate unrelated baseline entries. */
type Violation = {
  file: string;
  rule: string;
  text: string;
};

type Baseline = {
  /** Human-facing note; ignored when comparing. */
  readme: string;
  violations: Violation[];
};

export type { Violation };

export function key(v: Violation): string {
  // U+001F (unit separator) cannot occur in a path or a Stylelint message, so
  // two different violations can never collide into one key. Written as an
  // escape rather than a literal control character to keep this file plain
  // text — a raw one makes grep and other tooling treat the source as binary.
  return `${v.file}\u001F${v.rule}\u001F${v.text}`;
}

/**
 * Compares current violations against the baseline as a multiset, so that N
 * identical violations in one file are not masked by a single baseline entry.
 *
 * `added` are violations to fail on; `fixed` is how many baseline entries no
 * longer occur, which must force the baseline to be re-recorded (the ratchet).
 */
export function diffAgainstBaseline(
  current: readonly Violation[],
  baseline: readonly Violation[]
): { added: Violation[]; fixed: number } {
  const remaining = new Map<string, number>();
  for (const v of baseline) {
    remaining.set(key(v), (remaining.get(key(v)) ?? 0) + 1);
  }

  const added: Violation[] = [];
  for (const v of current) {
    const count = remaining.get(key(v)) ?? 0;
    if (count > 0) {
      remaining.set(key(v), count - 1);
    } else {
      added.push(v);
    }
  }

  const fixed = [...remaining.values()].reduce((sum, n) => sum + n, 0);
  return { added, fixed };
}

async function collectViolations(): Promise<Violation[]> {
  const { results } = await stylelint.lint({
    cwd: packageRoot,
    files: ['src/**/*.css', 'src/**/*.svelte'],
    configFile: path.join(packageRoot, 'stylelint.config.js')
  });

  const violations: Violation[] = [];
  for (const result of results) {
    const file = path.relative(packageRoot, result.source ?? '');
    for (const warning of result.warnings) {
      violations.push({ file, rule: warning.rule, text: warning.text });
    }
  }

  return violations.sort((a, b) => key(a).localeCompare(key(b)));
}

function readBaseline(): Baseline {
  if (!existsSync(baselineFile)) {
    return { readme: '', violations: [] };
  }
  return JSON.parse(readFileSync(baselineFile, 'utf8'));
}

async function main(): Promise<number> {
  const args = new Set(process.argv.slice(2));
  const violations = await collectViolations();

  if (args.has('--list')) {
    for (const v of violations) console.log(`${v.file}: ${v.text}`);
    console.log(`\n${violations.length} violations`);
    return 0;
  }

  if (args.has('--update')) {
    const baseline: Baseline = {
      readme:
        'Known design-token violations, recorded so the quality gate fails only on NEW drift. ' +
        'Do not add entries by hand and do not re-run --update to silence a new violation: ' +
        'fix it instead. Regenerate only after legitimately removing violations, which shrinks ' +
        'this list. See packages/desktop-app/stylelint.config.js.',
      violations
    };
    writeFileSync(baselineFile, `${JSON.stringify(baseline, null, 2)}\n`);
    console.log(`Recorded ${violations.length} violations to ${path.basename(baselineFile)}`);
    return 0;
  }

  const baseline = readBaseline();
  const { added, fixed } = diffAgainstBaseline(violations, baseline.violations);

  if (added.length > 0) {
    console.error(`\n✗ ${added.length} new design-token violation(s):\n`);
    for (const v of added) {
      console.error(`  ${v.file}`);
      console.error(`    ${v.text}\n`);
    }
    console.error('These break the design system spec (DESIGN.md in the docs repo).');
    console.error('Fix them — do not re-record the baseline to make this pass.\n');
    return 1;
  }

  if (fixed > 0) {
    console.error(`\n✗ ${fixed} baseline violation(s) no longer occur.\n`);
    console.error('The baseline is a ratchet, so it has to shrink when violations are fixed.');
    console.error(
      'Re-record it with: bun run --cwd packages/desktop-app quality:design-tokens --update\n'
    );
    return 1;
  }

  console.log(`✓ No new design-token violations (${violations.length} known, tracked in baseline)`);
  return 0;
}

// Only run the CLI when invoked directly, so tests can import the helpers.
if (import.meta.main) {
  process.exit(await main());
}
