---
allowed-tools: Bash(git diff:*), Bash(git log:*), Bash(git show:*), Bash(bun run:*)
description: Perform a comprehensive code review of recent changes against requirements
---

## Context

- Current git status: !`git status`
- Recent changes: !`git diff HEAD~1`
- Recent commits: !`git log --oneline -5`
- Current branch: !`git branch --show-current`
- Branch issue number: Extract from branch name (e.g., `feature/issue-98-description` → issue #98)

## Pre-Review Steps

1. **Extract issue number** from the current branch name
2. **Fetch issue requirements** using `bun run gh:view <issue-number>` if issue number found
3. **Parse acceptance criteria** from the issue body (look for `- [ ]` checklist items)

## Your task

Perform a comprehensive code review focusing on:

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
- Use severity levels: 🔴 Critical, 🟡 Important, 🟢 Suggestion

**Recommendation**: APPROVE / REQUEST CHANGES / COMMENT