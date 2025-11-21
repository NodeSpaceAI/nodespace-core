# OptimisticOperationManager Usage Guide

## Overview

The `OptimisticOperationManager` implements the **fire-and-forget with rollback** pattern for structural operations in NodeSpace. This enables instant perceived latency (<10ms) while ensuring consistency through backend confirmation via LIVE SELECT.

## Architecture Pattern

```typescript
1. Take snapshot of current state (structure + optionally data)
2. Apply optimistic change immediately (UI updates instantly)
3. Fire backend operation (don't await - let LIVE SELECT confirm)
4. On success: LIVE SELECT confirms change (UI already updated)
5. On error: Rollback to snapshot + emit error event for UI notification
```

See: `docs/architecture/development/hierarchy-reactivity-architecture-review.md` Section 5

## Basic Usage

### Simple Structural Operation

```typescript
import { optimisticOperationManager } from '$lib/services/optimistic-operation-manager.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { backend } from '$lib/services/backend-adapter';

async function indentNode(nodeId: string, newParentId: string) {
  await optimisticOperationManager.executeStructuralChange(
    // Step 1: Optimistic update (UI changes immediately)
    () => {
      structureTree.__testOnly_addChild({
        id: `edge-${nodeId}`,
        in: newParentId,
        out: nodeId,
        order: 1.5 // Calculate appropriate fractional order
      });
    },

    // Step 2: Backend operation (fire-and-forget)
    async () => {
      await backend.moveNode(nodeId, newParentId);
      // LIVE SELECT will confirm success automatically
    },

    // Step 3: Operation metadata
    {
      description: 'indent node',
      affectedNodes: [nodeId, newParentId]
    }
  );
}
```

### Operation with Data Changes

If your operation modifies both structure AND node data, enable data snapshotting:

```typescript
async function deleteNodeWithContentChange(nodeId: string) {
  await optimisticOperationManager.executeStructuralChange(
    () => {
      // Remove from structure
      const parentId = structureTree.getParent(nodeId);
      if (parentId) {
        // Remove edge
        structureTree.removeChild({ in: parentId, out: nodeId });
      }

      // Update node content
      const node = nodeData.getNode(nodeId);
      if (node) {
        node.content = '[Deleted]';
        nodeData.nodes.set(nodeId, node);
      }
    },
    async () => {
      await backend.deleteNode(nodeId);
    },
    {
      description: 'delete node',
      affectedNodes: [nodeId],
      snapshotData: true // Enable data snapshot for rollback
    }
  );
}
```

## Batch Operations

Use `executeBatch` to perform multiple related operations with a shared snapshot. If any operation fails, all operations are rolled back atomically:

```typescript
async function outdentNodeWithSiblingTransfer(
  nodeId: string,
  grandparentId: string,
  siblingsToTransfer: string[]
) {
  const operations = [
    // Move node to grandparent level
    {
      optimisticUpdate: () => {
        structureTree.__testOnly_addChild({
          id: `edge-${nodeId}`,
          in: grandparentId,
          out: nodeId,
          order: 2.5
        });
      },
      backendOperation: async () => {
        await backend.moveNode(nodeId, grandparentId);
      },
      description: 'outdent node',
      affectedNodes: [nodeId, grandparentId]
    },

    // Transfer siblings to become children of node
    ...siblingsToTransfer.map((siblingId) => ({
      optimisticUpdate: () => {
        structureTree.__testOnly_addChild({
          id: `edge-${siblingId}`,
          in: nodeId,
          out: siblingId,
          order: 1.0
        });
      },
      backendOperation: async () => {
        await backend.moveNode(siblingId, nodeId);
      },
      description: `transfer sibling ${siblingId}`,
      affectedNodes: [siblingId, nodeId]
    }))
  ];

  await optimisticOperationManager.executeBatch(operations);
}
```

## Error Handling

### Automatic Rollback

When a backend operation fails:
1. Operation manager automatically rolls back to the snapshot
2. Error event is emitted via Tauri (`error:persistence-failed`)
3. UI can display toast notification with retry option

### Error Event Structure

```typescript
interface PersistenceFailedEvent {
  type: 'error:persistence-failed';
  namespace: 'error';
  timestamp: number;
  source: 'optimistic-operation-manager';
  message: string; // Human-readable error message
  failedNodeIds: string[]; // Affected nodes
  failureReason: 'timeout' | 'foreign-key-constraint' | 'database-locked' | 'unknown';
  canRetry: boolean; // Always true - operations are retryable
  affectedOperations: Array<{
    nodeId: string;
    operation: 'create' | 'update' | 'delete';
    error?: string;
  }>;
  metadata?: Record<string, unknown>;
}
```

### Listening to Error Events

```svelte
<!-- ErrorToast.svelte -->
<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import type { PersistenceFailedEvent } from '$lib/services/event-types';

  let errors = $state<Array<PersistenceFailedEvent>>([]);

  $effect(() => {
    const unlisten = listen<PersistenceFailedEvent>(
      'error:persistence-failed',
      (event) => {
        errors.push(event.payload);

        // Auto-dismiss after 5 seconds
        setTimeout(() => {
          errors = errors.filter(e => e !== event.payload);
        }, 5000);
      }
    );

    return () => unlisten.then(fn => fn());
  });

  function retryOperation(nodeIds: string[]) {
    // Implement retry logic - re-execute the operation
    console.log('Retrying operation for nodes:', nodeIds);
  }
</script>

{#each errors as error}
  <div class="error-toast">
    <p>{error.message}</p>
    <p class="error-reason">{error.failureReason}</p>
    <button onclick={() => retryOperation(error.failedNodeIds)}>
      Retry
    </button>
  </div>
{/each}

<style>
  .error-toast {
    background: var(--error-background);
    border: 1px solid var(--error-border);
    padding: 1rem;
    margin: 0.5rem;
    border-radius: 4px;
  }

  .error-reason {
    font-size: 0.875rem;
    color: var(--error-text-muted);
  }
</style>
```

## Snapshot Behavior

### Structure Snapshot (Default)

By default, only structure is snapshot:
- `structureTree` state is saved and restored
- Node data changes are NOT rolled back
- Use for pure structural operations (indent, outdent, move, reorder)

```typescript
// Default behavior - structure only
await optimisticOperationManager.executeStructuralChange(
  optimisticUpdate,
  backendOperation,
  {
    description: 'move node',
    // snapshotData defaults to false
  }
);
```

### Data + Structure Snapshot

Enable data snapshotting for operations that modify both:
- `structureTree` AND `nodeData` are saved and restored
- Use for delete operations, content changes with structure changes

```typescript
// Enable data snapshot
await optimisticOperationManager.executeStructuralChange(
  optimisticUpdate,
  backendOperation,
  {
    description: 'delete node with content',
    snapshotData: true // Snapshot both structure and data
  }
);
```

## Performance Characteristics

| Aspect | Measurement | Notes |
|--------|-------------|-------|
| Perceived Latency | <10ms | Optimistic update is synchronous |
| Backend Confirmation | 50-100ms | LIVE SELECT confirmation delay |
| Rollback Time | <5ms | Snapshot restore is fast (Map assignment) |
| Snapshot Memory | ~100-200 bytes per node | Structure info only (node ID + order) |
| Full Data Snapshot | ~1-2KB per node | Include all node data |

## Integration with Existing Code

### Current Status

As of this implementation, structural operations (indent, outdent, create, delete) are not yet centralized. They may be implemented:
- Inline in components (`BaseNodeViewer.svelte`, `BaseNode.svelte`)
- In keyboard command handlers
- In menu action handlers

### Migration Strategy

To migrate existing operations to use OptimisticOperationManager:

1. **Identify the operation** - Find where indent/outdent/etc. are currently implemented
2. **Extract backend call** - Separate the backend persistence from UI update
3. **Wrap with operation manager** - Use `executeStructuralChange`
4. **Test rollback** - Verify error handling works correctly

### Example Migration

**Before:**
```typescript
// Inline implementation in component
async function handleIndent(nodeId: string) {
  // Update UI
  structureTree.__testOnly_addChild({
    id: `edge-${nodeId}`,
    in: newParentId,
    out: nodeId,
    order: 1.5
  });

  // Persist to backend
  try {
    await backend.moveNode(nodeId, newParentId);
  } catch (error) {
    // Manual error handling
    console.error('Failed to indent node:', error);
    // No automatic rollback!
  }
}
```

**After:**
```typescript
// Using OptimisticOperationManager
import { optimisticOperationManager } from '$lib/services/optimistic-operation-manager.svelte';

async function handleIndent(nodeId: string) {
  await optimisticOperationManager.executeStructuralChange(
    // Optimistic update
    () => {
      structureTree.__testOnly_addChild({
        id: `edge-${nodeId}`,
        in: newParentId,
        out: nodeId,
        order: 1.5
      });
    },
    // Backend operation
    async () => {
      await backend.moveNode(nodeId, newParentId);
    },
    // Metadata
    {
      description: 'indent node',
      affectedNodes: [nodeId, newParentId]
    }
  );
  // Automatic rollback on error!
  // Automatic error event emission!
}
```

## Testing

The OptimisticOperationManager has comprehensive test coverage:
- 16 test cases covering all scenarios
- Tests for successful operations, backend failures, optimistic failures
- Batch operation tests
- Error categorization tests
- Integration tests with reactive stores

Run tests:
```bash
bun run test src/tests/services/optimistic-operation-manager.test.ts
```

## Future Work

### Centralized Operation Service

Create a dedicated service for all structural operations:

```typescript
// src/lib/services/structural-operations.svelte.ts
import { optimisticOperationManager } from './optimistic-operation-manager.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { backend } from './backend-adapter';

export const structuralOperations = {
  async indentNode(nodeId: string, newParentId: string) {
    await optimisticOperationManager.executeStructuralChange(
      () => { /* optimistic update */ },
      async () => { /* backend operation */ },
      { description: 'indent node', affectedNodes: [nodeId, newParentId] }
    );
  },

  async outdentNode(nodeId: string, grandparentId: string) {
    // Implementation
  },

  async createNode(parentId: string, content: string, nodeType: string) {
    // Implementation
  },

  async deleteNode(nodeId: string) {
    // Implementation
  }
};
```

### Retry UI Component

Implement a reusable retry UI component:
- Display error toasts with operation details
- Allow one-click retry of failed operations
- Show success confirmation after retry

### Offline Queue

When backend is unavailable:
- Queue operations locally
- Replay when connection restored
- Resolve conflicts with CRDT or last-write-wins

## Related Documentation

- **Architecture Overview**: `/docs/architecture/development/hierarchy-reactivity-architecture-review.md`
- **Event Types**: `/packages/desktop-app/src/lib/services/event-types.ts`
- **Reactive Structure Tree**: `/packages/desktop-app/src/lib/stores/reactive-structure-tree.svelte.ts`
- **Reactive Node Data**: `/packages/desktop-app/src/lib/stores/reactive-node-data.svelte.ts`

## Questions & Support

For questions about this implementation or integration help:
- Check issue #580 on GitHub
- Review the architecture document (section 5)
- Run the test suite for usage examples
