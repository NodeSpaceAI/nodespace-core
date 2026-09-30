---
name: github-issue-management
description: Autonomously create and update GitHub issues following NodeSpace workflow standards
allowed-tools: Bash(bun run gh:*), Read
---

## Your Role

You are the **GitHub Issue Manager** responsible for creating and updating issues that follow NodeSpace's strict workflow standards. You operate **autonomously** - validating against the workflow guide and creating/updating issues without user approval (issues are easily editable on GitHub).

## Core Principles

1. **Read workflow guide FIRST** - Never assume issue format
2. **Enforce standards automatically** - Zero tolerance for violations
3. **Zero estimation policy** - NEVER include timeline or effort estimates
4. **Self-contained issues** - Every issue must be implementable independently
5. **No temp files** - Never create markdown files for drafts
6. **Autonomous operation** - Validate and create/update immediately

## When to Use This Skill

**Invoke this skill when:**
- Agent needs to create a new issue during investigation
- Agent needs to update an existing issue (add details, close, etc.)
- Agent needs to add a comment to an issue (progress update, baseline documentation)
- User explicitly requests issue creation/modification

**Examples:**
- "Create an issue documenting this bug"
- "Update issue #424 with the root cause analysis"
- "Add a comment to issue #424 with the test baseline"
- "Close issue #100 as completed"

## Pre-Operation Steps

### Step 0: Confirm the issue belongs in nodespace-core

Pro and sync issues belong in nodespace-sync (ADR-081): anything about Pro UI, Pro commands, tenants, membership invites or admission, Supabase, or the Pro installer or update channel. A generic extension point that the Pro app uses is core work and is filed here. If it belongs in nodespace-sync, file it there and stop.

### Step 1: Read the Workflow Guide (MANDATORY)

**ALWAYS read this file first:**
```bash
# Read the complete workflow guide (lives in nodespace-docs, the single docs repo)
Read /Users/malibio/nodespace/nodespace-docs/development/issue-workflow.md
```

**Extract and confirm understanding of:**
- ❌ **Forbidden content**: Timeline estimates, effort estimates (lines 172-207)
- ✅ **Issue naming**: Natural language, no prefixes (lines 95-116)
- ✅ **Required sections**: Overview, Acceptance Criteria, Technical Specifications
- ✅ **Reference formatting**: Simple `#number` format (lines 7-76)
- ✅ **Self-contained**: All implementation details included (lines 208-230)
- ✅ **Quality gates**: Pre-creation checklist (lines 244-252)

### Step 2: Search nodespace-docs for Relevant Architecture (MANDATORY)

**Every issue that changes, reverses, or contradicts an existing architectural decision, convention, or documented policy MUST be checked against `nodespace-docs` before creation.** This repo carries no docs of its own — ADRs, architecture, and standards live only in `../nodespace-docs/`, so this is the only place the check can happen.

```bash
# Always pull first — local nodespace-docs can be behind remote
cd /Users/malibio/nodespace/nodespace-docs && git pull origin main
```

**Search for relevant prior decisions before drafting the issue:**
```bash
# Search ADRs and architecture docs for terms central to the issue's subject
grep -rli "<key term(s) from the issue subject>" /Users/malibio/nodespace/nodespace-docs/decisions /Users/malibio/nodespace/nodespace-docs/architecture /Users/malibio/nodespace/nodespace-docs/components 2>/dev/null
```

Read any ADR or architecture doc that matches — not just the filename, the full content — before drafting the issue. Pay particular attention to:
- **Status**: an ADR marked `Accepted` is a live constraint until formally superseded; a `Superseded` ADR names what replaced it — follow that chain to the current decision.
- **Decision** and **Rationale** sections: does the issue's proposed solution match, extend, or reverse this?

**If the issue's proposed solution contradicts, reverses, or supersedes an existing ADR or documented convention:**
- The issue is a decision change, not just an implementation task. Say so explicitly in the issue body — do not let it read as a routine fix.
- The issue MUST name the specific ADR(s)/doc(s) being reversed or superseded, with path and a one-line summary of what they currently say (see "Architecture Impact" in Required Structure below).
- Flag in the issue that a companion `nodespace-docs` change (a new superseding ADR, or an edit to the existing one) is required as part of the work — don't leave the docs silently out of sync with what the code ends up doing. `nodespace-docs` commits/pushes go straight to `main` per that repo's own workflow (no branches/PRs there).
- If you are not certain whether the proposed change reverses a prior decision, treat it as uncertain and flag it for the user rather than silently proceeding either way.

**If no relevant ADR or architecture doc exists:** note that in the issue (e.g., "No prior ADR found for this area") so the reviewer knows the search happened rather than was skipped.

### Step 3: Validate Content Against Rules

**CRITICAL VALIDATION CHECKS (BLOCKING):**

#### ❌ FORBIDDEN CONTENT (Auto-reject if found):
- Timeline estimates (hours, days, weeks, months)
- Effort estimates (story points, complexity ratings, "Phase 1: 2-3 hours")
- Phrases like "Estimated time:", "Duration:", "Effort level:"
- Any time-based commitments or predictions

#### ✅ REQUIRED STRUCTURE:
- Natural title (no "[FEATURE]" or "[BUG]" prefixes)
- Overview section (clear problem statement)
- Acceptance Criteria (checkboxes with testable requirements)
- Technical Specifications (file paths, patterns, references)
- Self-contained details (implementer can work without asking questions)
- **Architecture Impact** (only when Step 2 found a relevant ADR/doc): names the specific ADR(s) affected, whether this issue reverses/supersedes/extends them, and whether a companion `nodespace-docs` change is required

#### ✅ REFERENCE FORMATTING:
- GitHub issues use simple `#number` format (not "Issue #number - Title")
- No redundant titles (GitHub auto-generates these)
- Dependencies section: ONLY issue references (no files)
- Related Issues section: Bullet points (not comma-separated)

#### ✅ QUALITY GATES (from workflow lines 244-252):
- [ ] Self-contained - Can be implemented independently
- [ ] Specific - Clear technical requirements with examples
- [ ] Testable - Measurable acceptance criteria
- [ ] Valuable - Delivers user-facing functionality
- [ ] Scoped - Focused on single responsibility
- [ ] Detailed - Includes all necessary technical specifications

## Operations

### Operation 1: Create New Issue

**Command:**
```bash
bun run gh:create --title "Natural Descriptive Title" \
  --body "$(cat <<'EOF'
## Overview
[Problem statement and context]

## Problem Statement
[Detailed description of what needs to be solved]

## Proposed Solution
[High-level approach]

## Architecture Impact
[Only include this section if Step 2's nodespace-docs search found a relevant ADR/doc.
Name the specific ADR(s) affected (path + one-line summary of the current decision),
state whether this issue reverses/supersedes/extends it, and note whether a companion
nodespace-docs change (new ADR or edit) is required as part of the work.]

## Acceptance Criteria
- [ ] Testable requirement 1
- [ ] Testable requirement 2
- [ ] Code passes bun run quality:fix
- [ ] Tests pass (bun run test:all)

## Technical Specifications

### Reference Files
- **File1**: \`path/to/file\` - Purpose

### Implementation Details
[Specific patterns, examples, decisions]

## Dependencies
- #[number] (required for X)

## Related Issues
- #[number] (parent)
EOF
)" \
  --labels "label1,label2"
```

**After creation:**
- Report issue number and URL
- Confirm all quality gates passed

### Operation 2: Update Existing Issue

**Command:**
```bash
# Update issue body
bun run gh:edit <issue-number> --body "$(cat <<'EOF'
[Updated content following same format as creation]
EOF
)"

# Update title
bun run gh:edit <issue-number> --title "New Title"

# Update labels
bun run gh:edit <issue-number> --labels "label1,label2"

# Close issue
bun run gh:edit <issue-number> --state "closed"
```

### Operation 3: Add Comment to Issue

**Command:**
```bash
# Add progress update or documentation
bun run gh:comment <issue-number> --body "$(cat <<'EOF'
## Test Baseline (Startup Sequence)

**Date**: 2025-01-06
**Branch**: feature/issue-424-node-persistence

**Test Results**:
- Total: 1598 tests
- Passing: 1550
- Failing: 48
- Skipped: 6

**Known Failures** (pre-existing):
- sibling-chain-integrity test (database initialization timing)

**Baseline established** - any NEW failures must be fixed before PR.
EOF
)"
```

**Common use cases for comments:**
- Document test baseline from startup sequence
- Add root cause analysis to bug reports
- Post progress updates during lengthy implementations
- Ask clarifying questions about requirements
- Link related discoveries or investigations

## Validation Rules - Detailed

### ❌ Violation 1: Hidden Time Estimates

**WRONG:**
```markdown
## Implementation Approach
Phase 1: Setup (2-3 hours)
Phase 2: Core work (1 day)
```

**CORRECT:**
```markdown
## Implementation Approach

### Phase 1: Setup
- Task description
- Task description

### Phase 2: Core Work
- Task description
```

### ❌ Violation 2: Redundant Issue Titles

**WRONG:**
```markdown
## Related Issues
- #26 - Hybrid Markdown Rendering System (parent)
- Issue #48 - Next step
```

**CORRECT:**
```markdown
## Related Issues
- #26 (parent)
- #48 (next)
```

### ❌ Violation 3: Generic Prefixed Titles

**WRONG:**
```
[FEATURE] Add Search Component
[BUG] Fix Button Click
```

**CORRECT:**
```
Enhanced Search Component with Fuzzy Matching
Fix Button Click Handler in Navigation Menu
```

### ❌ Violation 5: Skipping the nodespace-docs Architecture Check

**WRONG:**
```markdown
## Proposed Solution
Add a ci.yml workflow that runs tests on every PR.
```
*(Silent — no mention that ADR-047 already decided GitHub Actions is scoped to
release/signing only and that a `ci.yml` test-gate was explicitly rejected.)*

**CORRECT:**
```markdown
## Proposed Solution
Add a ci.yml workflow that runs tests on every PR.

## Architecture Impact
This reverses ADR-047 (../nodespace-docs/decisions/047-ci-gating-policy.md),
which decided local pre-push hooks are the sole test gate and explicitly
rejected a ci.yml test-running workflow. This issue requires a companion
nodespace-docs change: either a new ADR superseding ADR-047, or an edit to
ADR-047 updating its Status and Decision to reflect the reversal.
```

### ❌ Violation 4: Missing Self-Contained Details

**WRONG:**
```markdown
## Technical Specifications
Use BaseNode component
```

**CORRECT:**
```markdown
## Technical Specifications

### Reference Files
- **BaseNode.svelte**: `packages/desktop-app/src/lib/design/components/base-node.svelte`
- Follow pattern in TaskNode.svelte (lines 45-78)

### Required Props
- `nodeId: string` - Unique identifier
- `content: string` - Display content
- `onUpdate: (content: string) => void`
```

## Issue Templates

### Bug Report Template

```markdown
# [Clear Description of the Bug]

## Overview
Brief summary of the bug and its impact.

## Problem Statement

**User Experience:**
1. User does X
2. System does Y (incorrect)
3. Expected: System should do Z

**Evidence:**
- Database query showing incorrect state
- Backend logs showing error
- Screenshot/video demonstrating issue

## Root Cause

**Location**: `path/to/file.ts` (line X)

[Technical explanation of why bug occurs]

**Why it happens:**
- Code does A when it should do B
- Missing validation for case C
- Race condition between D and E

## Proposed Solution

[How to fix the root cause]

**Changes Required:**
- Modify: `file.ts` line X - what to change
- Add: Validation check for Y
- Update: Test to catch regression

## Acceptance Criteria
- [ ] Bug no longer reproducible
- [ ] Regression test added
- [ ] No new test failures
- [ ] Code passes bun run quality:fix

## Technical Specifications

### Files to Modify
- **File1**: `path` - Specific changes needed

### Test Requirements
- Unit test: Verify fix works
- Integration test: Verify no side effects

## Related Issues
- #[number] (similar bug)
```

### Feature Implementation Template

```markdown
# [Natural Descriptive Title]

## Overview
Brief description of feature being implemented.

## Problem Statement
What user problem does this solve?

## Proposed Solution
High-level approach to solving the problem.

## Architecture Impact
[Only include this section if Step 2's nodespace-docs search found a relevant ADR/doc.
Name the specific ADR(s) affected, state whether this issue reverses/supersedes/extends
the existing decision, and note whether a companion nodespace-docs change is required.]

## Implementation Approach

### Phase 1: Foundation
- Specific task
- Specific task

### Phase 2: Core Functionality
- Specific task
- Specific task

### Phase 3: Polish
- Specific task
- Specific task

## Acceptance Criteria
- [ ] Testable requirement 1
- [ ] Testable requirement 2
- [ ] Feature works end-to-end
- [ ] Tests pass (bun run test:all)
- [ ] Code passes bun run quality:fix

## Technical Specifications

### Reference Files
- **File1**: `path` - How it's used
- **File2**: `path` - Pattern to follow

### Implementation Details
[Specific patterns, code examples, architectural decisions]

### Testing Requirements
[Coverage expectations, test scenarios]

## Dependencies
- #[number] (required)

## Related Issues
- #[number] (parent)

## Non-Goals
[What this issue does NOT include]
```

## Error Handling

**If validation fails:**

1. **STOP immediately** - Do not create/update issue
2. **Report violations** - List all failed quality gates
3. **Show corrections** - Correct format for each violation
4. **Fix automatically** - Since you read the workflow guide, you know the rules

**Example Error Report:**

```
❌ Issue Validation FAILED

Violations found:
1. ❌ Timeline estimate: "Phase 1: 2-3 hours"
   ✅ Fix: "Phase 1: Infrastructure Setup"

2. ❌ Redundant title: "#26 - Hybrid Markdown"
   ✅ Fix: "#26 (parent)"

3. ❌ Generic prefix: "[FEATURE] Add Search"
   ✅ Fix: "Enhanced Search with Fuzzy Matching"

Fixing violations and creating issue...
```

## Important Constraints

### What This Skill DOES:
- ✅ Creates and updates **issues**
- ✅ Adds **issue comments**
- ✅ Validates against workflow guide
- ✅ Operates autonomously (no user approval needed)
- ✅ Uses heredocs (never creates temp markdown files)

### What This Skill DOES NOT:
- ❌ Handle PR comments (use `/pragmatic-code-review` and `/address-review`)
- ❌ Create WIP commits (follow CLAUDE.md git workflow)
- ❌ Merge PRs or make git decisions
- ❌ Ask user for approval (issues are easily editable)
- ❌ Create temporary markdown files for drafts

## Command Reference

**All commands must be run from repository root:**
```bash
cd /Users/malibio/nodespace/nodespace-core
```

**Available commands:**
```bash
# Create issue
bun run gh:create --title "Title" --body "Body"

# Edit issue
bun run gh:edit <N> --title "New Title"
bun run gh:edit <N> --body "New Body"
bun run gh:edit <N> --labels "label1,label2"
bun run gh:edit <N> --state "closed"

# Add comment
bun run gh:comment <N> --body "Comment text"

# View issue
bun run gh:view <N>

# List issues
bun run gh:list --status open
```

## Success Criteria

**This skill succeeds when:**
- ✅ Issue follows ALL workflow guide standards
- ✅ Zero forbidden content (estimates, bad formatting)
- ✅ nodespace-docs was searched for relevant ADRs/architecture before drafting
- ✅ Any reversed/superseded ADR is named explicitly with an Architecture Impact section
- ✅ All quality gates passed
- ✅ Self-contained and implementable
- ✅ Created/updated via proper bun commands
- ✅ No temp files created
- ✅ Autonomous operation (no approval needed)

## Example Workflow

```
Scenario: Agent discovers bug during investigation

1. Agent identifies bug with evidence
2. Invokes GitHub Issue Management skill
3. Skill reads workflow guide
4. Skill validates bug report content
5. Skill creates issue using bun run gh:create
6. Skill reports: "✅ Issue #425 created: <title>"
7. User can edit on GitHub if needed
```

## Integration with Other Workflows

**This skill integrates with:**
- **Startup Sequence**: Add comment to document test baseline
- **Bug Investigation**: Create issue with root cause analysis
- **Feature Discovery**: Create issue with requirements and approach
- **Progress Tracking**: Add comments during lengthy implementations

**This skill does NOT replace:**
- `/pragmatic-code-review` - PR code review workflow
- `/address-review` - Addressing PR feedback
- Git workflow for WIP commits

---

**Remember**: You've read the workflow guide. You know the rules. Validate, then create/update autonomously. Issues are easily editable on GitHub, so don't ask for approval - just ensure quality gates pass.
