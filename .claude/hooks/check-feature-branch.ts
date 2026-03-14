#!/usr/bin/env bun
/**
 * PreToolUse hook: warns when editing files on the main branch.
 *
 * The startup sequence requires creating a feature branch before modifying
 * any files. Being on `main` is a reliable signal that the sequence was skipped.
 *
 * Exits 0 in all cases (never blocks), but prints a warning to stderr when on
 * `main` so Claude Code surfaces it as a message to the agent — prompting
 * self-correction without hard-blocking legitimate operations.
 */

import { execSync } from 'child_process';

function getCurrentBranch(): string {
  try {
    return execSync('git branch --show-current', { encoding: 'utf8' }).trim();
  } catch {
    // Not a git repo or git not available — allow through
    return '';
  }
}

const branch = getCurrentBranch();

if (branch === 'main') {
  process.stderr.write(`
⚠️  WARNING: You are on the \`main\` branch.

Have you completed the startup sequence before editing files?

  1. git fetch origin && git pull origin main
  2. bun install
  3. bun run test  (record baseline)
  4. bun run gh:comment <N> "Frontend: X passed"
  5. git checkout -b feature/issue-<N>-description
  6. bun run gh:assign <N> "@me"
  7. bun run gh:status <N> "In Progress"

If not, STOP and complete the sequence first, then create a feature branch.
`);
}

process.exit(0);
