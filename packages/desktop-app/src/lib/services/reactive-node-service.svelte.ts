/**
 * ReactiveNodeService - Svelte 5 Runes Adapter for SharedNodeStore
 *
 * - Delegates node data storage to SharedNodeStore (single source of truth)
 * - Maintains per-viewer UI state (expand/collapse, focus, depth)
 * - Provides Svelte 5 reactivity via $state and $derived
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
import { focusManager } from './focus-manager.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import { createLogger } from '$lib/utils/logger';
import type { Node, NodeUIState } from '$lib/types';
import { createDefaultUIState } from '$lib/types';
import type { UpdateSource } from '$lib/types/update-protocol';
import { DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';
import { waitForPendingMoveOperations, trackMoveOperation } from './pending-operations';
import { confirmNodeDeletion } from './delete-confirmation.svelte';

const log = createLogger('ReactiveNodeService');
// Schema defaults extraction removed in a simplification
// TODO: Re-add schema defaults if needed via backendAdapter.getSchema() + SchemaNodeHelpers
import { backendAdapter } from './backend-adapter';
import type { InsertPosition } from '$lib/services/backend-adapter';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';

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

  // Get SharedNodeStore instance dynamically (important for tests that call resetInstance())
  const sharedNodeStore = SharedNodeStore.getInstance();

  // UI state only - node data stored in SharedNodeStore
  const _uiState = $state<Record<string, NodeUIState>>({});
  let _rootNodeIds = $state<string[]>([]);
  const _activeNodeId = $state<string | undefined>(undefined);

  const contentProcessor = ContentProcessor.getInstance();

  // Compute a node's depth by walking its parent chain via backend hierarchy queries.
  function computeDepth(nodeId: string, visited = new Set<string>()): number {
    if (visited.has(nodeId)) return 0; // Prevent infinite recursion
    visited.add(nodeId);

    const parents = sharedNodeStore.getParentsForNode(nodeId);
    if (parents.length === 0) return 0;
    return 1 + computeDepth(parents[0].id, visited);
  }

  // Pure read of a node's UI state for the render/derived path. Returns the persisted
  // entry if present, otherwise a freshly-computed default WITHOUT writing $state.
  //
  // UI state is seeded for real by the genuine mutation/lifecycle paths (node load,
  // create, expand/collapse, structure changes). A lazy write during a read is an
  // ADR-049 violation: getVisibleNodes/getUIState run inside the visibleNodesFromStores
  // $derived (via flattenNodes), so persisting here throws state_unsafe_mutation and
  // wedges the flush — which stalls slash/type-conversion and Task creation in the UI.
  function peekUIState(nodeId: string): NodeUIState {
    return _uiState[nodeId] ?? createDefaultUIState(nodeId, { depth: computeDepth(nodeId) });
  }

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
      // Pure read: getVisibleNodesRecursive feeds $derived render paths, so it must not
      // persist UI state (would throw state_unsafe_mutation). See peekUIState.
      const uiState = node ? peekUIState(nodeId) : _uiState[nodeId];
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

    // Determine parent from explicit parameter or structureTree (single source of truth)
    let newParentId: string | null;
    if (parentId !== undefined) {
      newParentId = parentId;
    } else {
      newParentId = structureTree.getParent(afterNodeId);
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

    // Calculate insertPosition for backend ordering
    // - insertAtBeginning: Beginning (insert before all siblings)
    // - afterNodeId defined: After(afterNodeId)
    // - otherwise: End
    const insertPosition: InsertPosition = insertAtBeginning
      ? { type: 'beginning' }
      : afterNodeId
        ? { type: 'after', siblingId: afterNodeId }
        : { type: 'end' };

    // Create Node with unified type system
    // parentId is NOT on Node — structureTree is the single source of truth for hierarchy.
    // The persistence path derives parent from structureTree.getParent(nodeId) at CREATE time.
    const newNode: Node & { insertPosition?: InsertPosition | null } = {
      id: nodeId,
      nodeType: nodeType,
      content: initialContent,
      createdAt: new Date().toISOString(),
      insertPosition,
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      embeddingVector: null,
      mentions: []
    };

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
      const afterNodeIndex = childrenWithOrder.findIndex((c) => c.nodeId === afterNodeId);

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
        order =
          childrenWithOrder.length > 0
            ? childrenWithOrder[childrenWithOrder.length - 1].order + 1.0
            : 1.0;
      }

      structureTree.addInMemoryRelationship(newParentId, nodeId, order);
    }

    // Set focus using FocusManager (single source of truth)
    // This replaces manual autoFocus flag manipulation
    if (shouldFocusNewNode) {
      focusManager.focusNode(nodeId, paneId);
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
      children: children.map((c) => c.id),
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

      // BACKGROUND PERSISTENCE: Atomically transfer all children in one RPC (C3c).
      // Structure tree already updated above; this persists to the daemon.
      Promise.resolve().then(async () => {
        try {
          // Wait for newNode to be persisted so children can reference it as parent.
          await sharedNodeStore.waitForNodeSaves([nodeId]);

          // Single atomic RPC — all-or-nothing OCC, one transaction in the daemon.
          const updatedChildren = await backendAdapter.moveChildrenToParent(
            nodeId,
            children.map((c) => ({ id: c.id, version: c.version }))
          );

          // Sync versions from the response (order reconciles via RelationshipUpdated events).
          for (const updated of updatedChildren) {
            sharedNodeStore.updateNode(
              updated.id,
              { version: updated.version },
              { type: 'database', reason: 'move-version-sync' },
              { skipPersistence: true }
            );
          }
        } catch (error) {
          // ROLLBACK: Revert optimistic UI changes on failure (preserves the move-rejected notification).
          log.error('[createNode] Failed to transfer children to database, rolling back:', error);
          for (const child of children) {
            structureTree.moveInMemoryRelationship(nodeId, afterNodeId, child.id);
          }
          conflictNotifications.add({
            nodeId,
            message: "Changes couldn't be saved. Please try again.",
            conflictType: 'child-transfer-failure'
          });
        }
      });
    }

    // Handle hierarchy positioning
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
      }
    }

    // NOTE: Sorted children cache removed - backend provides pre-sorted children

    events.nodeCreated(nodeId);
    events.hierarchyChanged();

    // If we're not focusing the new node, keep focus on the original node
    if (!shouldFocusNewNode && afterNode) {
      focusManager.focusNode(afterNodeId, paneId);
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

    // Always include nodeType to preserve slash command conversions
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

    // Only send nodeType — don't include content to avoid cancelling
    // a pending content update from insertSlashCommand (contentTemplate)
    const updatePayload: Partial<Node> = { nodeType };

    sharedNodeStore.updateNode(nodeId, updatePayload, viewerSource);

    scheduleContentProcessing(nodeId, node.content);
  }

  function updateNodeMentions(nodeId: string, mentions: string[]): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Mentions are computed fields - skip persistence (they're derived from content)
    sharedNodeStore.updateNode(nodeId, { mentions: [...mentions] }, viewerSource, {
      skipPersistence: true
    });
  }

  function updateNodeProperties(
    nodeId: string,
    properties: Record<string, unknown>,
    merge: boolean = true
  ): void {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // This function computes the full intended properties bag itself (merged or
    // replaced), so tell the store to use it as-is rather than deep-merging it
    // again onto the existing bag.
    sharedNodeStore.updateNode(
      nodeId,
      { properties: merge ? { ...node.properties, ...properties } : { ...properties } },
      viewerSource,
      { replaceProperties: true }
    );
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
    focusManager.focusNodeAtPosition(previousNodeId, mergePosition, paneId);
    events.focusRequested(previousNodeId, mergePosition);
    events.nodeDeleted(currentNodeId);
    events.hierarchyChanged();
  }

  function stripFormattingSyntax(content: string): string {
    let cleaned = content;
    // Strip header prefixes (# , ## , etc.)
    cleaned = cleaned.replace(/^#{1,6}\s+/, '');
    // Strip checkbox bullet prefix (- [ ] or - [x] or - [X] )
    cleaned = cleaned.replace(/^- \[[ xX]\] /, '');
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

    // structureTree is the single source of truth for hierarchy
    const newParentId = structureTree.getParent(oldParentId);

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
  }

  // ============================================================================
  // INDENT/OUTDENT OPERATIONS
  // ============================================================================

  async function indentNode(nodeId: string): Promise<boolean> {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return false;

    // Find previous sibling (indent target) — structureTree is the single source of truth
    const currentParentId = structureTree.getParent(nodeId);

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

    // NOTE: Cache management removed - ReactiveStructureTree handles hierarchy via domain events
    // Sibling positioning removed - Backend handles ordering via fractional IDs

    // Update ReactiveStructureTree (single source of truth for hierarchy)
    // In Tauri mode, domain events update the tree, but in browser mode we must do it manually
    structureTree.moveInMemoryRelationship(currentParentId, targetParentId, nodeId);

    events.hierarchyChanged();

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
      log.debug(
        `[indentNode] Node ${nodeId.substring(0, 8)} not persisted yet, re-triggering CREATE with new parent`
      );

      // structureTree already updated above — at CREATE time, persistence path will derive
      // parentId from structureTree.getParent(nodeId). Re-trigger setNode to cancel the pending
      // CREATE and schedule a new one (with cleared insertPosition so it appends to new parent).
      // This is a local user action re-triggering its OWN pending write, not a foreign write —
      // it must use `viewerSource`, never a `database` source (a `database` source tells
      // `setNode`'s skip-while-editing guard this is a foreign broadcast, which it isn't; a
      // just-created, just-indented node is by definition focused/pending, so a `database`
      // source would always be declined there).
      const updatedNode = sharedNodeStore.getNode(nodeId);
      const reCreateApplied = updatedNode
        ? sharedNodeStore.setNode(
            {
              ...updatedNode,
              insertPosition: { type: 'end' } as InsertPosition // clear stale sibling ref — append to new parent
            } as typeof updatedNode & { insertPosition?: InsertPosition | null },
            viewerSource
          )
        : false;

      if (reCreateApplied) {
        // No moveOperation needed - the CREATE will include the correct parent
        return true;
      }

      // setNode declined the re-trigger (or the node vanished under us) — do NOT report
      // success as though the CREATE was rescheduled. Fall through to the MOVE path below
      // instead of silently continuing as if the optimization had landed.
      log.warn(
        `[indentNode] Re-triggered CREATE for ${nodeId.substring(0, 8)} was not applied; falling through to MOVE path`
      );
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
        const updatedNode = await backendAdapter.moveNode(
          nodeId,
          freshNode.version,
          targetParentId,
          null
        );

        // Sync local version from backend response
        sharedNodeStore.updateNode(
          nodeId,
          { version: updatedNode.version },
          { type: 'database', reason: 'move-version-sync' },
          { skipPersistence: true }
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
          // NOTE: Cache management removed - ReactiveStructureTree handles rollback via domain events
          updateDescendantDepths(nodeId);
          if (currentParentId) {
            structureTree.moveInMemoryRelationship(targetParentId, currentParentId, nodeId);
          }
          events.hierarchyChanged();

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

    // NOTE: Cache management removed - ReactiveStructureTree handles hierarchy via domain events

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
        log.debug(
          `[outdentNode] Node ${nodeId.substring(0, 8)} not persisted yet, re-triggering CREATE with new parent`
        );

        // Update structure tree first so persistence path can derive parentId from structureTree
        if (newParentId) {
          structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId);
        }

        // Re-trigger setNode to cancel the pending CREATE and schedule a new one.
        // Persistence path derives parentId from structureTree.getParent(nodeId) at CREATE time.
        // This is a local user action re-triggering its OWN pending write, not a foreign write —
        // it must use `viewerSource` (matching indentNode's equivalent re-trigger), never a
        // `database` source. A `database` source tells `setNode`'s skip-while-editing guard this
        // is a foreign broadcast; a just-created, just-outdented node is by definition
        // focused/pending, so a `database` source is always declined there, silently dropping
        // the CREATE and leaving the node's own bookkeeping falsely marked as persisted.
        const updatedNode = sharedNodeStore.getNode(nodeId);
        const reCreateApplied = updatedNode
          ? sharedNodeStore.setNode(
              {
                ...updatedNode,
                insertPosition: { type: 'end' } as InsertPosition // clear stale sibling ref — append to new parent
              } as typeof updatedNode & { insertPosition?: InsertPosition | null },
              viewerSource
            )
          : false;

        if (reCreateApplied) {
          events.hierarchyChanged();
          // No moveOperation needed - the CREATE will include the correct parent
          return true;
        }

        // setNode declined the re-trigger (or the node vanished under us) — do NOT report
        // success as though the CREATE was rescheduled. Fall through to the persisted-node
        // MOVE path below instead of silently continuing as if the optimization had landed.
        log.warn(
          `[outdentNode] Re-triggered CREATE for ${nodeId.substring(0, 8)} was not applied; falling through to MOVE path`
        );
      } else {
        // CREATE is in-flight! Update structureTree now so the in-flight CREATE closure reads
        // the correct parentId from structureTree.getParent(nodeId) at execution time.
        log.debug(
          `[outdentNode] Node ${nodeId.substring(0, 8)} CREATE in-flight, updating structureTree for in-flight CREATE`
        );

        // Update structure tree so in-flight CREATE derives correct parentId
        if (newParentId) {
          structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId);
        }

        // Clear stale insertPosition on the in-store node (references sibling under OLD parent)
        const currentNode = sharedNodeStore.getNode(nodeId);
        if (currentNode) {
          (
            currentNode as typeof currentNode & { insertPosition?: InsertPosition | null }
          ).insertPosition = { type: 'end' } as InsertPosition;
        }

        events.hierarchyChanged();

        // The in-flight CREATE will read structureTree.getParent(nodeId) and create with correct parent.
        return true;
      }
    }

    // Node is already persisted - proceed with MOVE operation
    // structureTree is the single source of truth for hierarchy.

    // Optimistic placement: move to new parent with relative-after intent.
    // Authoritative fractional order arrives via relationship:updated event.
    if (newParentId) {
      structureTree.moveInMemoryRelationship(oldParentId, newParentId, nodeId);
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

          // Update structure tree: move sibling from oldParent to nodeId (the outdented node)
          // Order: sequential integers to maintain original sibling order (AC=1, AD=2, AE=3, etc.)
          structureTree.moveInMemoryRelationship(oldParentId, nodeId, siblingId, i + 1);
        }
      }

      setExpanded(nodeId, true);
    }

    events.hierarchyChanged();

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
        const outdentPosition: InsertPosition = oldParentId
          ? { type: 'after', siblingId: oldParentId }
          : { type: 'end' };
        const updatedNode = await backendAdapter.moveNode(
          nodeId,
          freshNode.version,
          newParentId,
          outdentPosition
        );

        // Sync local version from backend response
        sharedNodeStore.updateNode(
          nodeId,
          { version: updatedNode.version },
          { type: 'database', reason: 'move-version-sync' },
          { skipPersistence: true }
        );

        // Move sibling transfers (get fresh versions for each)
        // NOTE: Sequential to preserve sibling order. Parallel execution is possible but requires
        // pre-issuing all move RPCs and waiting for the daemon to assign orders atomically. For
        // typical outdent operations (0-3 siblings), sequential latency is negligible.
        for (const siblingId of siblingsBelow) {
          const freshSibling = sharedNodeStore.getNode(siblingId);
          if (freshSibling) {
            // Backend returns updated sibling with new version
            const updatedSibling = await backendAdapter.moveNode(
              siblingId,
              freshSibling.version,
              nodeId,
              null
            );
            // Sync sibling's version from backend response
            sharedNodeStore.updateNode(
              siblingId,
              { version: updatedSibling.version },
              { type: 'database', reason: 'move-version-sync' },
              { skipPersistence: true }
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
      // That node becomes the insertion anchor; its parent becomes the new parent for the child
      let insertAfterSiblingId: string | null = null;
      let newParentForChild: string | null = null;
      let currentNode: string | null = previousNodeId;

      while (currentNode !== null) {
        const currentDepth = _uiState[currentNode]?.depth ?? 0;

        if (currentDepth === childDepth) {
          insertAfterSiblingId = currentNode;
          const parents = sharedNodeStore.getParentsForNode(currentNode);
          newParentForChild = parents.length > 0 ? parents[0].id : null;
          break;
        }

        if (currentDepth < childDepth) {
          newParentForChild = currentNode;
          break;
        }

        const parents = sharedNodeStore.getParentsForNode(currentNode);
        currentNode = parents.length > 0 ? parents[0].id : null;
      }

      // Calculate the depth this child should have
      const newChildDepth =
        newParentForChild !== null ? (_uiState[newParentForChild]?.depth ?? 0) + 1 : 0;

      const insertPosition: InsertPosition = insertAfterSiblingId
        ? { type: 'after', siblingId: insertAfterSiblingId }
        : { type: 'end' };

      // Use moveNodeCommand to properly update the has_child edge in the backend (with OCC)
      const childVersion = child.version;
      backendAdapter
        .moveNode(child.id, childVersion, newParentForChild, insertPosition)
        .then((updatedChild) => {
          // Sync child's local version from backend response
          sharedNodeStore.updateNode(
            child.id,
            { version: updatedChild.version },
            { type: 'database', reason: 'move-version-sync' },
            { skipPersistence: true }
          );
        })
        .catch((error) => {
          log.error(
            `[promoteChildren] Failed to move child ${child.id} to parent ${newParentForChild}:`,
            error
          );
        });

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
   * Deletes a node and its entire subtree (cascade).
   *
   * Shows a confirmation dialog when the node has descendants. If confirmed (or the node
   * has no descendants), the deletion is dispatched to the backend atomically. Descendant
   * cleanup happens automatically via `NodeDeleted` domain events arriving through the
   * sync listener — no manual tree pruning is needed here.
   *
   * **For Backspace/empty-node deletion** use `combineNodes()`, which promotes children
   * first and does not cascade.
   *
   * @param nodeId - The ID of the node to delete
   */
  async function deleteNode(nodeId: string): Promise<void> {
    const node = sharedNodeStore.getNode(nodeId);
    if (!node) return;

    // Fetch descendant count to determine whether confirmation is needed.
    const descendants = await backendAdapter.getDescendants(nodeId);
    const descendantCount = descendants.length;

    const confirmed = await confirmNodeDeletion(descendantCount);
    if (!confirmed) {
      log.debug(`Delete cancelled by user for node ${nodeId} (${descendantCount} descendants)`);
      return;
    }

    cleanupDebouncedOperations(nodeId);

    // Capture the view-layer state the optimistic removal below is about to strip,
    // so a backend refusal (subtree access gate) can restore it. The store delete
    // is fire-and-forget and the refusal fires async, so the store invokes this
    // callback once it has restored its own state — the store can't reach
    // `_rootNodeIds`/`_uiState`, which are this service's source of truth for the
    // top-level view (`getVisibleNodesForParent(null)`).
    const removedRootIndex = _rootNodeIds.indexOf(nodeId);
    const removedUiState = _uiState[nodeId];
    const restoreOnRefusal = () => {
      if (removedRootIndex >= 0 && !_rootNodeIds.includes(nodeId)) {
        const restored = [..._rootNodeIds];
        restored.splice(Math.min(removedRootIndex, restored.length), 0, nodeId);
        _rootNodeIds = restored; // reassign for Svelte 5 reactivity
      }
      if (removedUiState !== undefined) {
        _uiState[nodeId] = removedUiState;
      }
      events.hierarchyChanged();
    };

    sharedNodeStore.deleteNode(nodeId, viewerSource, false, [], restoreOnRefusal);
    delete _uiState[nodeId];

    // CRITICAL: Must reassign (not mutate) for Svelte 5 reactivity
    if (removedRootIndex >= 0) {
      _rootNodeIds = _rootNodeIds.filter((id) => id !== nodeId);
    }

    events.nodeDeleted(nodeId);
    events.hierarchyChanged();
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
        events.hierarchyChanged();
        return true;
      }

      // No change needed
      if (uiState.expanded === expanded) return false;

      _uiState[nodeId] = { ...uiState, expanded };

      events.hierarchyChanged();

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
    // Direct pure read of a node's UI state for computed properties — safe to call from
    // $derived/template render paths. For a node not yet seen by this viewer it returns a
    // computed default WITHOUT persisting it (see peekUIState); undefined only when the
    // node isn't in the store.
    getUIState(nodeId: string) {
      return sharedNodeStore.getNode(nodeId) ? peekUIState(nodeId) : _uiState[nodeId];
    },

    // Lifecycle management
    /**
     * No-op kept for call-site compatibility (components call this on unmount).
     * Node data reactivity now comes directly from SharedNodeStore's SvelteMap —
     * there is no subscription to tear down.
     */
    destroy() {},

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
        isInitialPlaceholder: options?.isInitialPlaceholder ?? false
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
          // Mark as placeholder if it's the initial viewer placeholder
          // This prevents the content watcher from persisting viewer-local placeholders
          isPlaceholder: skipPersistence && isPlaceholder
        });
      }

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

      events.hierarchyChanged();
    }
  };
}

export type ReactiveNodeService = ReturnType<typeof createReactiveNodeService>;

export { createReactiveNodeService as ReactiveNodeService };
