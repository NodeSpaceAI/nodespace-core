# Test Environment Types

This directory contains TypeScript definitions for the test environment to ensure proper typing and compatibility.

## Files

### `globals.d.ts`

Provides global type definitions for the test environment, including:

- `global` object compatibility (Node.js style)
- `globalThis` extensions for browser APIs
- DOM interface extensions for test mocking

### `vitest-env.d.ts`

Vitest-specific environment types including:

- Testing library matchers
- Process global definitions
- Test utility types

## Usage

These type definitions are automatically included in the TypeScript compilation via:

- `tsconfig.json` - includes the types directory
- `vitest.config.ts` - references the global types
- `setup.ts` - provides runtime compatibility layer

## Global vs GlobalThis

In the test environment, both `global` and `globalThis` are supported:

```typescript
// Both patterns work in tests
global.document = mockDocument;
globalThis.document = mockDocument;
```

The setup provides compatibility for legacy test code that uses `global` while maintaining modern `globalThis` support.
