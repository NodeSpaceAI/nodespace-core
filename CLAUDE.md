# NodeSpace Development Agent Guide

## Project Overview

NodeSpace is an AI-native knowledge management system built with Rust backend, Svelte frontend, and Tauri desktop framework. This guide helps agents understand the project structure and find their next tasks.

## Getting Started as an Agent

> ## 🚨 MANDATORY FIRST STEPS FOR EVERY TASK 🚨
> 
> **BEFORE ANY IMPLEMENTATION WORK - COMPLETE THIS EXACT SEQUENCE:**
> 
> 1. **Check git status**: `git status` - commit any pending changes first
> 2. **Determine branching strategy**: Check parent issue for specified approach (single branch vs. individual branches)
> 3. **Create/switch to branch**: Based on strategy - either `git checkout -b feature/issue-<number>-brief-description` OR switch to existing parent issue branch
> 4. **Assign issue**: `bun run gh:assign <number> "@me"`  
> 5. **Update project status**: `bun run gh:status <number> "In Progress"`
> 6. **Select subagent**: Choose appropriate specialized agent based on task complexity and type
> 7. **Read issue requirements**: Understand all acceptance criteria
> 8. **Plan implementation**: Self-contained approach with appropriate subagent
> 
> **🔴 CRITICAL PROCESS VIOLATIONS**
> 
> **If you start implementation work without completing the startup sequence:**
> 1. STOP immediately  
> 2. Complete the startup sequence
> 3. Restart implementation with proper branch and issue assignment
> 
> **Common mistakes agents make:**
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

⚠️ **IMPORTANT: All `bun run gh:*` commands must be run from the repository root directory (`/Users/malibio/nodespace/nodespace-core/`), NOT from subdirectories like `nodespace-app/`.**

```bash
# CORRECT - from repository root
cd /Users/malibio/nodespace/nodespace-core
bun run gh:list

# WRONG - from subdirectory  
cd /Users/malibio/nodespace/nodespace-core/nodespace-app
bun run gh:list  # ❌ Will fail

# List all open issues
bun run gh:list

# View specific issue details
bun run gh:view <issue-number>

# Check issue status and acceptance criteria
bun run gh:view <issue-number>
```

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
- **Database**: LanceDB for unified storage (planned)
- **AI**: mistral.rs with local models (planned)

**Development Philosophy:**
- UI-first approach: Build interfaces before storage integration
- Early-phase mock development: Use mock services temporarily for independent feature development (transitioning to real services soon)
- Build-time plugins: Compile-time extensibility for performance
- Design system driven: Consistent UI patterns from the start

### 4. Specialized Agent Usage

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

5. **Create PR with Comprehensive Review**
   ```bash
   git add .
   git commit -m "Implement feature (addresses #<number>)"
   git push -u origin feature/issue-<number>-brief-description
   # ⚠️ MUST be run from repository root
   bun run gh:pr <number>
   ```
   Automatically updates project status to "Ready for Review"

6. **Conduct Code Review**
   - **FOLLOW UNIVERSAL PROCESS**: Use the code review guidelines in the [PR review documentation](docs/architecture/development/process/pr-review.md) 
   - Use `senior-architect-reviewer` agent for complex features
   - All quality gates and review requirements apply universally to AI agents and human reviewers

**TodoWrite Tool Users - UPDATED:**
- Your **FIRST todo item** must be: "Complete startup sequence: git status, branch strategy, create branch, assign issue (bun run gh:assign N '@me'), update status (bun run gh:status N 'In Progress'), select subagent"
- All GitHub operations now use **bun commands** (no Claude Code approval prompts)
- Do NOT break the startup sequence into separate todo items
- Only after completing the startup sequence should you add implementation todos

**Before Starting Any Task:**
1. **COMPLETE THE MANDATORY STARTUP SEQUENCE** (steps 1-8 above)
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

**Package Manager Enforcement:**
- **MANDATORY: Use Bun only** - npm, yarn, and pnpm are blocked
- Project includes automatic enforcement via preinstall hooks
- Install Bun: `curl -fsSL https://bun.sh/install | bash`
- Install packages: `bun install` (not npm install)
- Run commands: `bun run dev`, `bun run build`, etc.

**Git Workflow:**
- Create feature branches: `feature/issue-<number>-brief-description`
- Link commits to issues: `git commit -m "Add TextNode component (closes #4)"`
- Include Claude Code attribution in commit messages

**Documentation:**
- Update relevant docs when changing architecture
- Include code examples in component documentation
- Maintain consistent markdown formatting

### 7. Mandatory Process Checklist

**EVERY AGENT MUST COMPLETE THIS CHECKLIST FOR EACH TASK:**

**Startup Sequence (MANDATORY - Steps 1-8 from above):**
- [ ] Checked git status and committed any pending changes
- [ ] Determined branching strategy from parent issue (single branch vs. individual branches)
- [ ] Created/switched to appropriate branch based on strategy
- [ ] Assigned issue to self (`bun run gh:assign <number> "@me"`)
- [ ] Updated GitHub project status using CLI: Todo → In Progress
- [ ] Selected appropriate subagent based on task complexity
- [ ] Read issue requirements and acceptance criteria
- [ ] Read development process documentation (start with [overview](docs/architecture/development/overview.md))
- [ ] Planned self-contained implementation with mock dependencies

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

**PR and Review:**
- [ ] Created PR with proper title and description
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

## Repository Structure

```
nodespace-core/
├── docs/architecture/          # Complete technical specifications
├── README.md                   # Project overview and quick start
├── CLAUDE.md                   # This file - agent guidance
├── .gitignore                  # Excludes build artifacts, models, databases
└── [implementation files will be created by agents]
```

## Current Project Status

- ✅ Architecture documentation complete
- ✅ GitHub project management setup
- ✅ Technology versions updated to current releases
- ⏳ Foundation implementation (Issue #1) - Ready for agent pickup
- ⏳ Design system, desktop shell, and core components - Planned

---

**Note**: This project uses UI-first development approach. Build user interfaces with mock data first, then integrate backend storage and AI functionality. Focus on creating excellent user experiences before tackling complex technical integrations.