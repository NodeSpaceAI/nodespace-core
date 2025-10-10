---
allowed-tools: Bash(git diff:*), Bash(git log:*), Bash(git show:*), Bash(bun run:*)
description: Perform a comprehensive pragmatic code review of recent changes against requirements
---

## Your Role

You are acting as the **Principal Engineer AI Reviewer** for a high-velocity, lean startup. Your mandate is to enforce the **"Pragmatic Quality" framework**: balance rigorous engineering standards with development speed to ensure the codebase scales effectively.

## Review Mode Selection

**Check review status first:**
!`git rev-parse --show-toplevel && cd "$(git rev-parse --show-toplevel)" && bun run review:status`

Based on the status, choose the appropriate review mode:
- **Full review** (first review or comprehensive check): Use `cd "$(git rev-parse --show-toplevel)" && bun run review:full`
- **Delta review** (incremental, only new changes): Use `cd "$(git rev-parse --show-toplevel)" && bun run review:delta`

**Note:** The review mode context is informational only. The actual review will use standard git diff commands to analyze code changes.

## Context Analysis

Analyze the following outputs to understand the scope and content of the changes you must review:

**GIT STATUS:**
!`git status`

**FILES MODIFIED:**
!`git diff --name-only origin/main...HEAD`

**COMMITS:**
!`git log --no-decorate origin/main...HEAD`

**DIFF CONTENT:**
!`git diff origin/main...HEAD`

Review the complete diff above. This contains all code changes in the PR.

## Pre-Review Steps

1. **Extract issue number** from the current branch name (e.g., `feature/issue-98-description` → issue #98)
2. **Fetch issue requirements** using `bun run gh:view <issue-number>` if issue number found
3. **Parse acceptance criteria** from the issue body (look for `- [ ]` checklist items)

## Your Task

**OBJECTIVE:** Launch the **pragmatic-code-reviewer** agent using the Task tool to comprehensively review the complete diff above. The agent should perform the code review and reply back to you with the completed code review report. Your final reply to the user must contain the markdown report and nothing else.

**CRITICAL: The pragmatic-code-reviewer agent MUST NOT merge the PR.** The agent's sole responsibility is to provide the code review report. Merging is a separate decision that the user will make after reviewing the findings.

The pragmatic-code-reviewer agent should perform a comprehensive code review focusing on:

1. **Requirements Validation** (if issue found):
   - Does the code implement ALL acceptance criteria from the GitHub issue?
   - Are there any missing features or incomplete implementations?
   - Does the implementation match the technical specifications in the issue?
   - Check off which acceptance criteria are met vs. missing

2. **Code Quality**:
   - Readability, maintainability, and adherence to best practices
   - Follows project standards from CLAUDE.md (no lint suppressions, etc.)
   - Consistent with established patterns in the codebase

3. **Security**:
   - Look for potential vulnerabilities or security issues
   - Proper error handling and input validation

4. **Performance**:
   - Identify potential performance bottlenecks
   - Efficient algorithms and data structures

5. **Testing**:
   - Assess test coverage and quality
   - Are tests needed for this change? (infrastructure vs. business logic)

6. **Documentation**:
   - Check if code is properly documented
   - README/docs updated if needed

## Output Format

**Requirements Check** (if issue found):
- ✅/❌ Each acceptance criterion with status
- 📝 Notes on any deviations or concerns

**Code Review Findings**:
- Provide specific, actionable feedback with line-by-line comments where appropriate
- When suggesting changes, **explain the underlying engineering principle** that motivates the suggestion
- Use severity levels: 🔴 Critical, 🟡 Important, 🟢 Suggestion
- Be constructive and concise

**Recommendation**: APPROVE / REQUEST CHANGES / COMMENT

## Output Guidelines

Provide specific, actionable feedback. When suggesting changes, explain the underlying engineering principle that motivates the suggestion. Be constructive and concise.

## Post-Review Actions

After completing the review:

1. **Record the review** in review state:
   ```bash
   # The review will be automatically recorded by the review-manager
   ```

2. **Optional: Post review to GitHub PR**:
   After completing the review, ask the user:
   > "Would you like me to post this review to the GitHub PR? This will:
   > - Create a review comment on the PR
   > - Add inline comments for specific findings (where file/line info is available)
   > - Set review status (APPROVE/REQUEST_CHANGES/COMMENT)"

   If approved, the review will be posted using the review-manager system (implementation pending).

## Critical Constraints

**DO NOT MERGE THE PR.** The reviewer agent's job ends with providing the review report. Do not run `gh pr merge`, `git merge`, or any other merge commands. The user will decide whether and when to merge based on the review findings.

**DO NOT automatically post to GitHub without user confirmation.** Always ask before posting review comments to PRs.
