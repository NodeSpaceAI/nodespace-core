#!/usr/bin/env bun
/**
 * PreToolUse hook: reminds the agent to embed the startup sequence in any plan.
 *
 * Fires before EnterPlanMode. Prints a reminder to stderr so Claude Code
 * surfaces it as a message — ensuring the agent includes the startup sequence
 * as Step 0 in the plan it is about to write.
 *
 * Exits 0 always (non-blocking).
 */

process.stderr.write(`
⚠️  PLAN MODE REMINDER: Your plan MUST include the startup sequence as Step 0.

Because context clears between planning and implementation, the implementing
agent will ONLY see the plan — not CLAUDE.md or this conversation.

Include this as Step 0 in your plan:
  Step 0: Complete startup sequence
    - git status (commit any pending changes)
    - git fetch origin && git pull origin main
    - bun install
    - bun run test  →  record baseline
    - bun run gh:comment <N> "Frontend: X passed"
    - git checkout -b feature/issue-<N>-description
    - bun run gh:assign <N> "@me"
    - bun run gh:status <N> "In Progress"

Also include at the end of the plan:
  Final steps: bun run test:all (no new failures), bun run quality:fix, bun run gh:pr <N>
`);

process.exit(0);
