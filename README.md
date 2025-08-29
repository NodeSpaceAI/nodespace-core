# NodeSpace

> **AI-Native Knowledge Management System**  
> A desktop application combining hierarchical note organization with intelligent AI assistance for seamless knowledge work.

## Overview

NodeSpace is a next-generation knowledge management system designed from the ground up to integrate artificial intelligence into every aspect of the user experience. Built with Rust backend and Svelte frontend in a Tauri desktop application, NodeSpace provides a hierarchical block-node interface powered by embedded AI capabilities.

### Core Vision

- **AI-Native**: Every operation can be performed through natural language
- **Hierarchical**: Flexible block-node structure for organizing complex information
- **Real-Time**: Live query updates and collaborative features
- **Desktop-First**: All-in-one application with embedded AI (no external dependencies)
- **Extensible**: Build-time plugin system for custom node types

## Key Features

### 🧠 **Intelligent Node Types**
- **Text Nodes**: ✅ **Implemented** - Advanced markdown support with revolutionary nested formatting system
  - **Context-Aware Formatting**: Intelligent Cmd+B/Cmd+I shortcuts that handle complex nested scenarios
  - **Cross-Marker Compatibility**: Both `**bold**`/`__bold__` and `*italic*`/`_italic_` syntaxes supported
  - **Perfect Nested Handling**: Advanced scenarios like `*__bold__*` → select "bold" → Cmd+B → `*bold*`
  - **marked.js Integration**: Battle-tested markdown parsing with NodeSpace-specific customizations
  - **Headers & Multiline**: Full header inheritance and multiline editing support
- **Task Nodes**: 🚧 **Planned** - Project management with natural language task creation  
- **AI Chat Nodes**: 🚧 **Planned** - Conversational interfaces with context awareness
- **Entity Nodes**: 🚧 **Planned** - Structured data with calculated fields and natural language operations
- **Query Nodes**: 🚧 **Planned** - Live data queries with real-time updates

### 🎨 **Modern Interface**
- **Multi-Node Selection**: Advanced selection system (single, range, multi-select) with full keyboard navigation
- **Hierarchical Visualization**: Modern chevron controls with visual connecting lines
- **Accessibility**: WCAG 2.1 compliant with comprehensive screen reader support
- **Cross-Platform**: Consistent experience across macOS, Windows, and Linux

### ⚡ **Real-Time Architecture**
- Live query results that update automatically as data changes
- Event-driven update coordination across the application
- Intelligent caching with dependency-aware invalidation

### 🔧 **Advanced Features**
- **Natural Language CRUD**: Create and modify entities through conversation
- **Calculated Fields**: Excel-like formulas with dependency tracking
- **Validation System**: Business rules expressed in natural language
- **RAG Integration**: Context-aware search across all content
- **Plugin Architecture**: Extensible node types for specialized workflows

## Technology Stack

### Core Technologies
- **Backend**: Rust with async/await and strong type safety
- **Frontend**: Svelte with reactive state management
- **Desktop Framework**: Tauri for native desktop integration
- **AI Engine**: mistral.rs with Gemma 3n-E4B-it 8B model
- **Database**: Turso (SQLite-compatible) with native vector search for unified data and embeddings

### AI Integration
- **Model**: Gemma 3n-E4B-it 8B (4-6GB RAM, excellent capability/resource balance)
- **Format**: UQFF (Ultra-Quick File Format) for fast model loading
- **Acceleration**: Metal GPU support on macOS
- **Backends**: Configurable support for mistral.rs, Ollama, and Candle

### Development Features
- **Build System**: Cargo workspace with optimized compilation
- **Testing**: Real service integration testing (no mocks)
- **Plugin Development**: Build-time compilation with service injection
- **Cross-Platform**: Native performance on macOS, Windows, and Linux

## Quick Start

### Prerequisites
- **Frontend**: Bun 1.0+ (Node.js not required)
- **Backend**: Rust 1.80+ with Cargo (for Tauri desktop app)
- 8GB+ RAM recommended for AI model
- macOS with Metal GPU support (optimal performance)

### 🛠️ Development CLI Commands

**Always use project CLI commands (never raw GitHub CLI):**

```bash
# View issue details  
bun run gh:view <number>

# List open issues
bun run gh:list  

# Assign issue to yourself
bun run gh:assign <number> "@me"

# Update issue status
bun run gh:status <number> "In Progress"
bun run gh:status <number> "Ready for Review"  
bun run gh:status <number> "Done"

# Create pull request
bun run gh:pr <number>
```

**❌ Don't use:** `gh issue view`, `gh issue list` - these bypass project tooling and automation.

⚠️ **IMPORTANT: All `bun run gh:*` commands must be run from the repository root directory (`/Users/malibio/nodespace/nodespace-core/`), NOT from subdirectories like `packages/desktop-app/`.**

### Development Setup

```bash
# Clone the repository
git clone <repository-url>
cd nodespace-core

# Install frontend dependencies  
bun install

# Download AI model (Gemma 3n-E4B-it 8B UQFF)
# Place in /Users/malibio/nodespace/models/gemma-3-4b-it-UQFF/

# Run in development mode (builds Rust backend + frontend)
bun run tauri:dev
```

### Configuration

NodeSpace supports multiple AI backends for development flexibility:

```rust
// Configuration options
AIBackend::MistralRS {
    model_path: "/path/to/gemma-3n-4b-it-UQFF"
}
AIBackend::Ollama {
    endpoint: "http://localhost:11434"
}
AIBackend::Candle {
    model_config: CandleConfig::default()
}
```

## Architecture Documentation

Comprehensive architecture documentation is organized in the `docs/architecture/` folder:

### 📚 **Core Architecture**
- [System Overview](docs/architecture/core/system-overview.md) - High-level design and components
- [Technology Stack](docs/architecture/core/technology-stack.md) - Detailed technology choices
- [Data Flow](docs/architecture/core/data-flow.md) - Information flow through the system

### 🔧 **Components**
- [Enhanced ContentEditable Architecture](docs/architecture/components/contenteditable-implementation.md) - 🔄 **In Progress** - Logseq-inspired dual-representation with preserved hierarchical indicators
- [Text Editor Architecture Refactor](docs/architecture/decisions/2025-01-text-editor-architecture-refactor.md) - ✅ **Current Plan** - Complete enhanced contenteditable architecture with backlinking
- [Entity Management](docs/architecture/components/entity-management.md) - Structured data with calculated fields
- [AI Integration](docs/architecture/components/ai-integration.md) - Natural language processing and RAG
- [Validation System](docs/architecture/components/validation-system.md) - Business rules and validation
- [Real-Time Updates](docs/architecture/components/real-time-updates.md) - Live query and update system

### 🔌 **Plugin Development**
- [Plugin Architecture](docs/architecture/plugins/plugin-architecture.md) - Build-time plugin system
- [Development Guide](docs/architecture/plugins/development-guide.md) - Creating custom node types
- [Examples](docs/architecture/plugins/examples/) - Sample plugin implementations

### 🚀 **Deployment**
- [Development Setup](docs/architecture/deployment/development-setup.md) - Local development environment
- [Testing Strategies](docs/architecture/deployment/testing-strategies.md) - Comprehensive testing approach

### 📋 **Architecture Decisions**
- [Enhanced ContentEditable Pivot](docs/architecture/decisions/2025-01-contenteditable-pivot.md) - Research findings and architecture pivot decision
- [Text Editor Architecture Refactor](docs/architecture/decisions/2025-01-text-editor-architecture-refactor.md) - Complete implementation plan with Logseq patterns

### 💡 **Design Decisions**
- [Why Rust + Svelte](docs/architecture/design-decisions/why-rust-svelte.md) - Technology choice rationale
- [AI Architecture](docs/architecture/design-decisions/ai-architecture-choices.md) - AI backend strategy
- [Post-MVP Roadmap](docs/architecture/design-decisions/post-mvp-roadmap.md) - Future enhancement plans

## Development Workflow

### Building
```bash
# Development mode
bun run tauri:dev

# Production build
bun run tauri:build

# Frontend development server
bun run dev

# Run tests
bun run test              # Frontend tests with Vitest + Happy DOM
cd packages/desktop-app/src-tauri && cargo test  # Rust backend tests
```

### Testing Philosophy
NodeSpace uses modern, fast testing with Bun runtime:
- **Frontend**: Vitest + Happy DOM (faster, lighter than jsdom)
- **Backend**: Rust integration tests with real services
- **Runtime**: Bun-only (Node.js not required)
- **Coverage**: 80% frontend, comprehensive integration testing
- **Performance**: Bun's speed + Happy DOM = 10x faster test execution

### Recommended IDE Setup

[VS Code](https://code.visualstudio.com/) + [Svelte](https://marketplace.visualstudio.com/items?itemName=svelte.svelte-vscode) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

### Plugin Development
NodeSpace uses "NodeType Extensions" for internal parallel development:
- Build-time compilation for optimal performance
- Service injection pattern for testability
- Shared interfaces for consistent behavior
- Examples and templates for rapid development

## Repository Structure

```
nodespace-core/
├── docs/                       # Documentation and specifications
│   ├── architecture/           # Complete technical architecture
│   │   ├── core/              # Core system design (system-overview, tech stack)
│   │   ├── components/        # Component specifications (AI, entities, validation)
│   │   ├── deployment/        # Development and testing setup
│   │   ├── development/       # Development process and standards
│   │   ├── decisions/         # Architecture decision records (ADRs)
│   │   ├── design-decisions/  # Strategic technology choices
│   │   └── plugins/           # Plugin development guide
│   └── design-system/         # UI design system with live examples
├── packages/desktop-app/      # Main Svelte frontend application
│   ├── src/                   # Application source code
│   │   ├── lib/               # Shared libraries and components
│   │   │   ├── components/    # Reusable Svelte components (TextNode, UI)
│   │   │   ├── design/        # Design system components and theming
│   │   │   ├── services/      # Data services and utilities
│   │   │   └── types/         # TypeScript type definitions
│   │   ├── routes/            # SvelteKit application routes
│   │   └── tests/             # Test suites (component, integration, unit)
│   ├── src-tauri/             # Tauri desktop application wrapper
│   │   ├── src/               # Rust backend code
│   │   ├── icons/             # Application icons for all platforms
│   │   └── capabilities/      # Tauri security capabilities
│   ├── static/                # Static web assets
│   └── [config files]         # Package.json, Svelte config, Tailwind, etc.
├── scripts/                   # Project automation scripts
│   ├── gh-api.ts             # GitHub API integration
│   ├── gh-utils.ts           # GitHub CLI utilities
│   └── github-client.ts      # GitHub client configuration
├── target/                    # Rust build output (generated)
├── Cargo.toml                 # Rust workspace configuration
├── package.json               # Project dependencies and scripts
├── README.md                  # This file - project overview
├── CLAUDE.md                  # AI agent development guidance
└── [build artifacts]         # Generated files (node_modules, coverage, etc.)
```

### Key Directory Purposes

**Core Application:**
- `packages/desktop-app/src/lib/components/` - Main UI components (TextNode, NodeTree, etc.)
- `packages/desktop-app/src/lib/design/` - Design system and theming infrastructure
- `packages/desktop-app/src/lib/services/` - Business logic and data services
- `packages/desktop-app/src-tauri/src/` - Rust backend for desktop integration

**Documentation:**
- `docs/architecture/core/` - High-level system architecture and technology decisions
- `docs/architecture/development/` - Development process, standards, and workflow guides
- `docs/design-system/` - Live UI component documentation with interactive examples

**Testing:**
- `packages/desktop-app/src/tests/` - All test suites organized by type (component, integration, unit)
- `coverage/` - Test coverage reports (generated)

**Automation:**
- `scripts/` - GitHub integration and project management automation
- Bun scripts in `package.json` for CLI commands (gh:view, gh:assign, etc.)

### Development Entry Points

**For New Engineers:**
1. Start with `README.md` (this file) for project overview
2. Read `docs/architecture/core/system-overview.md` for technical architecture
3. Review `docs/architecture/development/overview.md` for development process
4. Check `CLAUDE.md` for AI agent-specific workflow guidance
5. Explore `docs/design-system/index.html` for UI component examples

**For Implementation Work:**
- Component development: `packages/desktop-app/src/lib/components/`
- Design system updates: `packages/desktop-app/src/lib/design/`
- Backend logic: `packages/desktop-app/src-tauri/src/`
- Documentation: `docs/architecture/` (update when making architectural changes)

## Contributing

### Code Standards
- Rust code follows standard formatting (rustfmt)
- Comprehensive error handling with anyhow
- Async/await patterns for all I/O operations
- Strong typing and trait-based abstractions

### NodeType Development
When creating new node types:
1. Define traits for core functionality
2. Implement service injection patterns
3. Create comprehensive integration tests
4. Document AI integration points
5. Follow existing naming conventions

### Documentation
- Update architecture docs for significant changes
- Include examples in plugin development guide
- Maintain consistent markdown formatting
- Cross-reference related documentation

## Roadmap

### Current Phase: Enhanced ContentEditable Architecture
- ✅ **Architecture Research**: Comprehensive Logseq analysis and ProseMirror evaluation completed
- ✅ **Architecture Decision**: Enhanced contenteditable approach with dual-representation patterns
- ✅ **Advanced Formatting System**: Revolutionary context-aware nested formatting with keyboard shortcuts
  - **Context-Aware Algorithm**: Intelligent detection of nested formatting contexts (`*__bold__*` scenarios)
  - **Cross-Marker Compatibility**: Unified handling of equivalent markdown syntaxes
  - **marked.js Integration**: Battle-tested library integration for consistent parsing
  - **Comprehensive Testing**: 19+ test scenarios covering edge cases and performance
- ✅ **GitHub Issues Updated**: 4-phase implementation roadmap with 10 issues created/updated
- 🚧 **Phase 1**: Enhanced Service Layer (ContentProcessor, NodeManager, Core Logic Migration, BacklinkService, EventBus)
- 🚧 **Phase 2**: Backlinking Foundation (Link graph, real-time detection, decorations, navigation)
- 🚧 **Phase 3**: Rich Decorations & Context (Node type decorations, multi-level embeddings, performance optimization)
- 🚧 **Phase 4**: AI Integration Preparation (Extension points, smart suggestions, collaboration readiness)

### Post-MVP: Enterprise Features
See [Post-MVP Roadmap](docs/architecture/design-decisions/post-mvp-roadmap.md) for detailed plans including:
- Advanced resilience and recovery systems
- Comprehensive observability and monitoring
- Performance optimization engine
- Enhanced security model
- Multi-tenant architecture support

## License

[License information to be added]

## Support

[Support information to be added]

---

**NodeSpace** - Intelligent Knowledge Management for the AI Era