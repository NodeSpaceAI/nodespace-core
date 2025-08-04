# NodeSpace Development Agent Guide

## Project Overview

NodeSpace is an AI-native knowledge management system built with Rust backend, Svelte frontend, and Tauri desktop framework. This guide helps agents understand the project structure and find their next tasks.

## Getting Started as an Agent

### 1. Understanding the Project
- **Read the README.md** for high-level project overview and architecture
- **Review `/docs/architecture/`** for detailed technical specifications:
  - `core/system-overview.md` - Complete architecture and design decisions
  - `core/technology-stack.md` - Current tech stack and versions
  - `components/` - Detailed component specifications
  - `deployment/development-setup.md` - Setup requirements

### 2. Finding Tasks to Work On

**Primary Task Source: GitLab Issues**
```bash
# List all open issues
glab issue list

# View specific issue details
glab issue view <issue-number>

# Check issue status and acceptance criteria
glab issue view <issue-number> --web
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
- Real services testing: No mocking, use actual implementations
- Build-time plugins: Compile-time extensibility for performance
- Design system driven: Consistent UI patterns from the start

### 4. Implementation Approach

**Recommended Sequence:**
1. **Foundation**: Tauri + Svelte project structure
2. **Design System**: Tokens, patterns, component architecture
3. **Desktop Shell**: Multi-panel layout system
4. **Core Components**: TextNode, TaskNode, etc.
5. **Backend Integration**: Storage, AI, real-time updates

**Before Starting Any Task:**
1. Check issue acceptance criteria and requirements
2. Review related architecture documentation
3. Understand integration points with other components
4. Verify current technology versions in docs

### 5. Development Standards

**Code Quality:**
- Follow Rust formatting standards (rustfmt)
- Use TypeScript for frontend type safety
- Implement comprehensive error handling with anyhow/thiserror
- Write integration tests with real services (no mocks)

**Git Workflow:**
- Create feature branches: `feature/issue-<number>-brief-description`
- Link commits to issues: `git commit -m "Add TextNode component (closes #4)"`
- Include Claude Code attribution in commit messages

**Documentation:**
- Update relevant docs when changing architecture
- Include code examples in component documentation
- Maintain consistent markdown formatting

### 6. Getting Help

**Resources Available:**
- `/docs/architecture/` - Complete technical specifications
- `README.md` - Quick start and overview
- GitLab issues - Detailed implementation requirements
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
- ✅ GitLab project management setup
- ✅ Technology versions updated to current releases
- ⏳ Foundation implementation (Issue #1) - Ready for agent pickup
- ⏳ Design system, desktop shell, and core components - Planned

---

**Note**: This project uses UI-first development approach. Build user interfaces with mock data first, then integrate backend storage and AI functionality. Focus on creating excellent user experiences before tackling complex technical integrations.