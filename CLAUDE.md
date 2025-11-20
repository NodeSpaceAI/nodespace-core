# NodeSpace Development Agent Guide

## 🚨 CRITICAL: Pre-Release Development - NO BACKWARD COMPATIBILITY 🚨

**NodeSpace has ZERO users, NO production deployment, and NO releases.**

### Absolute Rules for All Agents

- ❌ **NO backward compatibility code** - Delete old patterns immediately when replaced
- ❌ **NO migration strategies** - We can reset the database anytime
- ❌ **NO gradual rollouts** - Implement new architecture directly, delete old code
- ❌ **NO transition periods** - No dual-mode support, no feature flags for compatibility
- ❌ **NO version support** - Don't maintain multiple versions of any API/method
- ❌ **NO "soak periods"** - No waiting weeks between changes
- ❌ **NO phased migrations** - Unless coordinating across multiple active worktrees

### What This Means for Implementation

**When fixing bugs or implementing features:**
- ✅ Make breaking changes without hesitation - we have no users to impact
- ✅ If you break something, fix it immediately in the same work session
- ✅ Delete deprecated code immediately - no "TODO: remove after migration"
- ✅ Update tests to match new behavior - don't test old patterns
- ✅ Implement final architecture directly - skip intermediate steps
- ✅ Reset database if needed - no data preservation required
- ✅ Own your changes end-to-end - don't leave broken code for others

**If you find yourself writing:**
- "for backward compatibility..."
- "during the transition period..."
- "to support both old and new..."
- "gradual rollout strategy..."
- "soak period before removing..."

**STOP. You're overthinking it. This is greenfield development.**

## Project Overview

NodeSpace is an AI-native knowledge management system built with Rust backend, Svelte frontend, and Tauri desktop framework. This guide helps agents understand the project structure and find their next tasks.

## Getting Started as an Agent

> ## 🚨 MANDATORY FIRST STEPS FOR EVERY TASK 🚨
>
> **BEFORE ANY IMPLEMENTATION WORK - COMPLETE THIS EXACT SEQUENCE:**
>
> **⚠️ EXCEPTION: If continuing from a WIP commit, skip to "Continuing from WIP" section below**
>
> 1. **Check git status**: `git status` - commit any pending changes first
> 2. **Pull latest changes**: `git fetch origin && git pull origin main` - ensure you're working from the latest codebase
> 3. **Run test baseline**: `bun run test` - record passing/failing counts and specific failing test names
> 4. **Document baseline**: Add comment to issue with baseline test status
> 5. **Determine branching strategy**: Check parent issue for specified approach (single branch vs. individual branches)
> 6. **Create/switch to branch**: Based on strategy - either `git checkout -b feature/issue-<number>-brief-description` OR switch to existing parent issue branch
> 7. **Assign issue**: `bun run gh:assign <number> "@me"`
> 8. **Update project status**: `bun run gh:status <number> "In Progress"`
> 9. **Select subagent**: Choose appropriate specialized agent based on task complexity and type
> 10. **Read issue requirements**: Understand all acceptance criteria
> 11. **Plan implementation**: Self-contained approach with appropriate subagent
>
> ## 📋 CONTINUING FROM WIP COMMIT
>
> **If you're picking up work from a previous WIP commit, use this simplified sequence:**
>
> 1. **Check git status**: `git status` - verify you're on the correct branch
> 2. **Pull latest changes**: `git fetch origin && git pull origin <branch-name>` - get latest WIP commits
> 3. **Review WIP commit message**: Read the handoff commit to understand current state and next steps
> 4. **Check issue comment**: Look for the baseline test status documented when work started
> 5. **Resume implementation**: Continue from "Remaining Work" section in WIP commit message
>
> **DO NOT:**
> - ❌ Re-run baseline tests (already done when work started)
> - ❌ Re-assign the issue (already assigned)
> - ❌ Re-update status to "In Progress" (already set)
> - ❌ Create a new branch (already exists)
>
> **Focus on:**
> - ✅ Understanding what was completed (from WIP commit)
> - ✅ Understanding what remains (from "Remaining Work" section)
> - ✅ Continuing the implementation approach
> - ✅ Maintaining consistency with established patterns
> 
> **🔴 CRITICAL PROCESS VIOLATIONS**
> 
> **If you start implementation work without completing the startup sequence:**
> 1. STOP immediately  
> 2. Complete the startup sequence
> 3. Restart implementation with proper branch and issue assignment
> 
> **Common mistakes agents make:**
> - **Skipping `git pull`** - Starting work without pulling latest changes leads to merge conflicts
> - **Skipping test baseline** - Not recording initial test status leads to regressions
> - Reading files before creating feature branch
> - Planning implementation before assigning issue
> - Using TodoWrite without including startup sequence as first item
> - Skipping git checkout step entirely

> 🚨 **ADDITIONAL CRITICAL REQUIREMENTS** 🚨
> 
> **BEFORE STARTING ANY TASK, YOU MUST ALSO READ:**
> - [`/docs/architecture/development/overview.md`](docs/architecture/development/overview.md) - Complete development process overview
> - [`/docs/architecture/development/process/startup-sequence.md`](docs/architecture/development/process/startup-sequence.md) - Mandatory pre-implementation steps
> 
> **KEY PRINCIPLES YOU MUST FOLLOW:**
> - ✅ **Self-Contained Implementation**: Each issue must work independently with full functionality
> - ✅ **Early-Phase Mock Development**: Use mock data/services temporarily for parallel development (transitioning to real services soon)
> - ✅ **Vertical Slicing**: Complete features end-to-end, not horizontal layers
> - ✅ **GitHub Status Updates**: Use CLI commands to update project status at each transition (Todo → In Progress → Ready for Review)
> - ✅ **Use Appropriate Subagents**: Use specialized agents when task complexity warrants expert assistance

### 1. Understanding the Project
- **Read the README.md** for high-level project overview and architecture
- **Review `/docs/architecture/`** for detailed technical specifications:
  - `core/system-overview.md` - Complete architecture and design decisions
  - `core/technology-stack.md` - Current tech stack and versions
  - `components/` - Detailed component specifications
  - `deployment/development-setup.md` - Setup requirements

### 2. Finding Tasks to Work On

**Primary Task Source: GitHub Issues**

⚠️ **IMPORTANT: All `bun run gh:*` commands must be run from the repository root directory (`/Users/malibio/nodespace/nodespace-core/`), NOT from subdirectories like `packages/desktop-app/`.**

```bash
# CORRECT - from repository root
cd /Users/malibio/nodespace/nodespace-core
bun run gh:list

# WRONG - from subdirectory  
cd /Users/malibio/nodespace/nodespace-core/packages/desktop-app
bun run gh:list  # ❌ Will fail

# List all open issues
bun run gh:list

# View specific issue details
bun run gh:view <issue-number>

# Edit issue properties
bun run gh:edit <issue-number> --title "New Title"
bun run gh:edit <issue-number> --body "Updated description"
bun run gh:edit <issue-number> --labels "foundation,ui"
bun run gh:edit <issue-number> --state "closed"

# Check issue status and acceptance criteria
bun run gh:view <issue-number>
```

**When creating or modifying issues:**
- **MUST follow**: [Issue Workflow Guide](docs/architecture/development/process/issue-workflow.md)
- Contains templates, formatting rules, and quality gates

**Issue Priority Guidelines:**
- Issues labeled `foundation` - Core infrastructure (highest priority)
- Issues labeled `design-system` - UI foundation components
- Issues labeled `ui` - User interface implementations
- Issues labeled `backend` - Rust backend functionality

### 3. Project Context and State

**Current Architecture:**
- **Backend**: Rust with async/await, trait-based architecture
- **Frontend**: Svelte 4.x with reactive state management
- **Desktop**: Tauri 2.0 for native integration
- **Database**: Turso embedded for unified storage and sync
- **AI**: mistral.rs with local models (planned)

**Development Philosophy:**
- UI-first approach: Build interfaces before storage integration
- Early-phase mock development: Use mock services temporarily for independent feature development (transitioning to real services soon)
- Build-time plugins: Compile-time extensibility for performance
- Design system driven: Consistent UI patterns from the start

### 4. Node Type System & Schema Architecture (CRITICAL)

**🚨 MANDATORY READING BEFORE IMPLEMENTING NODE TYPES OR PROPERTIES:**

NodeSpace uses a **hybrid architecture** combining hardcoded behaviors with schema-driven extensions. Understanding this is critical to avoid breaking changes and maintenance hell.

#### Core Architecture Documents (READ THESE FIRST)

**1. Node Behavior System**
- **Location**: [`/docs/architecture/business-logic/node-behavior-system.md`](docs/architecture/business-logic/node-behavior-system.md)
- **When to read**: Before modifying/creating ANY node type (task, text, date, etc.)
- **Key concepts**:
  - Hybrid approach: Core (hardcoded) vs Extension (schema-driven)
  - When to use behaviors vs schemas
  - Property ownership model
  - Validation hierarchy

**2. Schema Management**
- **Location**: [`/docs/architecture/development/schema-management-implementation-guide.md`](docs/architecture/development/schema-management-implementation-guide.md)
- **When to read**: Before adding properties to nodes or creating custom types
- **Key concepts**:
  - **Namespace enforcement** (CRITICAL for preventing conflicts)
  - User properties MUST use prefixes (`custom:`, `org:`, `plugin:`)
  - Core properties use simple names (reserved for future)
  - Protection levels and lazy migration

#### Quick Decision Tree

**Adding a property to a core node type (task, text, date, etc.):**
```
Is it a CORE property the UI depends on?
  ✅ YES → Edit hardcoded behavior in packages/core/src/behaviors/mod.rs
  ❌ NO → Use schema system with NAMESPACE PREFIX (custom:propertyName)
```

**Creating a new node type:**
```
Is it a built-in core type everyone needs?
  ✅ YES → Create hardcoded behavior + schema (requires issue approval)
  ❌ NO → Create schema-only type (no behavior needed)
```

#### Critical Rules

**DO:**
- ✅ Read node-behavior-system.md before touching node types
- ✅ Use namespace prefixes for user properties (`custom:`, `org:`, `plugin:`)
- ✅ Follow the hybrid architecture pattern
- ✅ Check issue #400 for namespace enforcement status

**DON'T:**
- ❌ Add user properties without namespace prefix (will conflict with future core properties)
- ❌ Delete core properties from schemas (breaks UI)
- ❌ Create hardcoded behaviors for plugin/custom types
- ❌ Skip reading the architecture docs (leads to breaking changes)

### 5. Component Architecture (CRITICAL)

**Established Naming Conventions** (Follow these patterns exactly):

```
*Node = Individual node components that wrap BaseNode
*NodeViewer = Page-level viewers that wrap BaseNodeViewer
```

**✅ Correct Component Hierarchy:**
- **BaseNode** (`src/lib/design/components/base-node.svelte`) - Abstract core (NEVER use directly)
- **BaseNodeViewer** (`src/lib/design/components/base-node-viewer.svelte`) - Node collection manager
- **TextNode** (`src/lib/components/viewers/text-node.svelte`) - Text node wrapper
- **TaskNode** (`src/lib/design/components/task-node.svelte`) - Task node wrapper
- **DateNode** (`src/lib/components/viewers/date-node.svelte`) - Date node wrapper
- **DateNodeViewer** (`src/lib/components/viewers/date-node-viewer.svelte`) - Date page viewer

**❌ DO NOT Create These:**
- `TextNodeViewer` - BaseNodeViewer is sufficient for text
- `DatePageViewer` - Should be `DateNodeViewer`
- Any direct usage of `BaseNode` in application code

**📖 Complete Documentation:**
- [`/docs/architecture/components/component-architecture-guide.md`](docs/architecture/components/component-architecture-guide.md) - Complete patterns and templates
- [`/docs/architecture/frontend-architecture.md`](docs/architecture/frontend-architecture.md) - Frontend overview

**When Building New Components:**
1. **Read the architecture guide first** - Contains templates and patterns
2. **Determine component type**: Node wrapper or Viewer wrapper?
3. **Follow naming convention**: `*Node` or `*NodeViewer`
4. **Use provided templates** from the architecture guide
5. **Register in plugin system** with correct lazy loading paths

### 5. Specialized Agent Usage

Use the most appropriate specialized sub-agent available for complex tasks. Claude Code will automatically select the best agent based on task context and complexity.

**CRITICAL: Sub-Agent Commissioning Instructions**

When commissioning a specialized sub-agent, you MUST include these specific instructions in your prompt:

```
IMPORTANT SUB-AGENT INSTRUCTIONS:
- DO NOT repeat the startup sequence (git status, branch creation, issue assignment, etc.) - the main agent has already completed this
- You are working on an EXISTING feature branch with the issue already assigned and in progress
- Focus ONLY on the specific technical implementation task assigned to you
- DO NOT commit changes or create pull requests - the main agent will handle all git operations and PR creation
- DO NOT run project management commands (bun run gh:status, bun run gh:pr, etc.) - main agent manages project status
- Follow all project standards (no lint suppression, use Bun only, etc.) but skip the administrative steps
- Continue with the existing implementation approach and maintain consistency with established patterns
- Return control to main agent when your technical work is complete
```

**Why This Matters:**
- Prevents redundant administrative work that wastes time
- Ensures sub-agents focus on their specialized expertise (not project management)
- Maintains single point of control for git operations and project status
- Avoids conflicts with already-completed setup and branch state
- Allows for seamless handoff between main agent and specialist
- Main agent maintains full context of implementation progress and can handle commits/PRs appropriately

### 5. Implementation Workflow

**CRITICAL**: Follow the complete development process in the [development documentation](docs/architecture/development/overview.md)

**Step-by-Step Process (Summary - See Full Process Documentation):**

1. **Pick an Issue & Assign Yourself**
   ```bash
   # ⚠️ MUST be run from repository root: /Users/malibio/nodespace/nodespace-core/
   bun run gh:list
   bun run gh:view <number>
   bun run gh:assign <number> "@me"
   bun run gh:status <number> "In Progress"
   ```
   All commands now use TypeScript API (no Claude Code approval prompts)

2. **Create Feature Branch**
   ```bash
   git checkout -b feature/issue-<number>-brief-description
   ```

3. **Implement with Self-Contained Approach**
   - **Use mock data/services temporarily** for independent development (see development process for patterns)
   - Build complete, working features that don't depend on other incomplete work
   - Follow vertical slicing: complete the feature end-to-end with mocks for now
   - Implement all acceptance criteria with demonstrable functionality

4. **Complete Acceptance Criteria**
   - Check off each `- [ ]` item in the issue as you complete it
   - Test thoroughly with mock data and services (temporarily, transitioning to real services soon)
   - Ensure feature works independently and provides user value

**4a. Testing (Required)**
   ```bash
   # Fast unit tests with Happy-DOM (use during development)
   bun run test                    # Run all unit tests once (FAST MODE - optimized)
   bun run test:unit               # Same as above (explicit)
   bun run test:watch              # Watch mode (recommended for TDD)
   bun run test:perf               # Full performance validation (large datasets)

   # Test specific files
   bun run test src/tests/integration/my-test.test.ts
   bun run test:watch src/tests/unit/my-component.test.ts

   # Browser tests with real DOM (Chromium via Playwright)
   bun run test:browser            # Run browser tests (for focus, events, etc.)
   bun run test:browser:watch      # Watch mode for browser tests

   # Run all tests (unit + browser + rust)
   bun run test:all                # Runs both unit and browser tests + Rust tests

   # Database integration tests (use before merging)
   bun run test:db                 # Full integration with SQLite
   bun run test:db:watch           # Watch mode with database

   # Coverage reports
   bun run test:coverage
   ```

   **Hybrid Testing Strategy:**
   NodeSpace uses a **two-tier testing approach** for optimal speed and reliability:

   1. **Happy-DOM (Fast Unit Tests)** - 728+ tests, ~10-20 seconds
      - Controller logic, services, utilities
      - Pattern matching, content processing
      - State management, data transformations
      - Use: `bun run test` or `bun run test:unit`
      - Location: `src/tests/**/*.test.ts` (excluding `browser/`)

   2. **Vitest Browser Mode (Real Browser Integration Tests)** - Targeted critical tests
      - Focus management (focus/blur events)
      - Edit mode activation and transitions
      - Dropdown interactions (slash commands, @mentions)
      - Cross-node navigation with real browser behavior
      - Use: `bun run test:browser`
      - Location: `src/tests/browser/**/*.test.ts`
      - **Note**: Requires Playwright browsers installed (`bunx playwright install chromium`)

   3. **Performance Tests** - Two modes for different workflows
      - **Fast Mode (default in `bun run test`)**: Reduced datasets (100-500 nodes) for quick feedback
      - **Full Mode (`bun run test:perf`)**: Large datasets (1000-2000 nodes) for comprehensive validation
      - Use: `bun run test:perf` when optimizing performance or before major releases
      - Location: `src/tests/performance/**/*.test.ts`
      - **Automatic Scaling**: Tests use `TEST_FULL_PERFORMANCE=1` to switch between modes

   **When to use which mode:**
   - **Happy-DOM (default)**: 99% of tests - logic, services, utilities (fast, TDD-friendly)
   - **Browser Mode**: Only when you need real focus/blur events or browser-specific DOM APIs
   - **Performance Tests**: Run fast mode daily, full mode before merging performance-critical changes
   - **Database Mode**: Full integration validation before merging critical changes
   - Some tests conditionally skip in in-memory mode (require full database persistence)
   - See [Testing Guide](docs/architecture/development/testing-guide.md) for details

5. **Run Tests & Quality Checks Before PR**
   ```bash
   # ⚠️ MANDATORY STEP 1: Verify no new test failures
   bun run test:all
   # Compare results to baseline from step 3
   # If any NEW failures: STOP and fix them before PR
   # Document any pre-existing failures in PR description

   # ⚠️ MANDATORY STEP 2: Run quality:fix
   bun run quality:fix

   # If quality:fix made changes, commit them
   git add .
   git commit -m "Fix linting and formatting"

   # Create PR (MUST be run from repository root)
   git push -u origin feature/issue-<number>-brief-description
   bun run gh:pr <number>
   ```
   **CRITICAL**:
   - Run `bun run test:all` FIRST - no new test failures allowed
   - Run `bun run quality:fix` SECOND - no lint/format issues allowed
   - Automatically updates project status to "Ready for Review"

6. **Conduct Code Review**
   - **FOLLOW UNIVERSAL PROCESS**: Use the code review guidelines in the [PR review documentation](docs/architecture/development/process/pr-review.md) 
   - Use `senior-architect-reviewer` agent for complex features
   - All quality gates and review requirements apply universally to AI agents and human reviewers

**TodoWrite Tool Users - UPDATED:**

**For NEW tasks (starting fresh):**
- Your **FIRST todo item** must be: "Complete startup sequence: git status, pull latest, run test baseline (bun run test), document baseline in issue, branch strategy, create branch, assign issue (bun run gh:assign N '@me'), update status (bun run gh:status N 'In Progress'), select subagent"
- Your **LAST todo items** must include: "Run test:all to verify no new failures", "Run quality:fix and commit changes", and "Create PR"
- Do NOT break the startup sequence into separate todo items
- Only after completing the startup sequence should you add implementation todos

**For CONTINUING from WIP commit:**
- Your **FIRST todo item** must be: "WIP continuation sequence: git status, pull latest from branch, review WIP commit message, check issue for baseline, resume from 'Remaining Work'"
- Do NOT re-run baseline tests or re-assign the issue
- Focus todos on remaining work from the WIP commit message
- Your **LAST todo items** still include: "Run test:all to verify no new failures", "Run quality:fix and commit changes", and "Create PR"

**General:**
- All GitHub operations now use **bun commands** (no Claude Code approval prompts)

**Before Starting Any Task:**
1. **COMPLETE THE MANDATORY STARTUP SEQUENCE** (steps 1-11 above, including test baseline)
2. **READ THE DEVELOPMENT PROCESS DOCUMENTATION** - Start with the [overview](docs/architecture/development/overview.md) and [startup sequence](docs/architecture/development/process/startup-sequence.md)
3. **Select appropriate subagent** based on task complexity and type
4. Check issue acceptance criteria and requirements
5. Plan self-contained implementation with mock dependencies

### 6. Development Standards

**Code Quality:**
- Follow Rust formatting standards (rustfmt)
- Use TypeScript for frontend type safety
- Implement comprehensive error handling with anyhow/thiserror
- Write tests with mock services temporarily for independent development (transitioning to real services soon)

**Linting Policy:**
- **NO lint suppression allowed** - Fix issues properly, don't suppress warnings
- **NO EXCEPTIONS** - All lint warnings and errors must be fixed with proper solutions
- Use proper TypeScript types instead of `any`
- Follow Svelte best practices and avoid unsafe patterns like `{@html}`

**Runtime and Package Manager:**
- **MANDATORY: Bun-only development** - Node.js not required
- All scripts use `bunx` for consistent Bun runtime execution
- Project includes automatic npm/yarn/pnpm blocking via preinstall hooks
- Install Bun: `curl -fsSL https://bun.sh/install | bash`
- Install packages: `bun install`
- Run commands: `bun run dev`, `bun run test`, `bun run build`, etc.
- Testing: Vitest + Happy DOM (faster than jsdom, Bun-optimized)

**Testing Requirements (CRITICAL):**
- **NEVER use `bun test`** - This command does NOT support Happy-DOM environment
- **ALWAYS use one of these:**
  - **In-Memory Mode (Fast - Recommended)**:
    - `bun run test` - Run all tests once
    - `bun run test:watch` - Watch mode for TDD
    - `bunx vitest` - Direct watch mode
  - **Database Mode (Full Integration)**:
    - `bun run test:db` - Full SQLite integration tests
    - `bun run test:db:watch` - Watch mode with database
  - **Coverage**:
    - `bun run test:coverage` - Generate coverage reports
- **Why?** Vitest is configured with Happy-DOM in vitest.config.ts. Bun's native test runner doesn't read this configuration, causing DOM-dependent tests to fail.
- **Test Modes:** Integration tests support two modes via `TEST_USE_DATABASE` flag:
  - **In-memory (default)**: 100x faster, perfect for TDD and CI/CD
  - **Database mode**: Full integration validation with SQLite persistence
- **Validation:** Tests will automatically fail with a clear error message if run with wrong command
- **CI/CD:** All test scripts in package.json use the correct commands

**Git Workflow:**
- Create feature branches: `feature/issue-<number>-brief-description`
- Link commits to issues: `git commit -m "Add TextNode component (closes #4)"`
- Include Claude Code attribution in commit messages

**Mid-Implementation Commits & Session Handoffs:**

When an issue is lengthy or implementation has gone longer than expected, commit work-in-progress to enable fresh session pickup. These handoff commits require **complete context for the next AI agent session**.

**When to Create Handoff Commits:**
- Implementation spans multiple logical phases/milestones
- Session approaching context limits or complexity threshold
- Natural breakpoint in work (completed subsystem, before major refactor)
- Need to preserve progress before tackling risky changes
- Work-in-progress needs to be saved for continuation later

**Handoff Commit Message Format:**

```
WIP: [Brief description of what was accomplished]

## Completed in This Session
- [x] Phase 1: [Specific accomplishment with details]
- [x] Phase 2: [Specific accomplishment with details]
- [x] [Any other completed items]

## Remaining Work
- [ ] Phase 3: [What needs to be done next]
- [ ] Phase 4: [Subsequent task]
- [ ] [Final tasks to complete the issue]

## Current State
- Files modified: [List key files changed]
- Tests status: [Passing/Failing/Not yet written]
- Known issues: [Any blockers or concerns]
- Dependencies: [What this work depends on or what depends on this]

## Context for Next Session
[2-3 sentences explaining the overall approach, any important decisions
made, and what the next agent should focus on]

## Acceptance Criteria Status
From issue #[number]:
- [x] [Completed criterion]
- [ ] [Remaining criterion]
- [ ] [Remaining criterion]

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
```

**Example Handoff Commit:**

```
WIP: Implement schema-driven property UI - Phases 1-2 complete

## Completed in This Session
- [x] Phase 1: Created SchemaPropertyForm component with type detection
- [x] Phase 2: Implemented text, number, and boolean property renderers
- [x] Added form validation with error display
- [x] Integrated with existing node type system

## Remaining Work
- [ ] Phase 3: Implement date/select/multi-value property types
- [ ] Phase 4: Add property reordering and deletion
- [ ] Phase 5: Write integration tests
- [ ] Phase 6: Update documentation

## Current State
- Files modified:
  - src/lib/components/property-forms/schema-property-form.svelte (new)
  - src/lib/services/schema-service.ts (extended)
  - src/lib/types/schema.ts (added PropertyRenderer type)
- Tests status: Unit tests passing, integration tests not yet written
- Known issues: None - all current functionality working
- Dependencies: Requires SchemaService, works with BaseNode

## Context for Next Session
The foundation is solid - basic property types render correctly with
validation. Focus next on complex types (date pickers, dropdowns) and
then the editing capabilities (reorder, delete). The component is already
integrated into the node system, so new property types just need renderers.

## Acceptance Criteria Status
From issue #193:
- [x] Schema properties display in node cards
- [x] Basic property types supported (text, number, boolean)
- [ ] All property types supported (date, select, multi-value)
- [ ] Properties can be reordered
- [ ] Properties can be deleted
- [ ] Comprehensive tests written

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: Claude <noreply@anthropic.com>
```

**Critical Guidelines:**
- **Do NOT use "WIP" commits for normal development** - only for intentional session handoffs
- **Push WIP commits immediately** so next session can pull them
- **Update issue comments** with handoff summary and link to commit
- **Be specific** - next agent shouldn't have to reverse-engineer your work
- **Include acceptance criteria status** - clearly show progress against original issue
- **Explain architectural decisions** made during implementation
- **Note any deviations** from original plan and why

**Documentation:**
- Update relevant docs when changing architecture
- Include code examples in component documentation
- Maintain consistent markdown formatting

### 7. Mandatory Process Checklist

**EVERY AGENT MUST COMPLETE THIS CHECKLIST FOR EACH TASK:**

**Startup Sequence - NEW TASK (MANDATORY - Steps 1-11 from above):**
- [ ] Checked git status and committed any pending changes
- [ ] **Pulled latest changes** (`git fetch origin && git pull origin main`)
- [ ] **Recorded test baseline** (`bun run test` - noted passing/failing counts)
- [ ] **Documented baseline in issue** (commented with test status and specific failures)
- [ ] Determined branching strategy from parent issue (single branch vs. individual branches)
- [ ] Created/switched to appropriate branch based on strategy
- [ ] Assigned issue to self (`bun run gh:assign <number> "@me"`)
- [ ] Updated GitHub project status using CLI: Todo → In Progress
- [ ] Selected appropriate subagent based on task complexity
- [ ] Read issue requirements and acceptance criteria
- [ ] Read development process documentation (start with [overview](docs/architecture/development/overview.md))
- [ ] Planned self-contained implementation with mock dependencies

**Startup Sequence - CONTINUING FROM WIP (Simplified):**
- [ ] Checked git status - verified on correct branch
- [ ] **Pulled latest from branch** (`git fetch origin && git pull origin <branch-name>`)
- [ ] **Read WIP commit message** - understand completed work and remaining tasks
- [ ] **Check issue for baseline** - reference the test baseline documented when work started
- [ ] Resume implementation from "Remaining Work" section
- [ ] **DO NOT re-run baseline, re-assign issue, or re-update status**

**During Implementation:**
- [ ] Following self-contained approach (feature works independently)
- [ ] Using mock data/services for dependencies
- [ ] Implementing vertical slice (complete feature end-to-end)
- [ ] All acceptance criteria being addressed

**Before Submitting:**
- [ ] Feature works independently and provides demonstrable value
- [ ] All acceptance criteria completed and checked off
- [ ] Comprehensive testing with mock services
- [ ] Code follows project standards
- [ ] **Run `bun run test:all` - verify no new test failures vs baseline**
- [ ] **Run `bun run quality:fix` and commit any changes**

**PR and Review:**
- [ ] **Verify test suite**: Run `bun run test:all` - no new failures allowed
- [ ] **Verify code quality**: Run `bun run quality:fix` one final time
- [ ] Commit any linting/formatting fixes
- [ ] Created PR with proper title and description
- [ ] **Document test status in PR**: Note baseline vs current test results
- [ ] Updated GitHub project status using CLI: In Progress → Ready for Review
- [ ] Used appropriate subagent for code review if needed
- [ ] Merged immediately if review passes, or addressed feedback

**Failure to follow this checklist blocks the development process and violates project standards.**

### 8. Getting Help

**Resources Available:**
- `/docs/architecture/` - Complete technical specifications
- `README.md` - Quick start and overview
- GitHub issues - Detailed implementation requirements
- Existing NodeSpace repositories in `/Users/malibio/nodespace/` for reference patterns

**When Stuck:**
- Check related issues for context and dependencies
- Review architecture docs for design decisions
- Look at existing NodeSpace codebases for established patterns
- Verify technology versions match current documentation

## Updated Repository Structure (Post-2024 Restructure)

```
nodespace-core/
├── docs/                          # UNIFIED documentation
│   ├── architecture/             # System architecture
│   │   ├── development/          # Development process & lessons learned
│   │   │   └── lessons/          # Analysis files preserved as permanent reference
│   │   ├── components/           # Component specifications
│   │   ├── decisions/            # Architecture decision records
│   │   └── data/                 # Data architecture and storage docs
│   ├── troubleshooting/          # Issue resolution guides
│   ├── performance/              # Performance analysis
│   └── design-system/            # UI component documentation
├── packages/
│   └── desktop-app/              # Main Svelte + Tauri application
│       ├── src/                  # Frontend source
│       ├── src-tauri/            # Tauri backend
│       └── [configs]             # App-specific configurations
├── scripts/                      # Build and GitHub utilities
├── package.json                  # Bun workspace root
└── Cargo.toml                    # Rust workspace (if needed)
```

### Key Improvements from Restructure:
- **Single Documentation Source**: All docs consolidated under `docs/`
- **Clear Package Structure**: Desktop app isolated in `packages/desktop-app/`
- **Preserved History**: All file moves maintain git history
- **Updated References**: All internal links and configurations updated
- **Lessons Preserved**: Analysis files moved to permanent `docs/architecture/development/lessons/`

## Current Project Status

- ✅ Architecture documentation complete
- ✅ GitHub project management setup
- ✅ Technology versions updated to current releases
- ⏳ Foundation implementation (Issue #1) - Ready for agent pickup
- ⏳ Design system, desktop shell, and core components - Planned

---

**Note**: This project uses UI-first development approach. Build user interfaces with mock data first, then integrate backend storage and AI functionality. Focus on creating excellent user experiences before tackling complex technical integrations.