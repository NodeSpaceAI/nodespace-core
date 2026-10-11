#!/usr/bin/env bun
/**
 * PreToolUse hook: reminds the agent of the startup sequence when it edits
 * files on the `main` branch.
 *
 * Issue work happens in an isolated worktree, whose branch is never `main`, so
 * the hook is silent there. On `main` it prints the sequence, because being on
 * `main` often means the sequence was skipped. Not every edit on `main` is issue
 * work, so the reminder does not tell the agent to stop.
 *
 * Exits 0 in all cases (never blocks). The reminder goes to stderr so Claude
 * Code surfaces it as a message to the agent.
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
⚠️  REMINDER: You are editing on the \`main\` branch of the primary checkout.

Work on an issue belongs in an isolated worktree. If this edit is issue work,
complete the startup sequence first, then make the edit inside the worktree.
If it is not issue work, ignore this reminder.

  1. git status  (commit any pending changes)
  2. git fetch origin && git pull origin main
  3. EnterWorktree({name: "issue-<N>-brief-desc"})
  4. bun install
  5. bun run test  (record baseline)
  6. bun run gh:comment <N> "Frontend: X passed"
  7. bun run gh:assign <N> "@me"
  8. bun run gh:status <N> "In Progress"

Detail: AGENTS.md, "Mandatory Startup Sequence".
`);
}

process.exit(0);
