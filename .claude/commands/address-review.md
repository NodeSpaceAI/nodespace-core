---
allowed-tools: Bash(git diff:*), Bash(git log:*), Bash(git show:*), Bash(bun run:*), Bash(git add:*), Bash(git commit:*)
description: Address all reviewer recommendations from a PR code review systematically
---

## Your Role

You are acting as the **Implementation Agent** tasked with addressing code review feedback. Your mandate is to systematically work through all reviewer recommendations, implementing changes that improve code quality while using sound engineering judgment to skip unnecessary or overkill suggestions.

## Context

This command is used AFTER a PR has been reviewed (typically via `/pragmatic-code-review`). The reviewer has provided recommendations categorized by severity and type. Your job is to address these recommendations efficiently and thoughtfully.

## Pre-Implementation Steps

1. **Verify you're on the correct branch**:
   ```bash
   git status
   ```
   Ensure you're on the feature branch that was reviewed.

2. **Fetch PR review comments**:
   ```bash
   # Get PR number
   gh pr view --json number

   # Fetch all review comments and reviews
   gh pr view --json reviews,comments
   ```

3. **Locate the review recommendations**:
   - **Primary source**: PR comments from `/pragmatic-code-review`
   - **Fallback**: Recent conversation history or user-provided review report
   - Review recommendations are typically categorized as:
     - 🔴 **Critical**: MUST fix (security, bugs, broken functionality)
     - 🟡 **Important**: SHOULD fix (performance, maintainability, best practices)
     - 🟢 **Suggestion**: NICE TO HAVE (style improvements, optimizations, refactoring)

4. **Extract issue number** from the current branch name (e.g., `feature/issue-98-description` → issue #98)

## Your Task

**OBJECTIVE:** Systematically address all reviewer recommendations using sound engineering judgment.

**Implementation Priority:**

1. **MUST Address** (🔴 Critical):
   - Security vulnerabilities
   - Functional bugs
   - Breaking changes
   - Data integrity issues
   - Test failures
   - Violations of project standards (from CLAUDE.md)

2. **SHOULD Address** (🟡 Important):
   - Performance issues
   - Maintainability concerns
   - Unclear or undocumented code
   - Missing error handling
   - Inconsistent patterns
   - Incomplete test coverage for business logic

3. **EVALUATE Before Implementing** (🟢 Suggestion):
   - Style improvements that don't affect readability
   - Optimizations with marginal benefit
   - Refactoring that doesn't improve maintainability
   - Over-engineering concerns

**When to SKIP a recommendation:**

Use your engineering judgment to skip recommendations that are:
- **Overkill**: The suggested change is disproportionate to the problem
- **Premature optimization**: Optimizing before measuring actual performance issues
- **Scope creep**: Suggestion extends beyond the PR's intended purpose
- **Contradictory**: Conflicts with other recommendations or project standards
- **Unclear value**: The benefit is unclear or negligible

**IMPORTANT**: When you skip a recommendation, you MUST:
1. Document the decision in your commit message or as a comment
2. Explain your reasoning clearly
3. Note any conditions under which you might revisit the decision

## Implementation Process

For each recommendation you decide to address:

1. **Read the relevant code sections** to understand context
2. **Implement the change** following project standards
3. **Test the change** if applicable (run tests, manual verification)
4. **Document the change** in commit messages

**Commit Strategy:**

- **Option A** (Recommended): Group related changes into logical commits
  ```bash
  git add <files>
  git commit -m "Address review: <brief description of changes>

  - Fixed <issue 1>
  - Improved <issue 2>
  - Refactored <issue 3>

  Addresses reviewer recommendations from PR review."
  ```

- **Option B**: Make one commit per recommendation (for complex changes)
  ```bash
  git commit -m "Address review: Fix security vulnerability in auth handler

  Reviewer found potential SQL injection in user input processing.
  Added parameterized queries and input validation.

  Severity: 🔴 Critical"
  ```

## After Implementation

1. **Run quality checks**:
   ```bash
   bun run quality:fix
   ```
   Commit any automated fixes.

2. **Run tests** (if applicable):
   ```bash
   bun run test
   ```

3. **Document skipped recommendations**:
   Create a summary comment explaining which recommendations were skipped and why.

4. **Push changes**:
   ```bash
   git push
   ```

5. **Post summary comment to PR**:
   ```bash
   gh pr comment <pr-number> --body "<summary-of-changes-and-skipped-items>"
   ```

6. **CRITICAL: Determine if re-review is needed**:
   Analyze the changes made and provide a clear recommendation:

   **Re-review IS needed if**:
   - Critical (🔴) issues were addressed with significant code changes
   - Important (🟡) architectural or design changes were made
   - New functionality was added to address feedback
   - Complex refactoring was performed
   - You're uncertain if the implementation properly addresses the feedback
   - Multiple interconnected changes were made

   **Re-review is NOT needed if**:
   - Only minor style/formatting changes
   - Simple typo fixes or documentation updates
   - Trivial refactoring with no logic changes
   - All changes are straightforward and low-risk

   **Output the decision clearly**:
   ```
   ## Re-Review Decision

   **Decision**: [RE-REVIEW NEEDED | NO RE-REVIEW NEEDED]

   **Rationale**: [Clear explanation of why re-review is or isn't needed]

   **If re-review needed**: Run `/pragmatic-code-review` again. The reviewer will:
   - Detect existing review comments
   - Review ONLY the new commits since last review
   - Verify previous feedback was properly addressed
   - Check for any new issues introduced

   **If no re-review needed**: The PR is ready for merge (pending any other approvals).
   ```

## Output Format

As you work through recommendations, provide:

1. **Summary of planned changes**:
   - ✅ Will address: [list recommendations]
   - ⏭️ Will skip: [list recommendations with brief reasoning]

2. **Progress updates** as you implement each change

3. **Final summary**:
   - ✅ Addressed: [count] recommendations
   - ⏭️ Skipped: [count] recommendations (with justifications)
   - 📝 Commits created: [count]
   - 🧪 Tests: [passed/failed/not applicable]

## Example Workflow

```
User: "Address the review recommendations from my PR"

Agent:
1. Reviews the code review findings
2. Categorizes recommendations by severity
3. Plans implementation approach
4. Shows summary: "Will address 8/10 recommendations, skipping 2 (over-engineering)"
5. Implements changes systematically
6. Commits changes with clear messages
7. Runs quality checks and tests
8. Provides final summary with justification for skipped items
```

## Critical Constraints

- **DO NOT** blindly implement all recommendations - use engineering judgment
- **DO NOT** skip critical (🔴) recommendations without strong justification
- **DO NOT** extend scope beyond addressing review feedback
- **DO NOT** merge the PR - that's a separate decision
- **DO** explain your reasoning when skipping recommendations
- **DO** follow project standards from CLAUDE.md
- **DO** run quality checks before pushing

## Project Context

Remember to follow NodeSpace development standards:
- No lint suppression (fix issues properly)
- Use Bun for all commands (not npm/yarn)
- Follow established component patterns
- Maintain test coverage for business logic
- Use self-contained, vertical slice approach
