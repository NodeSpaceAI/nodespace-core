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
> 4. **Assign issue**: `gh issue edit <number> --add-assignee "@me"`
> 5. **Update project status**: Todo → In Progress (GitHub web interface)
> 6. **Select subagent**: Use decision tree below to choose specialized agent
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
> - [`/docs/architecture/development/development-process.md`](docs/architecture/development/development-process.md)
> 
> **KEY PRINCIPLES YOU MUST FOLLOW:**
> - ✅ **Self-Contained Implementation**: Each issue must work independently with full functionality
> - ✅ **Early-Phase Mock Development**: Use mock data/services temporarily for parallel development (transitioning to real services soon)
> - ✅ **Vertical Slicing**: Complete features end-to-end, not horizontal layers
> - ✅ **GitHub Status Updates**: Manually update project status at each transition (Todo → In Progress → Ready for Review)
> - ✅ **Use Appropriate Subagents**: Select specialized agents based on task type (see Subagent Selection Guide below)

### 1. Understanding the Project
- **Read the README.md** for high-level project overview and architecture
- **Review `/docs/architecture/`** for detailed technical specifications:
  - `core/system-overview.md` - Complete architecture and design decisions
  - `core/technology-stack.md` - Current tech stack and versions
  - `components/` - Detailed component specifications
  - `deployment/development-setup.md` - Setup requirements

### 2. Finding Tasks to Work On

**Primary Task Source: GitHub Issues**
```bash
# List all open issues
gh issue list

# View specific issue details
gh issue view <issue-number>

# Check issue status and acceptance criteria
gh issue view <issue-number> --web
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

### 4. Subagent Selection Guide

**MANDATORY**: You must use the appropriate specialized subagent for your task type. This ensures expert-level implementation quality.

| Task Type | Recommended Subagent | When to Use | Examples |
|-----------|---------------------|-------------|-----------|
| **Design System, UI Patterns, Visual Design** | `ux-design-expert` | Creating design tokens, style guides, component patterns, user interface designs | Design system foundation, visual patterns, style guides, component architecture |
| **Frontend Components, Svelte Development** | `frontend-expert` | Building UI components, Svelte apps, Tauri desktop integration, DOM manipulation | Svelte components, Tauri integration, desktop app features, interactive UI |
| **AI/ML Integration, Local LLMs, Rust AI** | `ai-ml-engineer` | Implementing AI features, local model integration, NLP processing, mistral.rs setup | AI chat nodes, NLP integration, model loading, AI-powered features |
| **Architecture Review, Code Review, System Design** | `senior-architect-reviewer` | Complex system design, comprehensive code review, architectural decisions, team coordination | System architecture, design reviews, complex feature planning, integration strategies |
| **General Implementation, Research, Multi-step Tasks** | `general-purpose` | Complex research, file searching, multi-step implementations when no specialized agent fits | Project research, complex searches, general development tasks |

**Selection Decision Tree:**
1. **Is this about visual design, UI patterns, or design systems?** → Use `ux-design-expert`
2. **Is this about frontend components, Svelte, or desktop app features?** → Use `frontend-expert`
3. **Is this about AI integration, local models, or NLP?** → Use `ai-ml-engineer`
4. **Is this about architecture review or complex system design?** → Use `senior-architect-reviewer`
5. **Is this complex research or multi-step work?** → Use `general-purpose`

### 4.1. Subagent Instruction Guidelines

**🚨 CRITICAL: When using Task tool with subagents, ALWAYS include these instructions:**

#### **Branching Strategy Instructions**
Every subagent prompt MUST explicitly specify the branching approach:

**For Parent Issue Implementation (Default Approach):**
```
BRANCHING STRATEGY: Work on existing branch `feature/issue-<parent-number>-name`
PR POLICY: DO NOT create any PRs - this will be included in parent issue PR
SCOPE: Implement only the specific requirements for this sub-issue as part of larger feature
```

**For Individual Sub-Issue Implementation:**
```
BRANCHING STRATEGY: Create new branch `feature/issue-<number>-brief-description`
PR POLICY: Create individual PR for this sub-issue when implementation is complete
SCOPE: Complete independent implementation with all acceptance criteria
```

#### **Issue Structure Decision Framework**

**Use Parent Issue Branch (Option 1) When:**
- Sub-issues are tightly coupled and interdependent
- Total implementation is < 2 weeks of work
- Feature should be reviewed as a cohesive unit
- Parent issue explicitly specifies "single branch approach"

**Use Individual Branches (Option 2) When:**
- Sub-issues can be implemented and reviewed independently
- Sub-issues might be worked on by different people
- Feature is large/complex and benefits from incremental review
- Parent issue explicitly specifies "individual branch approach"

#### **Mandatory Subagent Prompt Template**

```markdown
I need to implement Issue #X [TITLE]. 

**BRANCHING STRATEGY:** [Specify approach based on parent issue]
**PR POLICY:** [Specify whether to create PR or not]
**SCOPE:** [Define exact scope of work]

## Context
[Provide project context and current state]

## Requirements
[List specific technical requirements from issue]

## What I Need You To Do
[Clear, specific instructions for implementation]

**🚨 CRITICAL MANDATORY REQUIREMENTS:**
- Follow the specified branching strategy exactly
- Do NOT deviate from the PR policy specified above
- Stay within the defined scope
- **FOLLOW UNIVERSAL PROCESS**: All quality gates and requirements in `/docs/architecture/development/development-process.md` apply to ALL team members (AI agents & humans)

#### **Process Verification**
Before using any subagent:
1. **Check parent issue** for specified branching strategy
2. **If not specified** in parent issue, default to Parent Issue Branch (Option 1)
3. **Include explicit instructions** in every subagent prompt
4. **Verify subagent follows instructions** - if they deviate, immediately correct

### 5. Implementation Workflow

**CRITICAL**: Follow the complete development process in `/docs/architecture/development/development-process.md`

**Step-by-Step Process (Summary - See Full Process Documentation):**

1. **Pick an Issue & Assign Yourself**
   ```bash
   gh issue list
   gh issue view <number>
   gh issue edit <number> --add-assignee "@me"
   ```
   Then manually update project status: Todo → In Progress (via GitHub web interface)

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
   gh pr create --title "Implement Issue #<number>" --body "Closes #<number>"
   ```
   Then manually update project status: In Progress → Ready for Review

6. **Conduct Code Review**
   - **FOLLOW UNIVERSAL PROCESS**: Use the code review guidelines in `/docs/architecture/development/development-process.md` 
   - Use `senior-architect-reviewer` agent for complex features
   - All quality gates and review requirements apply universally to AI agents and human reviewers

**TodoWrite Tool Users - CRITICAL:**
- Your **FIRST todo item** must be: "Complete mandatory startup sequence (git status, determine branching strategy, checkout/switch branch, assign issue, select subagent)"
- Do NOT break the startup sequence into separate todo items
- Only after completing the startup sequence should you add implementation todos

**Before Starting Any Task:**
1. **COMPLETE THE MANDATORY STARTUP SEQUENCE** (steps 1-8 above)
2. **READ THE DEVELOPMENT PROCESS DOCUMENTATION** - This is mandatory
3. **Verify subagent selection** using the decision tree above
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
- **SINGLE EXCEPTION**: `TextNode.svelte` markdown rendering uses controlled HTML injection
  - This follows industry standard (GitHub, React Markdown, etc.)
  - HTML is safely escaped before parsing in `markdownUtils.ts`
  - Suppression documented with detailed justification comment
  - This is the ONLY approved suppression in the entire codebase

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
- [ ] Assigned issue to self (`gh issue edit <number> --add-assignee "@me"`)
- [ ] Updated GitHub project status: Todo → In Progress (manual update)
- [ ] Selected appropriate subagent using the decision tree
- [ ] Read issue requirements and acceptance criteria
- [ ] Read development process documentation (`/docs/architecture/development/development-process.md`)
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
- [ ] Updated GitHub project status: In Progress → Ready for Review (manual)
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