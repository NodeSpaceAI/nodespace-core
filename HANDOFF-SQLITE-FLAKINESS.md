# Handoff: Fix SQLite-Related Test Flakiness

## Summary

This document provides comprehensive context for fixing SQLite-related flakiness in integration tests. The `TEST_USE_DATABASE` flag infrastructure has been implemented (PR #337), providing the foundation to eliminate flakiness by running tests in in-memory mode by default.

## What Has Been Completed

### ✅ Infrastructure Implementation (PR #337)

1. **TEST_USE_DATABASE Flag**
   - Environment variable control: `TEST_USE_DATABASE=true` enables database mode
   - Default behavior: in-memory mode (fast, no database, no SQLite locking issues)
   - New npm scripts:
     - `bun run test` - In-memory mode (default, ~2-3 seconds)
     - `bun run test:db` - Database mode (~10-15 seconds)
     - `bun run test:watch` / `bun run test:db:watch` - Watch modes

2. **Utility Functions** (`src/lib/utils/test-environment.ts`)
   - `shouldLogDatabaseErrors()` - Determines if errors should be logged
   - `isTestEnvironment()` - Detects test environment

3. **Test Utilities** (`src/tests/utils/should-use-database.ts`)
   - `shouldUseDatabase()` - Check if database mode is enabled
   - `initializeDatabaseIfNeeded(testName)` - Conditionally init database
   - `cleanupDatabaseIfNeeded(dbPath)` - Conditionally cleanup database
   - `getTestModeName()` - Get current mode name for logging

4. **Code Quality Improvements**
   - Eliminated 11 duplicate environment check patterns
   - Centralized database error logging logic
   - Added comprehensive unit tests (all passing)
   - All 680 existing tests pass in in-memory mode

## The Problem: SQLite Locking Flakiness

### Root Cause

Integration tests that use the HTTP dev-server with SQLite experience intermittent failures (10-20% of runs) due to:

1. **SQLite's "database is locked" errors**
   - Multiple concurrent write operations
   - Connection management issues with WAL mode
   - Backend has write serialization (mutex) which helps but doesn't eliminate the issue

2. **Why Backend Fixes Aren't Sufficient**
   - SQLite locking is inherent to the database engine
   - HTTP adapter introduces additional connection management complexity
   - Multiple test files running concurrently create contention

### Documented Issues

- **Issue #266**: SQLite locking investigation
- **Issue #285**: Long-term solution - refactor tests to use in-memory mode
- **PR #283**: Attempted backend fixes (improved but not eliminated)

### Known Flaky Test

**File:** `src/tests/integration/sibling-chain-integrity.test.ts`
**Test:** `should repair chain when node is deleted`
**Failure Rate:** 10-20% of runs
**Error:** "database is locked" (SQLite internal error)

## What Needs To Be Done

### 🎯 Goal

Refactor integration tests to use in-memory mode by default, eliminating SQLite locking issues while maintaining full database integration testing capability via `TEST_USE_DATABASE=true`.

### Tests Requiring Refactoring

8 integration tests currently use always-on database initialization:

1. ✅ **sibling-chain-integrity.test.ts** (7 tests) - **Known flaky**, highest priority
2. content-processing.test.ts
3. date-node-placeholder-persistence.test.ts
4. edge-cases.test.ts
5. indent-outdent-operations.test.ts
6. integration-scenarios.test.ts
7. regression-prevention.test.ts
8. shift-enter-operations.test.ts

### Refactoring Pattern

#### Current Pattern (Always Uses Database)

```typescript
import { createTestDatabase, initializeTestDatabase, cleanupTestDatabase } from '../utils/test-database';
import { createAndFetchNode, checkServerHealth } from '../utils/test-node-helpers';

describe('My Test', () => {
  let dbPath: string;
  let adapter: HttpAdapter;

  beforeAll(async () => {
    // Always checks server health
    await checkServerHealth(new HttpAdapter('http://localhost:3001'));
  });

  beforeEach(async () => {
    // Always creates database
    dbPath = createTestDatabase('my-test');
    await initializeTestDatabase(dbPath);
    adapter = new HttpAdapter('http://localhost:3001');
  });

  afterEach(async () => {
    // Always cleans up database
    await cleanupTestDatabase(dbPath);
  });

  it('should do something', async () => {
    // Always creates nodes via HTTP
    const node = await createAndFetchNode(adapter, { /* ... */ });
    service.initializeNodes([node]);

    // Test logic...

    // Always verifies database persistence
    const persisted = await adapter.getNode(node.id);
    expect(persisted).toBeDefined();
  });
});
```

#### New Pattern (Conditional Database Use)

```typescript
import { cleanupTestDatabase, waitForDatabaseWrites } from '../utils/test-database';
import { initializeDatabaseIfNeeded, cleanupDatabaseIfNeeded, shouldUseDatabase } from '../utils/should-use-database';
import { createAndFetchNode, checkServerHealth } from '../utils/test-node-helpers';

describe('My Test', () => {
  let dbPath: string | null;  // Now nullable
  let adapter: HttpAdapter;

  beforeAll(async () => {
    // Only check server health in database mode
    if (shouldUseDatabase()) {
      await checkServerHealth(new HttpAdapter('http://localhost:3001'));
    }
  });

  beforeEach(async () => {
    // Conditionally initialize database
    dbPath = await initializeDatabaseIfNeeded('my-test');
    adapter = new HttpAdapter('http://localhost:3001');

    // ... rest of setup
  });

  afterEach(async () => {
    // Conditionally cleanup database
    await cleanupDatabaseIfNeeded(dbPath);
  });

  it('should do something', async () => {
    // In database mode: create via HTTP
    // In in-memory mode: create directly in service
    let node: Node;
    if (shouldUseDatabase()) {
      node = await createAndFetchNode(adapter, { /* ... */ });
    } else {
      // Create node object directly for in-memory testing
      node = {
        id: 'node-1',
        nodeType: 'text',
        content: 'Test',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: null,
        properties: {},
        embeddingVector: null,
        mentions: [],
        createdAt: Date.now(),
        modifiedAt: Date.now()
      };
    }

    service.initializeNodes([node]);

    // Test business logic (works in both modes)...

    // Only verify database persistence in database mode
    if (shouldUseDatabase()) {
      const persisted = await adapter.getNode(node.id);
      expect(persisted).toBeDefined();
    }
  });
});
```

### Key Refactoring Steps

For each test file:

1. **Update Imports**
   ```typescript
   // Remove:
   import { createTestDatabase, initializeTestDatabase } from '../utils/test-database';

   // Add:
   import { initializeDatabaseIfNeeded, cleanupDatabaseIfNeeded, shouldUseDatabase } from '../utils/should-use-database';
   ```

2. **Update beforeAll**
   - Wrap `checkServerHealth()` in `if (shouldUseDatabase())`

3. **Update beforeEach**
   - Change `dbPath: string` to `dbPath: string | null`
   - Replace `createTestDatabase()` + `initializeTestDatabase()` with `initializeDatabaseIfNeeded()`

4. **Update afterEach**
   - Replace `cleanupTestDatabase(dbPath)` with `cleanupDatabaseIfNeeded(dbPath)`

5. **Update Test Bodies**
   - Wrap `createAndFetchNode()` calls with conditional logic
   - Create node objects directly for in-memory mode
   - Wrap database assertions in `if (shouldUseDatabase())`
   - Keep business logic assertions outside conditionals (they work in both modes)

### Helper Function Recommendation

Consider creating a helper to reduce boilerplate:

```typescript
// src/tests/utils/test-node-builder.ts (enhance existing)
export function createNodeForTest(nodeData: Partial<Node>): Node {
  return {
    id: nodeData.id || `node-${Date.now()}`,
    nodeType: nodeData.nodeType || 'text',
    content: nodeData.content || '',
    parentId: nodeData.parentId ?? null,
    containerNodeId: nodeData.containerNodeId ?? null,
    beforeSiblingId: nodeData.beforeSiblingId ?? null,
    properties: nodeData.properties || {},
    embeddingVector: nodeData.embeddingVector ?? null,
    mentions: nodeData.mentions || [],
    createdAt: nodeData.createdAt || Date.now(),
    modifiedAt: nodeData.modifiedAt || Date.now()
  };
}

// Usage in tests:
const node = shouldUseDatabase()
  ? await createAndFetchNode(adapter, nodeData)
  : createNodeForTest(nodeData);
```

## Testing Strategy

### Development Workflow

1. **Default: In-Memory Mode**
   ```bash
   bun run test
   ```
   - Fast feedback loop (~2-3 seconds)
   - No SQLite locking issues
   - Tests business logic

2. **Pre-Commit: Database Mode**
   ```bash
   bun run test:db
   ```
   - Full integration validation (~10-15 seconds)
   - Verifies database persistence
   - Catches integration issues

3. **CI/CD**
   - Run both modes in parallel
   - In-memory for fast feedback
   - Database mode for comprehensive coverage

### Validation After Refactoring

For each refactored test file:

```bash
# Verify in-memory mode works (should be fast and reliable)
bun run test src/tests/integration/[file].test.ts

# Verify database mode still works (comprehensive validation)
TEST_USE_DATABASE=true bun run test src/tests/integration/[file].test.ts
```

## Expected Impact

### Before Refactoring
- ❌ 10-20% failure rate on flaky tests
- ❌ 1,960 database error logs in test output
- ❌ ~10-15 second test execution time
- ❌ SQLite locking contention

### After Refactoring
- ✅ 0% failure rate (in-memory mode)
- ✅ 0 database error logs (in-memory mode)
- ✅ ~2-3 second test execution time
- ✅ No SQLite locking issues
- ✅ Optional database mode for integration validation

## Priority Order

1. **sibling-chain-integrity.test.ts** - Known flaky test, highest impact
2. **integration-scenarios.test.ts** - Complex operations, likely to benefit
3. **edge-cases.test.ts** - Edge cases benefit from fast iteration
4. **indent-outdent-operations.test.ts** - Hierarchy operations
5. **regression-prevention.test.ts** - Regression suite
6. **shift-enter-operations.test.ts** - Content operations
7. **content-processing.test.ts** - Content processing
8. **date-node-placeholder-persistence.test.ts** - Specific feature test

## Success Criteria

- [ ] All 8 integration test files refactored
- [ ] All tests pass in in-memory mode (`bun run test`)
- [ ] All tests pass in database mode (`bun run test:db`)
- [ ] No SQLite locking errors in default mode
- [ ] Test execution time reduced from ~10-15s to ~2-3s (in-memory mode)
- [ ] Flaky test failure rate: 0% in in-memory mode
- [ ] Documentation updated in test file headers

## Additional Context

### Architecture Decisions

**Why not mock the database entirely?**
- Want to maintain real integration testing capability
- Database mode validates actual persistence behavior
- In-memory mode tests business logic without SQLite overhead

**Why conditionally check vs. two separate test files?**
- Single source of truth for test logic
- Easier maintenance (one test file vs. two)
- Clear separation via environment variable
- Can run same tests in both modes for validation

### Files Modified (PR #337)

- `packages/desktop-app/package.json` - Added test:db scripts
- `packages/desktop-app/src/lib/utils/test-environment.ts` - New utilities
- `packages/desktop-app/src/lib/services/shared-node-store.ts` - Use utilities
- `packages/desktop-app/src/lib/services/persistence-coordinator.svelte.ts` - Use utilities
- `packages/desktop-app/src/tests/utils/should-use-database.ts` - Test utilities
- `packages/desktop-app/src/tests/utils/should-use-database.test.ts` - Unit tests
- `packages/desktop-app/src/tests/global-setup.ts` - Conditional test mode

## Questions?

If you encounter issues during refactoring:

1. **Test fails in in-memory mode but passes in database mode?**
   - Check if test is asserting database-specific behavior
   - Move database assertions inside `if (shouldUseDatabase())`

2. **Test still flaky even after refactoring?**
   - Verify `initializeDatabaseIfNeeded()` is being used
   - Check that `createAndFetchNode()` is conditional
   - Ensure server health check is conditional

3. **Tests are slower than expected?**
   - Confirm `TEST_USE_DATABASE` is not set in environment
   - Verify `PersistenceCoordinator` is in test mode (check console output)

## Next Steps

Start with `sibling-chain-integrity.test.ts` as it has documented flakiness. Follow the refactoring pattern above, test in both modes, then move to the next file.

Good luck! 🚀
