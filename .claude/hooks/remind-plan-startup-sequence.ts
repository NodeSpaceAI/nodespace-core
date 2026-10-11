#!/usr/bin/env bun
/**
 * PreToolUse hook: reminds the agent to embed the startup sequence and the
 * final steps in any plan.
 *
 * Fires before EnterPlanMode. Prints a reminder to stderr so Claude Code
 * surfaces it as a message — ensuring the agent includes the startup sequence
 * as Step 0 and the final steps at the end of the plan it is about to write.
 *
 * Exits 0 always (non-blocking).
 */

process.stderr.write(`
⚠️  PLAN MODE REMINDER: Your plan MUST include the startup sequence as Step 0
and the final steps at its end.

Because context clears between planning and implementation, the implementing
agent will ONLY see the plan, not this conversation.

Include this as Step 0 in your plan:
  Step 0: Complete startup sequence
    - git status (commit any pending changes), on the primary checkout
    - git fetch origin && git pull origin main
    - EnterWorktree({name: "issue-<N>-brief-desc"})  (the tool owns the location and branch name)
    - bun install
    - bun run test  →  record baseline
    - bun run gh:comment <N> "Frontend: X passed"
    - bun run gh:assign <N> "@me"
    - bun run gh:status <N> "In Progress"

Also include at the end of the plan:
  Final steps:
    - bun run test:changed
    - bun run quality:fix, then commit
    - git push origin HEAD:issue-<N>-brief-desc  (lint only)
    - gh pr create --head issue-<N>-brief-desc
    - after the user approves the merge: bun run merge <PR#>,
      then ExitWorktree({action: "remove", discard_changes: true})

Inline the standards the implementing agent needs as well.

Detail: AGENTS.md, "Plan Mode" and "Mandatory Startup Sequence".
`);

process.exit(0);
