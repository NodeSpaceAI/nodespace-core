# NodeSpace Desktop Application

AI-native knowledge management system built with Tauri, SvelteKit, and TypeScript.

## Quick Start

### Development Setup

```bash
# Install dependencies
bun install

# Start development server
bun run tauri:dev
```

### Testing

NodeSpace uses a simplified testing foundation with 4 core testing types:

```bash
# Run all tests
bun run test

# Run tests with coverage
bun run test:coverage

# Run Rust backend tests
cd src-tauri && cargo test

# Run tests in watch mode
bun run test:watch
```

**Testing Standards:**

- **Frontend**: 80% coverage (lines/functions/statements), 75% branches
- **Backend**: 80% coverage target
- **Integration**: 100% critical workflows tested

### Test Examples

- **Unit Tests**: `src/tests/example/BasicFunctions.test.ts`
- **Component Tests**: `src/tests/component/TextNode.test.ts`
- **Integration Tests**: `src/tests/integration/node-workflow.test.ts`

See `docs/architecture/development/testing-guide.md` for detailed testing patterns and examples.

## Development Commands

```bash
# Quality checks
bun run quality:fix          # Fix linting, formatting, and type errors

# Testing
bun run test                 # Run all frontend tests
bun run test:coverage        # Run with coverage report
cargo test                   # Run Rust tests (from src-tauri/)

# Tauri desktop app
bun run tauri:dev           # Development mode
bun run tauri:build         # Production build
```

## Recommended IDE Setup

[VS Code](https://code.visualstudio.com/) + [Svelte](https://marketplace.visualstudio.com/items?itemName=svelte.svelte-vscode) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer).

## Architecture

- **Frontend**: SvelteKit + TypeScript + TailwindCSS
- **Backend**: Rust with async/await patterns
- **Desktop**: Tauri 2.0 for native integration
- **Testing**: Vitest + Testing Library + Playwright (E2E)
- **Package Manager**: Bun (required)
