/**
 * ReactiveNodeService - Svelte 5 Runes Adapter for SharedNodeStore
 *
 * REFACTORED FOR MULTI-VIEWER SUPPORT (Phase 1.2):
 * - Delegates node data storage to SharedNodeStore (single source of truth)
 * - Maintains per-viewer UI state (expand/collapse, focus, depth)
 * - Provides Svelte 5 reactivity via $state and $derived
 * - Backward compatible - existing API preserved
 *
 * Architecture:
 * - SharedNodeStore: Stores all node data (content, hierarchy, metadata)
 * - ReactiveNodeService: Per-viewer instance with UI-specific state
 * - Subscriptions: Automatically updates when SharedNodeStore changes
 *
 * Key features:
 * - Uses Node from $lib/types (with node_type, properties, etc.)
 * - Separates data (Node in SharedNodeStore) from UI state (NodeUIState)
 * - Computes children on-demand instead of storing arrays
 * - Real-time synchronization across multiple viewers
 */

import { v4 as uuidv4 } from 'uuid';
import { ContentProcessor } from './content-processor';
import { SharedNodeStore } from './shared-node-store.svelte';
import { getFocusManager } from './focus-manager.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { createLogger } from '$lib/utils/logger';
import type { Node, NodeUIState } from '$lib/types';
import { createDefaultUIState } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';
import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
import { waitForPendingMoveOperations, trackMoveOperation } from './pending-operations';

const log = createLogger('ReactiveNodeService');
// Schema defaults extraction removed in Issue #690 simplification
// TODO: Re-add schema defaults if needed via backendAdapter.getSchema() + SchemaNodeHelpers
import { moveNode as moveNodeCommand } from './tauri-commands';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

/**
 * Pure function to calculate the insert order for an outdented node.
 * The outdented node should appear right after its old parent among the new parent's children.
 *
 * @param children - Array of children with order, sorted by order
 * @param oldParentId - The old parent (the node is moving out from under this parent)
 * @returns The calculated fractional order for insertion
 *
 * @example
 * // Old parent at order 2.0, next sibling at 3.0 -> insert at 2.5
 * calculateOutdentInsertOrderPure([{nodeId: 'old', order: 2.0}, {nodeId: 'next', order: 3.0}], 'old') // 2.5
 *
 * @example
 * // Old parent at order 2.0, no next sibling -> insert at 3.0
 * calculateOutdentInsertOrderPure([{nodeId: 'old', order: 2.0}], 'old') // 3.0
 *
 * @example
 * // Old parent not found, append to end
 * calculateOutdentInsertOrderPure([{nodeId: 'other', order: 5.0}], 'missing') // 6.0
 */
export function calculateOutdentInsertOrderPure(
  children: Array<{ nodeId: string; order: number }>,
  oldParentId: string
): number {
  const oldParentIndex = children.findIndex(c => c.nodeId === oldParentId);
  if (oldParentIndex >= 0) {
    const oldParentOrder = children[oldParentIndex].order;
    const nextSibling = children[oldParentIndex + 1];
    return nextSibling ? (oldParentOrder + nextSibling.order) / 2 : oldParentOrder + 1.0;
  }
  // Fallback: append to end
  return children.length > 0 ? children[children.length - 1].order + 1.0 : 1.0;
}

/**
 * Calculate the insert order for an outdented node in its new parent's children list.
 * Wrapper that uses the structureTree to get children.
 *
 * @param newParentId - The new parent (grandparent of the node being outdented)
 * @param oldParentId - The old parent (the node is moving out from under this parent)
 * @returns The calculated fractional order for insertion
 */
function calculateOutdentInsertOrder(newParentId: string, oldParentId: string): number {
  const newParentChildren = structureTree.getChildrenWithOrder(newParentId);
  return calculateOutdentInsertOrderPure(newParentChildren, oldParentId);
}

export interface NodeManagerEvents {
  focusRequested: (nodeId: string, position?: number) => void;
  hierarchyChanged: () => void;
  nodeCreated: (nodeId: string) => void;
  nodeDeleted: (nodeId: string) => void;
}

export function createReactiveNodeService(events: NodeManagerEvents) {
  // ADAPTER PATTERN: Delegate data storage to SharedNodeStore
  // This service now focuses on per-viewer UI state and Svelte reactivity

  // Generate unique viewer ID for this instance
  const viewerId = uuidv4();

  // Update source for this viewer
  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId
  };

  // Focus management - single source of truth
  const focusManager = getFocusManager();

  // Get SharedNodeStore instance dynamically (important for tests that call resetInstance())
  const sharedNodeStore = SharedNodeStore.getInstance();

  // UI state only - node data stored in SharedNodeStore
  const _uiState = $state<Record<string, NodeUIState>>({});
  let _rootNodeIds = $state<string[]>([]);
  const _activeNodeId = $state<string | undefined>(undefined);

  // Manual reactivity trigger - incremented when SharedNodeStore updates
  let _updateTrigger = $state(0);

  const contentProcessor = ContentProcessor.getInstance();

  // Subscribe to SharedNodeStore changes for reactive updates
  // Wildcard subscription - updates _updateTrigger when any node changes
  // IMPORTANT: Call destroy() when unmounting to prevent subscription memory leak
  let _subscriptionUnsubscribe: (() => void) | null = sharedNodeStore.subscribeAll((node) => {
    // CRITICAL: Initialize UI state for new nodes (e.g., promoted placeholders)
    // If UI state doesn't exist, compute depth and create default state
    if (!_uiState[node.id]) {
      // Compute depth using backend hierarchy queries
      const computeDepth = (nodeId: string, visited = new Set<string>()): number => {
        if (visited.has(nodeId)) return 0; // Prevent infinite recursion
        visited.add(nodeId);

        const parents = sharedNodeStore.getParentsForNode(nodeId);
        if (parents.length === 0) return 0;
        return 1 + computeDepth(parents[0].id, visited);
      };

      const depth = computeDepth(node.id);
      _uiState[node.id] = createDefaultUIState(node.id, { depth });
    }

    _updateTrigger++;
  });

  // NOTE: Backend now returns children pre-sorted via fractional ordering (ORDER BY order ASC)
  // No frontend sorting needed - we trust the backend's ordering

  function getVisibleNodesRecursive(nodeIds: string[]): (Node & {
    depth: number;
    children: string[];
    expanded: boolean;
    autoFocus: boolean;
    inheritHeaderLevel: number;
    isPlaceholder: boolean;
  })[] {
    const result: (Node & {
      depth: number;
      children: string[];
      expanded: boolean;
      autoFocus: boolean;
      inheritHeaderLevel: number;
      isPlaceholder: boolean;
    })[] = [];

    for (const nodeId of nodeIds) {
      const node = sharedNodeStore.getNode(nodeId);
      const uiState = _uiState[nodeId];
      if (node) {
        // Get children from SharedNodeStore (already sorted by backend via fractional ordering)
        const children = sharedNodeStore.getNodesForParent(nodeId).map((n) => n.id);

        // Derive autoFocus from FocusManager (single source of truth)
        const autoFocus = focusManager.editingNodeId === nodeId;

        // Merge Node with UI state for components
        result.push({
          ...node,
          depth: uiState?.depth || 0,
          children,
          expanded: uiState?.expanded || false,
          autoFocus,
          inheritHeaderLevel: uiState?.inheritHeaderLevel || 0,
          isPlaceholder: uiState?.isPlaceholder || false
        });

        if (uiState?.expanded && children.length > 0) {
          result.push(...getVisibleNodesRecursive(children));
        }
      }
    }

    return result;
  }

  /**
   * Computes visible nodes for a specific parent ID
   * @param viewParentId - The parent node ID to get children for (null = root nodes)
   * @returns Sorted, hierarchical array of visible nodes
   */
  function getVisibleNodesForParent(viewParentId: string | null): (Node & {
    depth: number;
    children: string[];
    expanded: boolean;
    autoFocus: boolean;
    inheritHeaderLevel: number;
    isPlaceholder: boolean;
  })[] {
    void _updateTrigger; // Still track updates for reactivity
    void _rootNodeIds; // Track root node changes

    let viewRoots: string[];
    if (viewParentId !== null) {
      // Get children from SharedNodeStore (already sorted by backend via fractional ordering)
      // NOTE: Parent may not exist yet (e.g., virtual date nodes) - this is OK
      viewRoots = sharedNodeStore.getNodesForParent(viewParentId).map((n) => n.id);
    } else {
      viewRoots = _rootNodeIds;
    }

    return getVisibleNodesRecursive(viewRoots);
  }

  function findNode(nodeId: string): Node | null {
    return sharedNodeStore.getNode(nodeId) || null;
  }

  // DEPRECATED: clearAllAutoFocus() removed
  // Focus is now managed by FocusManager (single source of truth)
  // Use focusManager.setEditingNode(nodeId) or focusManager.clearEditing() instead

  function createNode(
    afterNodeId: string,
    content: string = '',
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning?: boolean,
    originalNodeContent?: string,
    focusNewNode?: boolean,
    paneId: string = DEFAULT_PANE_ID,
    isInitialPlaceholder: boolean = false,
    parentId?: string | null // Accept parent ID as parameter (not stored in node)
  ): string {
    const afterNode = findNode(afterNodeId);
    if (!afterNode) {
      return '';
    }

    const nodeId = uuidv4();

    // Focus management now handled by FocusManager (single source of truth)
    // No need to clear autoFocus flags - derived from focusManager.editingNodeId
    const afterUIState = _uiState[afterNodeId] || createDefaultUIState(afterNodeId);
    const newDepth = afterUIState.depth;

    // Determine parent from parameter or use afterNode's parentId directly
    // CRITICAL FIX: Don't rely on broken parentsCache - use the node's own parentId field
    let newParentId: string | null;
    if (parentId !== undefined) {
      newParentId = parentId;
    } else if (afterNode.parentId !== undefined) {
      // Use afterNode's parentId directly - this is the authoritative source
      newParentId = afterNode.parentId ?? null;
    } else {
      // Fallback: try cache (for compatibility, though it may be null)
      const parents = sharedNodeStore.getParentsForNode(afterNodeId);
      newParentId = parents.length > 0 ? parents[0].id : null;
    }

    let initialContent = content;
    const sourceContent = originalNodeContent || afterNode.content;

    if (content.trim() === '') {
      const headerMatch = sourceContent.match(/^(#{1,6}\s+)/);
      if (headerMatch) {
        initialContent = headerMatch[1];
      }
    } else {
      const sourceHeaderMatch = sourceContent.match(/^(#{1,6}\s+)/);
      const contentHeaderMatch = content.match(/^(#{1,6}\s+)/);

      if (sourceHeaderMatch && !contentHeaderMatch) {
        initialContent = sourceHeaderMatch[1] + content;
      }
    }

    const shouldFocusNewNode = focusNewNode !== undefined ? focusNewNode : !insertAtBeginning;
    const isPlaceholder = initialContent.trim() === '' || /^#{1,6}\s*$/.test(initialContent.trim());

    // Calculate insertAfterNodeId for backend ordering (Issue #657)
    // - If insertAtBeginning: null (insert at start of siblings)
    // - Otherwise: afterNodeId (insert after the reference node)
    const insertAfterNodeId = insertAtBeginning ? null : afterNodeId;

    // Create Node with unified type system
    // NOTE: beforeSiblingId removed - backend uses fractional ordering on edges
    // insertAfterNodeId is a creation hint passed to backend for correct sibling ordering
    const newNode: Node & { insertAfterNodeId?: string | null } = {
      id: nodeId,
      nodeType: nodeType,
      content: initialContent,
      createdAt: new Date().toISOString(),
      parentId: newParentId,
      insertAfterNodeId: insertAfterNodeId,
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      embeddingVector: null,
      mentions: []
    };

    // Create UI state (autoFocus removed - now derived from FocusManager)
    const newUIState = createDefaultUIState(nodeId, {
      depth: newDepth,
      expanded: true,
      inheritHeaderLevel: headerLevel !== undefined ? headerLevel : afterUIState.inheritHeaderLevel,
      isPlaceholder
    });

    // Skip persistence ONLY for initial viewer placeholder (when no children exist)
    // All other nodes (including blank nodes created during editing) persist immediately
    // This prevents UNIQUE constraint violations when indenting blank nodes
    const skipPersistence = isInitialPlaceholder;
    sharedNodeStore.setNode(newNode, viewerSource, skipPersistence);
    _uiState[nodeId] = newUIState;

    // CRITICAL FIX: Register parent-child edge in ReactiveStructureTree immediately
    // This makes the new node visible in visibleNodesFromStores for instant UI feedback
    // In Tauri mode, domain events will also fire but addChild handles duplicates gracefully
    // In browser mode, this is the only way the tree gets updated (no domain events)
    if (newParentId) {
      // Calculate order: insert right after afterNodeId in the sibling list
      const childrenWithOrder = structureTree.getChildrenWithOrder(newParentId);
      const afterNodeIndex = childrenWithOrder.findIndex(c => c.nodeId === afterNodeId);

      let order: number;
      if (insertAtBeginning) {
        // Insert directly BEFORE afterNodeId (not at beginning of all siblings)
        // This handles Enter key at cursor position 0: new node above current node
        if (afterNodeIndex >= 0) {
          const afterNodeOrder = childrenWithOrder[afterNodeIndex].order;
          const prevSibling = childrenWithOrder[afterNodeIndex - 1];
          if (prevSibling) {
            // Fractional order between prevSibling and afterNode
            order = (prevSibling.order + afterNodeOrder) / 2;
          } else {
            // afterNode is first, insert before it
            order = afterNodeOrder - 1.0;
          }
        } else {
          // afterNodeId not found, insert at beginning
          order = childrenWithOrder.length > 0 ? childrenWithOrder[0].order - 1.0 : 1.0;
        }
      } else if (afterNodeIndex >= 0) {
        // Insert right after afterNodeId
        const afterNodeOrder = childrenWithOrder[afterNodeIndex].order;
        const nextSibling = childrenWithOrder[afterNodeIndex + 1];
        if (nextSibling) {
          // Fractional order between afterNode and nextSibling
          order = (afterNodeOrder + nextSibling.order) / 2;
        } else {
          // afterNode is last, append after it
          order = afterNodeOrder + 1.0;
        }
      } else {
        // afterNodeId not found in children, append at end
        order = childrenWithOrder.length > 0
          ? childrenWithOrder[childrenWithOrder.length - 1].order + 1.0
          : 1.0;
      }

      structureTree.addInMemoryRelationship(newParentId, nodeId, order);
    }

    // Set focus using FocusManager (single source of truth)
    // This replaces manual autoFocus flag manipulation
    if (shouldFocusNewNode) {
      focusManager.setEditingNode(nodeId, paneId);
    }

    // NOTE: Sibling linked list updates removed - backend handles ordering via fractional ordering

    // Transfer children from EXPANDED parent nodes only (per sophisticated-keyboard-handling.md)
    // - Expanded parent: New node inherits children (new node appears between parent and children)
    // - Collapsed parent: Children stay with original node (new sibling created after parent)
    const children = sharedNodeStore.getNodesForParent(afterNodeId);
    const structureChildren = structureTree.getChildren(afterNodeId);
    const afterNodeUIState = _uiState[afterNodeId];
    const isExpanded = afterNodeUIState?.expanded ?? true; // Default to expanded if no state

    log.debug('[createNode] Child transfer check:', {
      afterNodeId,
      insertAtBeginning,
      isExpanded,
      sharedNodeStoreChildren: children.length,
      structureTreeChildren: structureChildren.length,
      children: children.map(c => c.id),
      structureIds: structureChildren
    });

    // Only transfer children when:
    // 1. Not inserting at beginning (cursor not at start of line)
    // 2. Parent has children
    // 3. Parent is EXPANDED (collapsed parents keep their children)
    if (!insertAtBeginning && children.length > 0 && isExpanded) {
        // OPTIMISTIC UI: Update structure tree IMMEDIATELY for instant visual feedback
        // This ensures the UI shows the correct hierarchy without waiting for database
        for (const child of children) {
          structureTree.moveInMemoryRelationship(afterNodeId, nodeId, child.id);
        }

        // BACKGROUND PERSISTENCE: Database sync with error handling and rollback
        // Database updates happen asynchronously without blocking UI
        Promise.resolve().then(async () => {
          try {
            // Wait for newNode to be persisted to database
            // This prevents FOREIGN KEY constraint violations when children reference the new parent
            await sharedNodeStore.waitForNodeSaves([nodeId]);

            // Persist child transfers to database
            // Structure tree already updated above, so UI is already correct
            for (const child of children) {
              // Move child to be under the new node in database (with OCC)
              // Backend returns updated child with new version
              const updatedChild = await moveNodeCommand(child.id, child.version, nodeId, null);
              // Sync child's local version from backend response
              sharedNodeStore.updateNode(
                child.id,
                { version: updatedChild.version },
                { type: 'database', reason: 'move-version-sync' },
                { skipPersistence: true, skipConflictDetection: true }
              );
              // Note: No need to call structureTree.moveInMemoryRelationship() again
              // It was already done synchronously above for instant UI update
            }
          } catch (error) {
            // ROLLBACK: Revert optimistic UI changes on failure
            log.error('[createNode] Failed to transfer children to database, rolling back:', error);
            for (const child of children) {
              // Move children back to original parent in structure tree
              structureTree.moveInMemoryRelationship(nodeId, afterNodeId, child.id);
            }
            // TODO(#656): Emit error event for UI notification (toast/banner)
            // https://github.com/malibio/nodespace-core/issues/656
          }
        });
    }

    // Handle hierarchy positioning
    // NOTE: Now using backend queries instead of afterNode.parentId
    const afterNodeParents = sharedNodeStore.getParentsForNode(afterNodeId);
    const afterNodeIsRoot = afterNodeParents.length === 0;

    if (insertAtBeginning) {
      if (!afterNodeIsRoot) {
        // Update sibling pointers (backend handles parent relationships)
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex)
        ];
      }
    } else {
      if (!afterNodeIsRoot) {
        // Insert after sibling (backend handles parent relationships)
      } else {
        const afterNodeIndex = _rootNodeIds.indexOf(afterNodeId);
        _rootNodeIds = [
          ..._rootNodeIds.slice(0, afterNodeIndex + 1),
          nodeId,
          ..._rootNodeIds.slice(afterNodeIndex + 1)
        ];
        _updateTrigger++;
      }
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    events.nodeCreated(nodeId);
    events.hierarchyChanged();

    // If we're not focusing the new node, keep focus on the original node
    if (!shouldFocusNewNode && afterNode) {
      focusManager.setEditingNode(afterNodeId, paneId);
    }

    return nodeId;
  }

  function createPlaceholderNode(
    afterNodeId: string,
    nodeType: string = 'text',
    headerLevel?: number,
    insertAtBeginning: boolean = false,
    originalNodeContent?: string,
    focusNewNode?: boolean,
    paneId: string = DEFAULT_PANE_ID,
    isInitialPlaceholder: boolean = false,
    parentId?: string | null // Accept parent ID as parameter
  ): string {
    return createNode(
      afterNodeId,
      '',
      nodeType,
      headerLevel,
      insertAtBeginning,
      originalNodeContent,
      focusNewNode,
      paneId,
      isInitialPlaceholder,
      parentId // Forward parent ID to createNode
    );
  }

  const debouncedOperations = new Map<
    string,
    {
      fastTimer?: ReturnType<typeof setTimeout>;
      expensiveTimer?: ReturnType<typeof setTimeout>;
      pendingContent?: string;
    }
  >();

  function updateNodeContent(nodeId: string, content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    const headerLevel = contentProcessor.parseHeaderLevel(content);
    const isPlaceholder = content.trim() === '';

    // Issue #424 FIX: Always include nodeType to preserve slash command conversions
    // Pattern conversions work correctly because they call updateNodeType() separately
    // BEFORE the content update, so both the pattern-detected type and content persist together
    sharedNodeStore.updateNode(nodeId, { content, nodeType: node.nodeType }, viewerSource);

    _uiState[nodeId] = {
      ..._uiState[nodeId],
      isPlaceholder,
      inheritHeaderLevel: headerLevel
    };

    scheduleContentProcessing(nodeId, content);
  }

  function updateNodeType(nodeId: string, nodeType: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // CRITICAL: Include content with nodeType update to ensure backend persistence works
    // Some backends may not support updating nodeType alone
    const updatePayload: Partial<Node> = { nodeType, content: node.content };

    // Note: Schema defaults extraction was removed in Issue #690 simplification
    // The backend handles defaults via SchemaNode if needed

    // Skip conflict detection for nodeType changes - they are always intentional conversions
    sharedNodeStore.updateNode(nodeId, updatePayload, viewerSource, {
      skipConflictDetection: true
    });

    scheduleContentProcessing(nodeId, node.content);
  }

  function updateNodeMentions(nodeId: string, mentions: string[]): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Mentions are computed fields - skip persistence (they're derived from content)
    sharedNodeStore.updateNode(nodeId, { mentions: [...mentions] }, viewerSource, {
      skipPersistence: true
    });

    _updateTrigger++;
  }

  function updateNodeProperties(
    nodeId: string,
    properties: Record<string, unknown>,
    merge: boolean = true
  ): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    sharedNodeStore.updateNode(
      nodeId,
      { properties: merge ? { ...node.properties, ...properties } : { ...properties } },
      viewerSource
    );

    _updateTrigger++;
  }

  function scheduleContentProcessing(nodeId: string, content: string): void {
    const existing = debouncedOperations.get(nodeId);
    if (existing?.fastTimer) clearTimeout(existing.fastTimer);
    if (existing?.expensiveTimer) clearTimeout(existing.expensiveTimer);

    debouncedOperations.set(nodeId, { pendingContent: content });

    const fastTimer = setTimeout(() => {
      processFastContentOperations(nodeId, content);
    }, 300);

    const expensiveTimer = setTimeout(() => {
      processExpensiveContentOperations(nodeId, content);
    }, 2000);

    debouncedOperations.set(nodeId, {
      pendingContent: content,
      fastTimer,
      expensiveTimer
    });
  }

  function processFastContentOperations(nodeId: string, _content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      operations.fastTimer = undefined;
      debouncedOperations.set(nodeId, operations);
    }
  }

  function processExpensiveContentOperations(nodeId: string, _content: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Note: Expensive operations (persistence, embeddings, reference propagation)
    // are now handled via domain events and backend polling
    debouncedOperations.delete(nodeId);
  }

  function combineNodes(
    currentNodeId: string,
    previousNodeId: string,
    paneId: string = DEFAULT_PANE_ID
  ): void {
    const currentNode = sharedNodeStore.getNode(currentNodeId);
    const previousNode = sharedNodeStore.getNode(previousNodeId);

    if (!currentNode || !previousNode) return;

    const cleanedContent = stripFormattingSyntax(currentNode.content);
    const combinedContent = previousNode.content + cleanedContent;
    const mergePosition = previousNode.content.length;

    // Use viewer source with explicit immediate persistence (no longer need 'external' workaround)
    // The explicit persist option ensures the content update persists immediately
    const contentUpdateSource: UpdateSource = {
      type: 'viewer' as const,
      viewerId: paneId // Use paneId as viewerId since we're in a standalone function
    };
    sharedNodeStore.updateNode(
      previousNodeId,
      { content: combinedContent },
      contentUpdateSource,
      { persist: 'immediate' } // Explicit immediate persistence (replaces 'external' workaround)
    );
    _uiState[previousNodeId] = { ..._uiState[previousNodeId], autoFocus: false };

    // Handle child promotion using shared depth-aware logic
    // Per sophisticated-keyboard-handling.md:
    // - Collapsed targets: New children inserted at beginning and target auto-expands
    // - Expanded targets: New children appended at end
    const currentChildren = sharedNodeStore.getNodesForParent(currentNodeId);
    if (currentChildren.length > 0) {
      const targetUIState = _uiState[previousNodeId];
      const targetIsCollapsed = !(targetUIState?.expanded ?? true);

      if (targetIsCollapsed) {
        // Auto-expand collapsed target so promoted children become visible
        // Note: We directly update _uiState here since setExpanded() is defined later in the file
        // and we need the immediate effect before promoteChildren() runs
        _uiState[previousNodeId] = {
          ..._uiState[previousNodeId],
          expanded: true
        };
        _updateTrigger++;
      }
    }

    promoteChildren(currentNodeId, previousNodeId);

    // CRITICAL: Collect dependencies that must persist before deletion
    // This prevents "database is locked" errors and FOREIGN KEY violations
    // PersistenceCoordinator will ensure these operations complete before deletion
    const deletionDependencies = [previousNodeId]; // Content update must persist first
    const children = sharedNodeStore.getNodesForParent(currentNodeId);
    if (children.length > 0) {
      deletionDependencies.push(...children.map((c) => c.id)); // Child promotions must complete
    }

    // NOTE: Sibling chain management removed - backend handles ordering via fractional ordering

    // Delete with dependencies - PersistenceCoordinator ensures correct order
    sharedNodeStore.deleteNode(currentNodeId, viewerSource, false, deletionDependencies);
    delete _uiState[currentNodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(currentNodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== currentNodeId);
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    // Set focus on the previous node using FocusManager
    focusManager.setEditingNode(previousNodeId, paneId, mergePosition);
    events.focusRequested(previousNodeId, mergePosition);
    events.nodeDeleted(currentNodeId);
    events.hierarchyChanged();
  }

  function stripFormattingSyntax(content: string): string {
    let cleaned = content;
    // Strip header prefixes (# , ## , etc.)
    cleaned = cleaned.replace(/^#{1,6}\s+/, '');
    // Strip task checkbox ([ ] , [x] , etc.)
    cleaned = cleaned.replace(/^\[\s*[x\s]*\]\s*/, '');
    // Strip quote-block prefixes (> ) from all lines
    cleaned = cleaned
      .split('\n')
      .map((line) => line.replace(/^>\s?/, ''))
      .join('\n');
    return cleaned.trim();
  }

  // NOTE: removeFromSiblingChain function removed - backend handles sibling ordering via fractional ordering

  // ============================================================================
  // INDENT/OUTDENT HELPER TYPES AND FUNCTIONS
  // ============================================================================

  /**
   * Result of outdent validation - contains all data needed for the operation.
   * Returned by validateOutdent() to provide type-safe access to validated data.
   */
  type OutdentValidationResult = {
    oldParentId: string;
    newParentId: string | null;
    siblingsBelow: string[];
    newDepth: number;
  };

  /**
   * Validates whether a node can be outdented and gathers all required data.
   *
   * @param nodeId - The ID of the node to validate for outdent
   * @returns OutdentValidationResult if valid, null if outdent is not possible
   *
   * Validation checks:
   * - Node must exist
   * - Node must have a parent (cannot outdent root nodes)
   *
   * Data gathered:
   * - oldParentId: Current parent to move away from
   * - newParentId: Grandparent to move to (null if becoming root)
   * - siblingsBelow: Siblings after this node that will become children
   * - newDepth: The depth the node will have after outdenting
   */
  function validateOutdent(nodeId: string): OutdentValidationResult | null {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return null;

    // Get parent information
    const parents = sharedNodeStore.getParentsForNode(nodeId);
    if (parents.length === 0) return null; // Cannot outdent root nodes

    const parent = parents[0];
    const oldParentId = parent.id;

    // CRITICAL FIX: Use structureTree as authoritative source for parent hierarchy
    // The Node.parentId field can be stale during rapid operations because:
    // 1. indentNode updates structureTree synchronously via moveInMemoryRelationship
    // 2. But Node.parentId in SharedNodeStore may not reflect the latest hierarchy
    // ReactiveStructureTree is the single source of truth for hierarchy during rapid edits
    const structureTreeParentId = structureTree.getParent(oldParentId);
    const newParentId = structureTreeParentId ?? parent.parentId ?? null;

    // Find siblings that come after this node (they will become children)
    // Backend returns children already sorted via fractional ordering
    const siblings = sharedNodeStore.getNodesForParent(oldParentId).map((n) => n.id);
    const nodeIndex = siblings.indexOf(nodeId);
    const siblingsBelow = nodeIndex >= 0 ? siblings.slice(nodeIndex + 1) : [];

    const newDepth = newParentId ? (_uiState[newParentId]?.depth || 0) + 1 : 0;

    return {
      oldParentId,
      newParentId,
      siblingsBelow,
      newDepth
    };
  }

  /**
   * Rolls back optimistic UI changes made during a failed outdent operation.
   *
   * @param nodeId - The node that was being outdented
   * @param originalUIState - The UI state before outdent was attempted
   * @param originalRootNodeIds - The root node IDs before outdent was attempted
   * @param siblingsBelow - The siblings that were transferred (need depth rollback)
   * @param oldParentId - The original parent ID (for depth calculation)
   * @param newParentId - The target parent ID (for structure tree rollback)
   */
  function rollbackOutdentChanges(
    nodeId: string,
    originalUIState: NodeUIState,
    originalRootNodeIds: string[],
    siblingsBelow: string[],
    oldParentId: string,
    newParentId: string | null
  ): void {
    // Restore main node UI state
    _uiState[nodeId] = originalUIState;
    _rootNodeIds = originalRootNodeIds;
    updateDescendantDepths(nodeId);

    // Rollback sibling depths
    for (const siblingId of siblingsBelow) {
      const sibling = sharedNodeStore.getNode(siblingId);
      if (sibling) {
        _uiState[siblingId] = {
          ..._uiState[siblingId],
          depth: (_uiState[oldParentId]?.depth || 0) + 1
        };
        updateDescendantDepths(siblingId);
      }
    }

    // Rollback structure tree if we moved to a new parent
    if (newParentId) {
      structureTree.moveInMemoryRelationship(newParentId, oldParentId, nodeId);
    }

    events.hierarchyChanged();
    _updateTrigger++;
  }

  // ============================================================================
  // INDENT/OUTDENT OPERATIONS
  // ============================================================================

  async function indentNode(nodeId: string): Promise<boolean> {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return false;

    // Find previous sibling (indent target)
    // CRITICAL FIX: Use node's own parentId first (authoritative), then fallback to structureTree
    // This fixes the placeholder promotion issue where structureTree may be stale when a promoted
    // placeholder has a parentId but hasn't been registered in the structure tree yet.
    const structureTreeParents = sharedNodeStore.getParentsForNode(nodeId);
    const currentParentId = node.parentId ?? (structureTreeParents.length > 0 ? structureTreeParents[0].id : null);

    // Get siblings (already sorted by backend via fractional ordering)
    let siblings: string[];
    if (currentParentId) {
      siblings = sharedNodeStore.getNodesForParent(currentParentId).map((n) => n.id);
    } else {
      siblings = _rootNodeIds;
    }

    const nodeIndex = siblings.indexOf(nodeId);
    if (nodeIndex <= 0) return false; // Can't indent if no previous sibling

    const prevSiblingId = siblings[nodeIndex - 1];
    const prevSibling = sharedNodeStore.getNode(prevSiblingId);
    if (!prevSibling || !pluginRegistry.canHaveChildren(prevSibling.nodeType)) {
      return false; // Can't indent into node that can't have children
    }

    const targetParentId = prevSiblingId;
    const targetParentUIState = _uiState[targetParentId];

    // Save current state for rollback
    const originalUIState = { ..._uiState[nodeId] };
    const originalRootNodeIds = [..._rootNodeIds];

    // Optimistic UI update: Show the move immediately
    _uiState[nodeId] = { ..._uiState[nodeId], depth: (targetParentUIState?.depth || 0) + 1 };
    updateDescendantDepths(nodeId);

    if (!currentParentId) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    setExpanded(targetParentId, true);

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via domain events
    // Sibling positioning removed (Issue #557) - Backend handles ordering via fractional IDs

    // Update local state and notify (BEFORE backend call for instant UI response)
    // Update node's parentId to move it to the new parent
    // NOTE: beforeSiblingId removed from node - backend handles ordering via fractional ordering
    sharedNodeStore.updateNode(
      nodeId,
      { parentId: targetParentId },
      { type: 'database', reason: 'indent-node' },
      { isComputedField: true }
    );

    // CRITICAL FIX: Update ReactiveStructureTree for browser mode
    // In Tauri mode, domain events update the tree, but in browser mode we must do it manually
    structureTree.moveInMemoryRelationship(currentParentId, targetParentId, nodeId);

    events.hierarchyChanged();
    _updateTrigger++;

    // Check if node has been persisted to database yet
    const isNodePersisted = sharedNodeStore.isNodePersisted(nodeId);
    const isOperationExecuting = sharedNodeStore.isNodePersistenceExecuting(nodeId);

    if (!isNodePersisted && !isOperationExecuting) {
      // OPTIMIZATION: Node hasn't been persisted yet AND no operation in flight.
      // Instead of CREATE + MOVE (two operations), we can:
      // 1. Cancel the pending CREATE
      // 2. Re-trigger setNode with updated parentId
      // 3. The CREATE will happen with the correct parent - no MOVE needed!
      //
      // This is more efficient and avoids race conditions entirely.
      log.debug(`[indentNode] Node ${nodeId.substring(0, 8)} not persisted yet, updating parentId for CREATE`);

      // Get the current node with updated parentId
      const updatedNode = sharedNodeStore.getNode(nodeId);
      if (updatedNode) {
        // Update the node's parentId and clear insertAfterNodeId
        // IMPORTANT: insertAfterNodeId referenced a sibling under the OLD parent,
        // so it's invalid for the new parent. Set to null to append at end of new parent's children.
        const nodeWithNewParent = {
          ...updatedNode,
          parentId: targetParentId,
          insertAfterNodeId: null  // Clear - old sibling reference is invalid for new parent
        } as typeof updatedNode & { insertAfterNodeId?: string | null };
        // Re-set the node to trigger a new CREATE with correct parentId
        // The previous pending CREATE will be cancelled by the new one
        sharedNodeStore.setNode(nodeWithNewParent, viewerSource);
      }

      // No moveOperation needed - the CREATE will include the correct parent
      return true;
    }

    // If operation is executing but node not marked persisted yet, fall through to MOVE logic.
    // The moveOperation will wait for pending saves which includes the executing CREATE.

    // Node is already persisted - need to MOVE it in the database
    // Track move operation to prevent race conditions with subsequent indent/outdent
    // CRITICAL: Other hierarchy operations must wait for this move to complete
    // before they can reference this node's edges in the database
    const moveOperation = (async () => {
      try {
        // CRITICAL: Wait for any pending move operations to complete first.
        // This ensures move operations are processed in order, preventing edge conflicts.
        await waitForPendingMoveOperations();

        // CRITICAL: Flush and wait for any pending saves before moving
        // This prevents race conditions where:
        // 1. Content updates race with the move (node's own saves)
        // 2. Parent doesn't exist in DB yet (parent's CREATE still pending)
        const nodesToFlush = [nodeId];
        if (targetParentId) {
          nodesToFlush.push(targetParentId);
        }
        await sharedNodeStore.flushNodeSaves(nodesToFlush);

        // Get fresh node data to ensure we have the latest version
        const freshNode = sharedNodeStore.getNode(nodeId);
        if (!freshNode) {
          log.warn('[indentNode] Node no longer exists after waiting for save:', nodeId);
          return;
        }

        // Now safe to move the node (with OCC)
        // Backend returns updated node with new version
        const updatedNode = await moveNodeCommand(nodeId, freshNode.version, targetParentId, null);

        // Sync local version from backend response
        sharedNodeStore.updateNode(
          nodeId,
          { version: updatedNode.version },
          { type: 'database', reason: 'move-version-sync' },
          { skipPersistence: true, skipConflictDetection: true }
        );
      } catch (error) {
        // Check if error is ignorable (unit test environment or unpersisted nodes)
        const isIgnorableError =
          error instanceof Error &&
          (error.message.includes('404') ||
            error.message.includes('Not Found') ||
            error.message.includes('ECONNREFUSED') ||
            error.message.includes('fetch failed') ||
            error.message.includes('Failed to execute "fetch()"'));

        if (!isIgnorableError) {
          // Non-ignorable error: rollback optimistic update
          _uiState[nodeId] = originalUIState;
          _rootNodeIds = originalRootNodeIds;
          // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles rollback via domain events
          updateDescendantDepths(nodeId);
          if (currentParentId) {
            structureTree.moveInMemoryRelationship(targetParentId, currentParentId, nodeId);
          }
          events.hierarchyChanged();
          _updateTrigger++;

          log.error('[indentNode] Failed to move node, rolled back:', error);
        }
        // Ignorable error: keep UI updates (for unit tests without server)
      }
    })();

    // Track this move so subsequent indent/outdent operations wait for it
    trackMoveOperation(nodeId, moveOperation);

    return true;
  }

  async function outdentNode(nodeId: string): Promise<boolean> {
    // Validate and gather all required data
    const validation = validateOutdent(nodeId);
    if (!validation) return false;

    const { oldParentId, newParentId, siblingsBelow, newDepth } = validation;

    // Save current state for rollback
    const originalUIState = { ..._uiState[nodeId] };
    const originalRootNodeIds = [..._rootNodeIds];

    // Optimistic UI updates for main node
    _uiState[nodeId] = { ..._uiState[nodeId], depth: newDepth };
    updateDescendantDepths(nodeId);

    if (!newParentId) {
      const parentIndex = _rootNodeIds.indexOf(oldParentId);
      _rootNodeIds = [
        ..._rootNodeIds.slice(0, parentIndex + 1),
        nodeId,
        ..._rootNodeIds.slice(parentIndex + 1)
      ];
    }

    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via domain events

    // Check if node has been persisted to database yet
    const isNodePersisted = sharedNodeStore.isNodePersisted(nodeId);
    const isOperationExecuting = sharedNodeStore.isNodePersistenceExecuting(nodeId);

    if (!isNodePersisted) {
      if (!isOperationExecuting) {
        // OPTIMIZATION: Node hasn't been persisted yet AND no operation in flight.
        // Instead of CREATE + MOVE (two operations), we can:
        // 1. Cancel the pending CREATE
        // 2. Re-trigger setNode with updated parentId
        // 3. The CREATE will happen with the correct parent - no MOVE needed!
        //
        // This is more efficient and avoids race conditions where the CREATE
        // fires with stale insertAfterNodeId while parentId has been updated.
        log.debug(`[outdentNode] Node ${nodeId.substring(0, 8)} not persisted yet, updating parentId for CREATE`);

        // Get the current node with updated parentId
        const updatedNode = sharedNodeStore.getNode(nodeId);
        if (updatedNode) {
          // Update the node's parentId and clear insertAfterNodeId
          // IMPORTANT: insertAfterNodeId referenced a sibling under the OLD parent,
          // so it's invalid for the new parent. Set to null to append at end of new parent's children.
          const nodeWithNewParent = {
            ...updatedNode,
            parentId: newParentId,
            insertAfterNodeId: null  // Clear - old sibling reference is invalid for new parent
          } as typeof updatedNode & { insertAfterNodeId?: string | null };
          // Re-set the node to trigger a new CREATE with correct parentId
          // The previous pending CREATE will be cancelled by the new one
          sharedNodeStore.setNode(nodeWithNewParent, { type: 'database', reason: 'outdent-node' });
        }

        // Update structure tree for browser mode
        if (newParentId) {
          const insertOrder = calculateOutdentInsertOrder(newParentId, oldParentId);
          structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId, insertOrder);
        }

        events.hierarchyChanged();
        _updateTrigger++;

        // No moveOperation needed - the CREATE will include the correct parent
        return true;
      } else {
        // CREATE is in-flight! This is a tricky race condition.
        // The CREATE operation reads current node state at execution time (from this.nodes).
        // We need to update the node in the store NOW so the CREATE sees the updated parent
        // and cleared insertAfterNodeId.
        log.debug(`[outdentNode] Node ${nodeId.substring(0, 8)} CREATE in-flight, updating store for in-flight CREATE`);

        // Get current node and update it in-place with new parent and cleared insertAfterNodeId
        const currentNode = sharedNodeStore.getNode(nodeId);
        if (currentNode) {
          // Update the store so the in-flight CREATE sees the new parentId
          // Use 'database' source with isComputedField to avoid triggering another persistence operation
          sharedNodeStore.updateNode(
            nodeId,
            { parentId: newParentId },
            { type: 'database', reason: 'outdent-node-inflight-fix' },
            { isComputedField: true }
          );

          // CRITICAL: Clear insertAfterNodeId since it references a sibling under the OLD parent
          // The CREATE operation reads current node state at execution time, so we need to
          // directly clear this on the node object in the store.
          // Since sharedNodeStore.getNode returns a reference to the actual object, we can mutate it.
          (currentNode as typeof currentNode & { insertAfterNodeId?: string | null }).insertAfterNodeId = null;
        }

        // Update structure tree for browser mode (same as non-executing case)
        if (newParentId) {
          const insertOrder = calculateOutdentInsertOrder(newParentId, oldParentId);
          structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId, insertOrder);
        }

        events.hierarchyChanged();
        _updateTrigger++;

        // The in-flight CREATE will read the updated state and create with correct parent.
        // No additional MOVE needed since we updated before the CREATE reads the state.
        return true;
      }
    }

    // Node is already persisted - proceed with MOVE operation
    // Update local state and transfer siblings (BEFORE backend for instant UI)
    // Update node's parentId to move it to the new parent
    // NOTE: beforeSiblingId removed from node - backend handles ordering via fractional ordering
    sharedNodeStore.updateNode(
      nodeId,
      { parentId: newParentId },
      { type: 'database', reason: 'outdent-node' },
      { isComputedField: true }
    );

    // CRITICAL FIX: Update ReactiveStructureTree for browser mode
    if (newParentId) {
      // Calculate correct order: insert right after oldParentId among newParentId's children
      // This matches backend behavior which uses insertAfterNodeId = oldParentId
      const insertOrder = calculateOutdentInsertOrder(newParentId, oldParentId);
      structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId, insertOrder);
    }

    // Transfer siblings below as children (optimistic UI first)
    if (siblingsBelow.length > 0) {
      // Transfer each sibling - UI updates first, maintaining their original order
      for (let i = 0; i < siblingsBelow.length; i++) {
        const siblingId = siblingsBelow[i];
        const sibling = sharedNodeStore.getNode(siblingId);
        if (sibling) {
          // Optimistic UI update
          const siblingDepth = newDepth + 1;
          _uiState[siblingId] = { ..._uiState[siblingId], depth: siblingDepth };
          updateDescendantDepths(siblingId);

          // Update parentId to move sibling to new parent (nodeId = the outdented node)
          sharedNodeStore.updateNode(
            siblingId,
            { parentId: nodeId },
            { type: 'database', reason: 'outdent-transfer' },
            { isComputedField: true }
          );

          // Update structure tree: move sibling from oldParent to nodeId (the outdented node)
          // Order: sequential integers to maintain original sibling order (AC=1, AD=2, AE=3, etc.)
          structureTree.moveInMemoryRelationship(oldParentId, nodeId, siblingId, i + 1);
        }
      }

      setExpanded(nodeId, true);
    }

    events.hierarchyChanged();
    _updateTrigger++;

    // Track move operation to prevent race conditions with subsequent indent/outdent
    // CRITICAL: Other hierarchy operations must wait for this move to complete
    const moveOperation = (async () => {
      try {
        // CRITICAL: Wait for any pending move operations (from indent) to complete.
        // This prevents "Sibling not found" errors when:
        // 1. User indents B under A (moveNodeCommand in progress, creating edge A→B)
        // 2. User immediately outdents C (needs to insert after B under A)
        // 3. If A→B edge doesn't exist yet, backend fails with "Sibling not found: B"
        await waitForPendingMoveOperations();

        // CRITICAL: Flush ALL pending saves before moveNode.
        // Ensures all node creations (and their edges) are persisted.
        await sharedNodeStore.flushAllPendingSaves();

        // Get fresh node data to ensure we have the latest versions
        const freshNode = sharedNodeStore.getNode(nodeId);
        if (!freshNode) {
          log.warn('[outdentNode] Node no longer exists after waiting for save:', nodeId);
          return;
        }

        // Now safe to move the node and its siblings (with OCC)
        // When outdenting, insert after the old parent (so it appears right below it)
        // Backend returns updated node with new version
        const updatedNode = await moveNodeCommand(nodeId, freshNode.version, newParentId, oldParentId);

        // Sync local version from backend response
        sharedNodeStore.updateNode(
          nodeId,
          { version: updatedNode.version },
          { type: 'database', reason: 'move-version-sync' },
          { skipPersistence: true, skipConflictDetection: true }
        );

        // Move sibling transfers (get fresh versions for each)
        // NOTE: Sequential to preserve sibling order. Parallel execution is possible but requires
        // pre-calculating fractional order values upfront to guarantee ordering. For typical
        // outdent operations (0-3 siblings), sequential latency is negligible.
        for (const siblingId of siblingsBelow) {
          const freshSibling = sharedNodeStore.getNode(siblingId);
          if (freshSibling) {
            // Backend returns updated sibling with new version
            const updatedSibling = await moveNodeCommand(siblingId, freshSibling.version, nodeId, null);
            // Sync sibling's version from backend response
            sharedNodeStore.updateNode(
              siblingId,
              { version: updatedSibling.version },
              { type: 'database', reason: 'move-version-sync' },
              { skipPersistence: true, skipConflictDetection: true }
            );
          }
        }
      } catch (error) {
        // Check if error is ignorable
        const isIgnorableError =
          error instanceof Error &&
          (error.message.includes('404') ||
            error.message.includes('Not Found') ||
            error.message.includes('ECONNREFUSED') ||
            error.message.includes('fetch failed') ||
            error.message.includes('Failed to execute "fetch()"'));

        if (!isIgnorableError) {
          // Non-ignorable error: rollback all changes
          rollbackOutdentChanges(
            nodeId,
            originalUIState,
            originalRootNodeIds,
            siblingsBelow,
            oldParentId,
            newParentId
          );
          log.error('[outdentNode] Failed to move node, rolled back:', error);
        }
        // Ignorable error: keep UI updates (for unit tests without server)
      }
    })();

    // Track this move so subsequent indent/outdent operations wait for it
    trackMoveOperation(nodeId, moveOperation);

    return true;
  }

  /**
   * Promotes children of a deleted node so they maintain their visual depth.
   *
   * When a node is deleted (via Backspace merge), its children should "shift up"
   * visually while maintaining their depth level. This means finding an appropriate
   * parent at (deletedNode.depth) by walking up from the merged-into node.
   *
   * Example:
   * ```
   * Starting:                     After deleting F (merges into "So so so deep"):
   * • A (depth 0)                 • A (depth 0)
   * • C (depth 0)                 • C (depth 0)
   *   └─ • D (depth 1)              └─ • D (depth 1)
   *        └─ • Even deeper (2)          └─ • Even deeper (2)
   *             └─ • So so deep (3)           └─ • So so deepF (merged)
   * • F (depth 0) ← deleted           └─ • G (depth 1, now child of C)
   *   └─ • G (depth 1)                     └─ • H (depth 2)
   *        └─ • H (depth 2)
   * ```
   *
   * The rule: Find the ancestor of the merged-into node that is at the same depth
   * as the deleted node. Children become children of that ancestor, maintaining
   * their visual depth.
   *
   * @param nodeId - The node being deleted whose children will be promoted
   * @param previousNodeId - The node that content is being merged into
   */
  function promoteChildren(nodeId: string, previousNodeId: string): void {
    const children = sharedNodeStore.getNodesForParent(nodeId);
    if (children.length === 0) return;

    // Process each child - find parent and insertion point by walking up from merged-into node
    for (const child of children) {
      const childDepth = _uiState[child.id]?.depth ?? 0;

      // Walk up from merged-into node to find a node at the child's depth
      // That node becomes the insertAfterNodeId
      // The parent of that node becomes the new parent for the child
      let insertAfterNodeId: string | null = null;
      let newParentForChild: string | null = null;
      let currentNode: string | null = previousNodeId;

      while (currentNode !== null) {
        const currentDepth = _uiState[currentNode]?.depth ?? 0;

        if (currentDepth === childDepth) {
          // Found a node at the same depth as the child
          // Child will be inserted after this node
          insertAfterNodeId = currentNode;

          // New parent is this node's parent
          const parents = sharedNodeStore.getParentsForNode(currentNode);
          newParentForChild = parents.length > 0 ? parents[0].id : null;
          break;
        }

        if (currentDepth < childDepth) {
          // We've gone past the target depth
          // This happens when child should be at root or there's no match
          newParentForChild = currentNode;
          break;
        }

        // Move up to parent
        const parents = sharedNodeStore.getParentsForNode(currentNode);
        currentNode = parents.length > 0 ? parents[0].id : null;
      }

      // Calculate the depth this child should have
      const newChildDepth = newParentForChild !== null
        ? (_uiState[newParentForChild]?.depth ?? 0) + 1
        : 0;

      // Use moveNodeCommand to properly update the has_child edge in the backend (with OCC)
      // Insert after the node at the same depth to maintain visual order
      // Don't await - let PersistenceCoordinator handle sequencing via deletionDependencies
      const childVersion = child.version;
      moveNodeCommand(child.id, childVersion, newParentForChild, insertAfterNodeId)
        .then((updatedChild) => {
          // Sync child's local version from backend response
          sharedNodeStore.updateNode(
            child.id,
            { version: updatedChild.version },
            { type: 'database', reason: 'move-version-sync' },
            { skipPersistence: true, skipConflictDetection: true }
          );
        })
        .catch((error) => {
          log.error(`[promoteChildren] Failed to move child ${child.id} to parent ${newParentForChild}:`, error);
        });

      // Update local state for immediate UI feedback
      sharedNodeStore.updateNode(
        child.id,
        { parentId: newParentForChild },
        viewerSource,
        { isComputedField: true } // Skip persistence since moveNodeCommand handles it
      );

      // Update ReactiveStructureTree for immediate UI update
      if (newParentForChild !== null) {
        structureTree.moveInMemoryRelationship(nodeId, newParentForChild, child.id);
      }

      // If promoting to root level, add to _rootNodeIds
      if (newParentForChild === null && !_rootNodeIds.includes(child.id)) {
        _rootNodeIds = [..._rootNodeIds, child.id];
      }

      // Update depth - children maintain their visual position
      _uiState[child.id] = {
        ..._uiState[child.id],
        depth: newChildDepth
      };

      // Update all descendants' depths to maintain relative structure
      updateDescendantDepths(child.id);
    }
  }


  /**
   * Deletes a node from storage.
   *
   * **IMPORTANT**: This function does NOT handle child promotion. Children will remain
   * orphaned with their parentId pointing to the deleted node. Use this only when:
   * 1. The node has no children, OR
   * 2. You have already handled child promotion explicitly
   *
   * **For user-facing deletion**: Use `combineNodes()` instead, which properly handles
   * child promotion using depth-aware logic to maintain outline structure.
   *
   * **Use cases for direct deleteNode()**:
   * - Backend operations where child handling is managed separately
   * - Cleanup operations where orphaned children are intentional
   *
   * @param nodeId - The ID of the node to delete
   */
  function deleteNode(nodeId: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // NOTE: Parent determination no longer needed - cache management removed (Issue #557)
    cleanupDebouncedOperations(nodeId);

    // NOTE: Sibling chain management removed - backend handles ordering via fractional ordering

    // CRITICAL: Update children cache to remove this node from its parent
    // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via domain events

    sharedNodeStore.deleteNode(nodeId, viewerSource);
    delete _uiState[nodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    const rootIndex = _rootNodeIds.indexOf(nodeId);
    if (rootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    events.nodeDeleted(nodeId);
    events.hierarchyChanged();
    _updateTrigger++;
  }

  /**
   * Set the expanded state of a node to a specific value
   * More efficient than toggleExpanded when you know the desired state
   *
   * @param nodeId - The ID of the node to update
   * @param expanded - The desired expanded state
   * @returns True if the state was changed, false if no change was needed or node doesn't exist
   */
  function setExpanded(nodeId: string, expanded: boolean): boolean {
    try {
      // Ensure node exists in SharedNodeStore first
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return false;

      // Initialize UIState if it doesn't exist (e.g., when indenting creates new parent-child relationship)
      let uiState = _uiState[nodeId];
      if (!uiState) {
        // Compute depth by walking parent chain using backend queries
        let depth = 0;
        let currentNodeId = nodeId;
        const visited = new Set<string>();
        while (!visited.has(currentNodeId)) {
          visited.add(currentNodeId);
          const parents = sharedNodeStore.getParentsForNode(currentNodeId);
          if (parents.length === 0) break;
          depth++;
          currentNodeId = parents[0].id;
        }

        uiState = createDefaultUIState(nodeId, { depth, expanded });
        _uiState[nodeId] = uiState;
        _updateTrigger++;
        events.hierarchyChanged();
        return true;
      }

      // No change needed
      if (uiState.expanded === expanded) return false;

      _uiState[nodeId] = { ...uiState, expanded };

      events.hierarchyChanged();
      _updateTrigger++;

      return true;
    } catch (error) {
      log.error(` Error setting expanded state for ${nodeId}:`, error);
      return false;
    }
  }

  /**
   * Batch set expansion states for multiple nodes
   * More efficient than calling setExpanded multiple times as it batches UI updates
   *
   * @param updates - Array of {nodeId, expanded} updates to apply
   * @returns Number of nodes that were actually changed
   */
  function batchSetExpanded(updates: Array<{ nodeId: string; expanded: boolean }>): number {
    try {
      let changedCount = 0;
      const affectedNodes: string[] = [];

      // Apply all updates without triggering events
      for (const { nodeId, expanded } of updates) {
        const uiState = _uiState[nodeId];
        if (!uiState) continue;

        // Skip if no change needed
        if (uiState.expanded === expanded) continue;

        _uiState[nodeId] = { ...uiState, expanded };
        changedCount++;
        affectedNodes.push(nodeId);
      }

      // Only trigger UI update once if anything changed
      if (changedCount > 0) {
        events.hierarchyChanged();
        _updateTrigger++;
      }

      return changedCount;
    } catch (error) {
      log.error(' Error in batch set expanded:', error);
      return 0;
    }
  }

  function toggleExpanded(nodeId: string): boolean {
    try {
      const uiState = _uiState[nodeId];
      if (!uiState) return false;

      const newExpandedState = !uiState.expanded;
      _uiState[nodeId] = { ...uiState, expanded: newExpandedState };

      events.hierarchyChanged();
      _updateTrigger++;

      return true;
    } catch (error) {
      log.error('Error toggling node expansion:', error);
      return false;
    }
  }

  function cleanupDebouncedOperations(nodeId: string): void {
    const operations = debouncedOperations.get(nodeId);
    if (operations) {
      if (operations.fastTimer) clearTimeout(operations.fastTimer);
      if (operations.expensiveTimer) clearTimeout(operations.expensiveTimer);
      debouncedOperations.delete(nodeId);
    }
  }

  // Helper methods
  function updateDescendantDepths(nodeId: string): void {
    const node = sharedNodeStore.getNode(nodeId);
    const uiState = _uiState[nodeId];
    if (!node || !uiState) return;

    const children = sharedNodeStore.getNodesForParent(nodeId);

    for (const child of children) {
      _uiState[child.id] = {
        ..._uiState[child.id],
        depth: uiState.depth + 1
      };
      updateDescendantDepths(child.id);
    }
  }

  return {
    // Reactive getters
    get nodes() {
      return sharedNodeStore.getAllNodes();
    },
    get rootNodeIds() {
      return _rootNodeIds;
    },
    get activeNodeId() {
      return _activeNodeId;
    },
    /**
     * Get visible nodes for a specific parent
     * @param parentId - Parent node ID (null for root-level nodes)
     * @returns Array of visible nodes sorted by visual hierarchy
     */
    visibleNodes(parentId: string | null): (Node & {
      depth: number;
      children: string[];
      expanded: boolean;
      autoFocus: boolean;
      inheritHeaderLevel: number;
      isPlaceholder: boolean;
    })[] {
      return getVisibleNodesForParent(parentId);
    },
    get _updateTrigger() {
      return _updateTrigger;
    },
    // Direct access to UI state for computed properties
    getUIState(nodeId: string) {
      return _uiState[nodeId];
    },

    // Lifecycle management
    /**
     * Cleanup method to prevent subscription memory leaks.
     * MUST be called when the service instance is no longer needed (e.g., on component unmount).
     * Safe to call multiple times (idempotent).
     * Failing to call this will result in subscription accumulation and unnecessary processing.
     */
    destroy() {
      if (_subscriptionUnsubscribe) {
        _subscriptionUnsubscribe();
        _subscriptionUnsubscribe = null;
      }
    },

    // Node operations
    findNode,
    createNode,
    createPlaceholderNode,
    updateNodeContent,
    updateNodeType,
    updateNodeMentions,
    updateNodeProperties,
    combineNodes,
    indentNode,
    outdentNode,
    deleteNode,
    toggleExpanded,
    setExpanded,
    batchSetExpanded,

    // Content processing methods for integration tests
    parseNodeContent(nodeId: string) {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return null;
      return contentProcessor.parseMarkdown(node.content);
    },

    async renderNodeAsHTML(nodeId: string): Promise<string> {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return '';
      const result = await contentProcessor.markdownToDisplay(node.content);
      return result || '';
    },

    getNodeHeaderLevel(nodeId: string): number {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return 0;
      const headerMatch = node.content.match(/^(#{1,6})\s+/);
      return headerMatch ? headerMatch[1].length : 0;
    },

    getNodeDisplayText(nodeId: string): string {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return '';
      return contentProcessor
        .displayToMarkdown(node.content)
        .replace(/[#*`[\]()]/g, '')
        .trim();
    },

    updateNodeContentWithProcessing(nodeId: string, content: string): boolean {
      const node = sharedNodeStore.getNode(nodeId);
      if (!node) return false;
      updateNodeContent(nodeId, content);
      return true;
    },

    // Initialize service with nodes (used for loading from database)
    initializeNodes(
      nodes: Array<Node>,
      options?: {
        expanded?: boolean;
        autoFocus?: boolean;
        inheritHeaderLevel?: number;
        isInitialPlaceholder?: boolean;
        /**
         * Optional parent mapping for test scenarios where backend queries aren't available.
         * Maps node ID to parent ID. If not provided, uses backend queries.
         *
         * Example: { 'child-1': 'parent', 'child-2': 'parent' }
         */
        parentMapping?: Record<string, string | null>;
      }
    ): void {
      // NOTE: We no longer cleanup unpersisted nodes here
      // Instead, BaseNodeViewer reuses existing placeholders when database returns 0 nodes
      // This supports multi-tab/multi-pane scenarios where multiple viewers may share the same placeholder
      // See base-node-viewer.svelte loadChildrenForParent() for placeholder reuse logic

      // CRITICAL: Do NOT clear all global _uiState - this would destroy expansion state
      // for nodes being viewed in other panes. Only update state for nodes being initialized.
      // _rootNodeIds is still cleared because it represents the top-level view (single instance)
      _rootNodeIds = [];

      const defaults = {
        expanded: options?.expanded ?? true,
        autoFocus: options?.autoFocus ?? false,
        inheritHeaderLevel: options?.inheritHeaderLevel ?? 0,
        isInitialPlaceholder: options?.isInitialPlaceholder ?? false,
        parentMapping: options?.parentMapping
      };

      // Compute depth for a node based on parent chain using backend queries
      const computeDepth = (nodeId: string, visited = new Set<string>()): number => {
        if (visited.has(nodeId)) return 0; // Prevent infinite recursion
        visited.add(nodeId);

        const parents = sharedNodeStore.getParentsForNode(nodeId);
        if (parents.length === 0) return 0;
        return 1 + computeDepth(parents[0].id, visited);
      };

      // First pass: Add all nodes to SharedNodeStore
      // For initial placeholder: skip persistence (ephemeral until content added)
      // For database nodes: mark as persisted (already in database)
      // For other nodes: persist normally
      const databaseSource = { type: 'database' as const, reason: 'initialization' };
      const viewerSource = {
        type: 'viewer' as const,
        viewerId: 'base-node-viewer',
        reason: 'placeholder-initialization'
      };

      for (const node of nodes) {
        const isPlaceholder = node.nodeType === 'text' && node.content.trim() === '';
        const source = isPlaceholder ? viewerSource : databaseSource;

        // Only skip persistence for initial viewer placeholder (when no children exist)
        // All other nodes (including blank nodes created during editing) persist immediately
        const skipPersistence = defaults.isInitialPlaceholder && isPlaceholder;

        sharedNodeStore.setNode(node, source, skipPersistence);
        _uiState[node.id] = createDefaultUIState(node.id, {
          depth: 0, // Will be computed in second pass
          expanded: defaults.expanded,
          autoFocus: defaults.autoFocus,
          inheritHeaderLevel: defaults.inheritHeaderLevel,
          // Issue #479: Mark as placeholder if it's the initial viewer placeholder
          // This prevents the content watcher from persisting viewer-local placeholders
          isPlaceholder: skipPersistence && isPlaceholder
        });
      }

      // CRITICAL: Build parent-child relationship cache from loaded nodes
      // This populates sharedNodeStore's childrenCache and parentsCache
      //
      // Always build cache from nodes' parentId field
      // This works for both test scenarios and production (backend sets parentId)
      const nodesByParent = new Map<string | null, string[]>();
      for (const node of nodes) {
        const parentKey = defaults.parentMapping?.[node.id] ?? node.parentId ?? null;
        if (!nodesByParent.has(parentKey)) {
          nodesByParent.set(parentKey, []);
        }
        nodesByParent.get(parentKey)!.push(node.id);
      }

      // NOTE: Cache management removed (Issue #557) - ReactiveStructureTree handles hierarchy via domain events

      // Second pass: Compute depths and identify roots
      // NOTE: Now using backend queries to determine which nodes are children
      const childIds = new Set<string>();
      for (const node of nodes) {
        const parents = sharedNodeStore.getParentsForNode(node.id);
        if (parents.length > 0) {
          childIds.add(node.id);
        }
      }
      _rootNodeIds = nodes.filter((n) => !childIds.has(n.id)).map((n) => n.id);

      for (const node of nodes) {
        const depth = computeDepth(node.id);
        _uiState[node.id] = { ..._uiState[node.id], depth };
      }

      _updateTrigger++;
      events.hierarchyChanged();
    }
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

// For backward compatibility with existing imports
export { createReactiveNodeService as ReactiveNodeService };
