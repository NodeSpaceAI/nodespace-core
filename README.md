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
- **Text Nodes**: Hybrid markdown rendering with precise cursor positioning and AI-assisted editing
- **Task Nodes**: Project management with natural language task creation
- **AI Chat Nodes**: Conversational interfaces with context awareness
- **Entity Nodes**: Structured data with calculated fields and natural language operations
- **Query Nodes**: Live data queries with real-time updates

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
- **Database**: LanceDB for both structured data and vector embeddings

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
- Rust 1.80+ with Cargo
- Node.js 20 LTS for frontend development
- 8GB+ RAM recommended for AI model
- macOS with Metal GPU support (optimal performance)

### Development Setup

```bash
# Clone the repository
git clone <repository-url>
cd nodespace-core

# Install dependencies
cargo build
npm install

# Download AI model (Gemma 3n-E4B-it 8B UQFF)
# Place in /Users/malibio/nodespace/models/gemma-3-4b-it-UQFF/

# Run in development mode
cargo run
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
- [Node Types](docs/architecture/components/node-types.md) - Core node type implementations
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
- [Production Deployment](docs/architecture/deployment/production-deployment.md) - Production considerations

### 💡 **Design Decisions**
- [Why Rust + Svelte](docs/architecture/design-decisions/why-rust-svelte.md) - Technology choice rationale
- [Build-Time Plugins](docs/architecture/design-decisions/why-build-time-plugins.md) - Plugin architecture decisions
- [AI Architecture](docs/architecture/design-decisions/ai-architecture-choices.md) - AI backend strategy
- [Post-MVP Roadmap](docs/architecture/design-decisions/post-mvp-roadmap.md) - Future enhancement plans

## Development Workflow

### Building
```bash
# Development build
cargo build

# Release build
cargo build --release

# Frontend development
npm run dev

# Run tests
cargo test
npm test
```

### Testing Philosophy
NodeSpace follows a "real services" testing approach:
- Integration tests use actual PostgreSQL, LanceDB, and AI models
- No mocking of external services
- Comprehensive end-to-end workflow testing
- Performance benchmarking with real data

### Plugin Development
NodeSpace uses "NodeType Extensions" for internal parallel development:
- Build-time compilation for optimal performance
- Service injection pattern for testability
- Shared interfaces for consistent behavior
- Examples and templates for rapid development

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

### Current Phase: MVP Development
- ✅ Core node types implementation
- ✅ AI integration with mistral.rs
- ✅ Real-time query system
- 🚧 Entity management with calculated fields
- 🚧 Validation system implementation
- 🚧 Plugin architecture completion

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