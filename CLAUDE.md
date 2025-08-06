# NodeSpace Development Agent Guide

## Project Overview

NodeSpace is an AI-native knowledge management system built with Rust backend, Svelte frontend, and Tauri desktop framework. This guide helps agents understand the project structure and find their next tasks.

## Getting Started as an Agent

> 🚨 **CRITICAL: READ DEVELOPMENT PROCESS FIRST** 🚨
> 
> **BEFORE STARTING ANY TASK, YOU MUST READ:**
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
   - Use `senior-architect-reviewer` agent for complex features
   - If review shows ready to merge: Immediately approve and merge
   - If review shows issues: Request changes with specific feedback

**Before Starting Any Task:**
1. **READ THE DEVELOPMENT PROCESS DOCUMENTATION** - This is mandatory
2. **Select appropriate subagent** using the decision tree above
3. Check issue acceptance criteria and requirements
4. Plan self-contained implementation with mock dependencies

### 6. Development Standards

**Code Quality:**
- Follow Rust formatting standards (rustfmt)
- Use TypeScript for frontend type safety
- Implement comprehensive error handling with anyhow/thiserror
- Write tests with mock services temporarily for independent development (transitioning to real services soon)

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

**Before Starting:**
- [ ] Read development process documentation (`/docs/architecture/development/development-process.md`)
- [ ] Selected appropriate subagent using the decision tree
- [ ] Assigned issue to self (`gh issue edit <number> --add-assignee "@me"`)
- [ ] Updated GitHub project status: Todo → In Progress (manual update)
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