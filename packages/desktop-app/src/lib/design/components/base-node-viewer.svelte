<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization
  
  Now uses NodeServiceContext to provide @ autocomplete functionality
  to all TextNode components automatically via proper inheritance.
-->

<script lang="ts">
  import { onMount, onDestroy, getContext, tick } from 'svelte';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import { formatTabTitle } from '$lib/utils/text-formatting';
  import BaseNode from '$lib/design/components/base-node.svelte';
  import BacklinksPanel from '$lib/design/components/backlinks-panel.svelte';
  import SchemaPropertyForm from '$lib/components/property-forms/schema-property-form.svelte';
  // Plugin registry provides all node components dynamically
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';
  import { structureTree as reactiveStructureTree } from '$lib/stores/reactive-structure-tree.svelte';
  import type { Node } from '$lib/types';
  import type { CoreTaskStatus } from '$lib/types/task-node';
  import type { Snippet } from 'svelte';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation';
  import { getViewerId, saveScrollPosition, getScrollPosition } from '$lib/stores/scroll-state';

  // Get paneId from context (set by PaneContent)
  const paneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

  // Props
  let {
    header,
    nodeId = null,
    /**
     * Tab identifier for this viewer instance.
     * Combined with paneId (from context) to create a unique scroll position identifier.
     * Each tab+pane combination maintains independent scroll state, allowing the same
     * document to be viewed in multiple panes with different scroll positions.
     * @default 'default'
     */
    tabId = 'default',
    onTitleChange,
    /**
     * Disable automatic title updates from node content.
     *
     * When true, this component will NOT call onTitleChange based on node content.
     * The parent component is responsible for managing the title completely.
     *
     * Use Cases:
     * - DateNodeViewer: Computes title from date ("Today", "Tomorrow", "Yesterday")
     * - Custom viewers with dynamic titles unrelated to node content
     *
     * @default false - BaseNodeViewer manages title from node.content
     */
    disableTitleUpdates = false
  }: {
    header?: Snippet;
    nodeId?: string | null;
    tabId?: string;
    onTitleChange?: (_title: string) => void;
    onNodeIdChange?: (_nodeId: string) => void; // In type for interface, not used by BaseNodeViewer
    disableTitleUpdates?: boolean;
  } = $props();

  // Get nodeManager from shared context
  const services = getNodeServices();
  if (!services) {
    throw new Error(
      'NodeServices not available. Make sure base-node-viewer is wrapped in NodeServiceContext.'
    );
  }

  const nodeManager = services.nodeManager;

  // Editable header state (for default header when no custom snippet provided)
  // Use local $state so input binding works and we can update tab title immediately
  let headerContent = $state('');

  // Cancellation flag to prevent database writes after component unmounts
  let isDestroyed = false;

  // Placeholder promotion flag - blocks new placeholder creation during async promotion window
  // Prevents race condition where promotion triggers reactive effects before updates complete
  let isPromoting = $state(false);

  // Stable placeholder ID - cached outside reactive system to avoid mutations during derived evaluation
  // Issue #653: Eliminated $effect by using a non-reactive cache variable
  // The ID is created lazily when first needed and reset when placeholder is promoted
  let cachedPlaceholderId: string | null = null;

  /**
   * Get or create a stable placeholder ID
   * Uses lazy initialization - creates ID on first access, reuses on subsequent accesses
   * Call resetPlaceholderId() when placeholder is promoted to ensure fresh ID next time
   */
  function getOrCreatePlaceholderId(): string {
    if (!cachedPlaceholderId) {
      cachedPlaceholderId = globalThis.crypto.randomUUID();
    }
    return cachedPlaceholderId;
  }

  /**
   * Reset the cached placeholder ID (called when placeholder is promoted to real node)
   * This ensures a fresh ID is generated for the next placeholder
   */
  function resetPlaceholderId(): void {
    cachedPlaceholderId = null;
  }

  // Viewer-local placeholder (not in sharedNodeStore until it gets content)
  // This placeholder is only visible to this viewer instance
  // Issue #653: Now uses lazy ID generation instead of $effect-managed state
  const viewerPlaceholder = $derived.by<Node | null>(() => {
    // Note: shouldShowPlaceholder is defined later in the file (forward reference is safe in Svelte 5)
    if (shouldShowPlaceholder) {
      return {
        id: getOrCreatePlaceholderId(),
        nodeType: 'text',
        content: '',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {},
        mentions: []
      };
    }

    return null;
  });

  // Track the viewed node reactively for schema form display
  // Issue #679: Now uses $derived directly since sharedNodeStore.nodes is $state
  const currentViewedNode = $derived(nodeId ? sharedNodeStore.getNode(nodeId) : null);

  // Scroll position tracking
  // Reference to the scroll container element
  let scrollContainer: HTMLElement | null = null;
  // Generate unique viewer ID for this viewer instance
  const viewerId = getViewerId(tabId, paneId);

  // Track expanded state for nodes (viewer-local UI state)
  // Use $state for reactive Map mutations
  let expandedState = $state(new Map<string, boolean>());

  // Track auto-focus nodes (viewer-local UI state)
  // Use $state for reactive Set mutations
  let autoFocusNodes = $state(new Set<string>());

  /**
   * Visible nodes derived from ReactiveStructureTree + ReactiveNodeData (Issue #555)
   *
   * Pure reactivity via $derived - NO _updateTrigger hack needed.
   * Falls back to sharedNodeStore during transition period (Issue #580 tracks removal).
   */
  const visibleNodesFromStores = $derived.by(() => {
    if (!nodeId) return [];
    // Reactive dependency on structure tree is automatic with $state.raw()
    // Svelte will re-run this derived when reactiveStructureTree.children changes

    // Establish reactive dependency on node content/property changes via ReactiveNodeService
    // The _updateTrigger is incremented by SharedNodeStore.subscribeAll callback
    void nodeManager._updateTrigger;

    // Helper function to recursively flatten visible nodes with depth
    function flattenNodes(parentId: string, depth: number, result: Array<any> = []): Array<any> {
      // TRANSITION PERIOD (Issue #580): Try reactive structure tree first, fall back to sharedNodeStore
      // This supports gradual migration where reactive stores are being populated asynchronously.
      // Once all nodes are in reactive stores, remove fallback and use only reactiveStructureTree.
      let childIds = reactiveStructureTree.getChildren(parentId);
      if (childIds.length === 0) {
        const cachedNodes = sharedNodeStore.getNodesForParent(parentId);
        if (cachedNodes && cachedNodes.length > 0) {
          childIds = cachedNodes.map(n => n.id);
        }
      }

      for (const id of childIds) {
        // Issue #679: sharedNodeStore is now single source of truth (consolidated from nodeData)
        const node = sharedNodeStore.getNode(id);
        if (!node) continue;

        // Get children IDs for this node
        // TRANSITION PERIOD (Issue #580): Same fallback pattern as above
        let children = reactiveStructureTree.getChildren(node.id);
        if (children.length === 0) {
          const cachedChildren = sharedNodeStore.getNodesForParent(node.id);
          if (cachedChildren) {
            children = cachedChildren.map(c => c.id);
          }
        }

        // Build node with UI state
        // Get UI state from ReactiveNodeService (has expanded: true by default)
        const uiState = nodeManager.getUIState(node.id);
        const nodeWithUI = {
          ...node,
          depth,
          children,
          expanded: uiState?.expanded ?? true, // Default to true (expanded by default)
          autoFocus: autoFocusNodes.has(node.id),
          inheritHeaderLevel: uiState?.inheritHeaderLevel ?? 0,
          isPlaceholder: false
        };

        result.push(nodeWithUI);

        // Recursively add children if this node is expanded
        if (nodeWithUI.expanded && children.length > 0) {
          flattenNodes(node.id, depth + 1, result);
        }
      }

      return result;
    }

    // Start flattening from the root nodeId at depth 0
    return flattenNodes(nodeId, 0);
  });

  // ============================================================================
  // Load children on component mount
  // ============================================================================
  // NOTE: This component is recreated via {#key} when nodeId changes (see pane-content.svelte)
  // So onMount runs fresh for each nodeId, eliminating the need for a reactive effect
  // ============================================================================

  onMount(() => {
    // Set up scroll position management
    // Restore scroll position when viewer becomes active
    let scrollCleanup: (() => void) | null = null;

    if (scrollContainer) {
      const savedPosition = getScrollPosition(viewerId);
      // Use requestAnimationFrame to ensure DOM is ready
      requestAnimationFrame(() => {
        if (scrollContainer) {
          scrollContainer.scrollTop = savedPosition;
        }
      });

      // Set up scroll position saving
      const handleScroll = () => {
        if (scrollContainer) {
          saveScrollPosition(viewerId, scrollContainer.scrollTop);
        }
      };

      scrollContainer.addEventListener('scroll', handleScroll, { passive: true });

      // Store cleanup function
      scrollCleanup = () => {
        if (scrollContainer) {
          scrollContainer.removeEventListener('scroll', handleScroll);
        }
      };
    }

    if (!nodeId) {
      // currentViewedNode is now $derived - no manual assignment needed
      return scrollCleanup || undefined;
    }

    // Load children asynchronously (non-blocking)
    (async () => {
      try {
        // Capture disableTitleUpdates at mount time
        const shouldDisableTitleUpdates = disableTitleUpdates;

        // Load children asynchronously
        // Note: loadChildrenForParent has internal cache checking, so this is efficient
        await loadChildrenForParent(nodeId);

        // CRITICAL: Prevent state updates after component destruction
        if (isDestroyed) {
          return;
        }

        // After loading completes, initialize header content and update tab title
        const node = sharedNodeStore.getNode(nodeId);
        headerContent = node?.content || '';
        // Issue #679: No longer need viewedNodeCache workaround
        // sharedNodeStore.nodes is now $state, so currentViewedNode $derived updates automatically

        // Update tab title after node is loaded
        if (!shouldDisableTitleUpdates) {
          updateTabTitle(headerContent);
        }
      } catch (error) {
        console.error('[BaseNodeViewer] Failed to load children:', error);
      }
    })();

    // Return cleanup function
    return scrollCleanup || undefined;
  });

  /**
   * Update tab title from header content
   *
   * Uses shared formatTabTitle utility to ensure consistent tab title formatting
   * across all viewers and navigation. Multi-line content is automatically
   * truncated to first line, and long titles are shortened with ellipsis.
   *
   * @param content - Full node content (may be multi-line)
   */
  function updateTabTitle(content: string) {
    if (onTitleChange) {
      onTitleChange(formatTabTitle(content));
    }
  }

  /**
   * Handle header content changes (for default editable header)
   * Updates local state, tab title, and persists to database
   */
  function handleHeaderInput(newValue: string) {
    // Update local state (since we use one-way binding)
    headerContent = newValue;

    // Update tab title immediately
    updateTabTitle(newValue);

    // Update node content in database if nodeId exists
    // Use the same method as child nodes to ensure consistent behavior
    if (nodeId) {
      try {
        nodeManager.updateNodeContent(nodeId, newValue);
      } catch (error) {
        console.error('[BaseNodeViewer] Failed to update header content:', error);
        // TODO: Show user-facing error notification via toast/notification system
      }
    }
  }

  /**
   * Node types that have structured content and cannot accept arbitrary merges
   * These nodes must maintain specific formatting (e.g., code fences, quote prefixes)
   */
  const STRUCTURED_NODE_TYPES = ['code-block', 'quote-block'] as const;

  /**
   * Check if a node type is a structured node that cannot accept arbitrary merges
   * @param nodeType - The node type to check
   * @returns true if the node type is structured and cannot accept merges
   */
  function isStructuredNode(nodeType: string): boolean {
    return STRUCTURED_NODE_TYPES.includes(nodeType as (typeof STRUCTURED_NODE_TYPES)[number]);
  }

  /**
   * Extract and transform node properties into component-compatible metadata
   *
   * Handles the mismatch between database property storage and component metadata expectations:
   * - Task nodes: Maps properties.task.status → metadata.taskState (for icon rendering)
   * - Other nodes: Returns properties as-is for future extension
   *
   * @param node - Node with properties from database
   * @returns Metadata object compatible with node component expectations
   */
  function extractNodeMetadata(node: {
    nodeType: string;
    properties?: Record<string, unknown>;
  }): Record<string, unknown> {
    const properties = node.properties || {};

    // Task nodes: Map schema status to taskState for icon rendering
    if (node.nodeType === 'task') {
      const taskProps = properties[node.nodeType] as Record<string, unknown> | undefined;
      const status = taskProps?.status || properties.status; // Support both nested and flat formats

      // Map task status to NodeState expected by TaskNode
      let taskState: 'pending' | 'inProgress' | 'completed' = 'pending';
      if (status === 'IN_PROGRESS') {
        taskState = 'inProgress';
      } else if (status === 'DONE') {
        taskState = 'completed';
      } else if (status === 'OPEN') {
        taskState = 'pending';
      }

      return { taskState, ...properties };
    }

    // Default: Return properties as-is
    return properties;
  }

  /**
   * Maps UI task state to schema status value.
   * UI uses: 'pending', 'inProgress', 'completed'
   * Schema uses: 'open', 'in_progress', 'done', 'cancelled'
   */
  function mapNodeStateToSchemaStatus(state: string): CoreTaskStatus {
    switch (state) {
      case 'pending':
        return 'open';
      case 'inProgress':
        return 'in_progress';
      case 'completed':
        return 'done';
      default:
        return 'open';
    }
  }

  /**
   * Update a schema field value for a node (schema-aware property update)
   *
   * Follows the same nested format pattern as schema-property-form.svelte:
   * - Builds nested structure: properties[nodeType][fieldName] = value
   * - Handles auto-migration from flat to nested format
   * - Calls sharedNodeStore.updateNode() to persist
   *
   * @param targetNodeId - Node ID to update
   * @param fieldName - Schema field name (e.g., 'status', 'due_date')
   * @param value - New value for the field
   */
  function updateSchemaField(targetNodeId: string, fieldName: string, value: unknown) {
    const targetNode = sharedNodeStore.getNode(targetNodeId);
    if (!targetNode) return;

    // Build nested namespace (properties[nodeType][fieldName])
    const typeNamespace = targetNode.properties?.[targetNode.nodeType];
    const isOldFormat = !typeNamespace || typeof typeNamespace !== 'object';

    let updatedNamespace: Record<string, unknown> = {};

    if (isOldFormat) {
      // Migrate from old flat format - copy ALL existing flat properties into namespace
      updatedNamespace = { ...targetNode.properties };
      // Remove internal fields that shouldn't be in namespace
      delete updatedNamespace._schema_version;
    } else {
      // Already in new format - copy namespace
      updatedNamespace = { ...(typeNamespace as Record<string, unknown>) };
    }

    // Apply the update
    updatedNamespace[fieldName] = value;

    // Build final properties with ONLY the nested namespace
    // CRITICAL: When migrating from old format, ALL flat properties are now in the namespace
    // So we start fresh with ONLY the nested structure, dropping all flat properties
    const updatedProperties = isOldFormat
      ? {
          // Old format: Start fresh with ONLY nested structure (drops ALL flat properties)
          [targetNode.nodeType]: updatedNamespace
        }
      : {
          // New format: Preserve existing properties structure
          ...targetNode.properties,
          [targetNode.nodeType]: updatedNamespace
        };

    // Persist via sharedNodeStore
    sharedNodeStore.updateNode(
      targetNodeId,
      { properties: updatedProperties },
      { type: 'viewer', viewerId: viewerId }
    );
  }

  // ============================================================================
  // Issue #653: ALL $effect blocks REMOVED from BaseNodeViewer
  // ============================================================================
  // Content persistence is now event-driven (no effects watching derived state):
  //
  // 1. Content changes: on:contentChanged → nodeManager.updateNodeContent()
  //    → sharedNodeStore.updateNode() → PersistenceCoordinator (debounced)
  //
  // 2. New nodes (placeholder promotion): on:contentChanged → sharedNodeStore.setNode()
  //    → PersistenceCoordinator (immediate for new nodes via isNewNode check)
  //
  // 3. Node deletions: handleCombineWithPrevious/handleDeleteNode
  //    → nodeManager.combineNodes() → sharedNodeStore.deleteNode()
  //
  // Benefits of eliminating effects:
  // - Simpler mental model: changes trigger persistence directly
  // - No derived state watching (anti-pattern for side effects)
  // - No need for lastSavedContent tracking (effects compared state changes)
  // - No isLoadingInitialNodes guard (effects needed to skip initial load)
  // ============================================================================

  /**
   * Helper function to promote a viewer-local placeholder to a real node
   * Extracts core Node properties without UI state (depth, children, expanded)
   *
   * @param placeholder - The viewer-local placeholder node
   * @param parentNodeId - The parent node ID
   * @param overrides - Content and/or nodeType to override from placeholder
   * @returns Promoted node with core properties only
   */
  function promotePlaceholderToNode(
    placeholder: Node,
    parentNodeId: string,
    overrides: { content?: string; nodeType?: string }
  ): Node & { parentId?: string } {
    // CRITICAL FIX (Issue #528): Include transient parentId field
    // This field is NOT part of the Node model (which stores hierarchy as graph edges)
    // but is needed by the backend HTTP API to create the parent-child edge relationship
    // The backend will extract this field and call operations.create_node() with it
    // Note (Issue #533): _containerId removed - backend auto-derives root from parent chain
    // Note (Issue #575): beforeSiblingId removed - backend now handles sibling ordering
    return {
      id: placeholder.id,
      nodeType: overrides.nodeType ?? placeholder.nodeType,
      content: overrides.content ?? placeholder.content,
      version: placeholder.version,
      createdAt: placeholder.createdAt,
      modifiedAt: new Date().toISOString(),
      properties: placeholder.properties,
      mentions: placeholder.mentions || [],
      // Transient field for backend edge creation (not persisted in Node model)
      parentId: parentNodeId // Root/container auto-derived from parent chain by backend
    };
  }

  async function loadChildrenForParent(nodeId: string, forceRefresh = false) {
    try {
      // OPTIMIZATION: loadChildrenTree fetches parent + children in ONE call
      // No need for separate getNode() call - eliminates redundant HTTP round-trip

      // Cache-first loading strategy: Check cache before hitting database (unless force refresh)
      let allNodes: Node[];

      if (!forceRefresh) {
        const cached = sharedNodeStore.getNodesForParent(nodeId);
        if (cached && cached.length > 0) {
          // Cache hit - use immediately (no database call!)
          allNodes = cached;
        } else {
          // Cache miss - fetch from database
          // Use loadChildrenTree which returns nested structure AND registers
          // parent-child edges in structureTree (critical for expand control visibility)
          // NOTE: This also loads the parent node internally (single HTTP call)
          allNodes = await sharedNodeStore.loadChildrenTree(nodeId);
        }
      } else {
        // Force refresh - bypass cache and fetch from database
        allNodes = await sharedNodeStore.loadChildrenTree(nodeId);
      }

      // Preload components for any node types not already cached (event-driven, no effects)
      const uniqueTypes = [...new Set(allNodes.map(n => n.nodeType))];
      for (const nodeType of uniqueTypes) {
        if (!(nodeType in loadedNodes)) {
          loadNodeComponent(nodeType);
        }
      }

      // Check if we have any nodes at all (reuse allNodes - no redundant cache check needed)
      if (allNodes.length === 0) {
        // No persisted children - create initial placeholder if needed
        // Note: We already checked cache/DB above, so if allNodes is empty, no persisted children exist

        // No children at all - placeholder will be created automatically by viewerPlaceholder derived
        // Issue #653: Removed manual ID creation - getOrCreatePlaceholderId() handles it lazily
        // Focus is handled by BaseNode's onMount when autoFocus=true
        // DON'T call initializeNodes() - keep placeholder completely viewer-local!
      } else {
        // Real children exist - initialize with ALL nodes
        // Issue #653: Removed lastSavedContent tracking - no longer needed without content watcher effect
        nodeManager.initializeNodes(allNodes, {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        });
      }

      // CRITICAL FIX: Register viewer with expansion coordinator AFTER nodes are loaded AND initialized
      // This ensures restoration can find the nodes instead of skipping them all
      // Must be inside try block (not finally) to ensure initializeNodes() has completed
      // Only register once per viewer instance (coordinator handles re-registration gracefully)
      NodeExpansionCoordinator.registerViewer(tabId, nodeManager);
    } catch (error) {
      console.error('[BaseNodeViewer] Failed to load children for parent:', nodeId, error);
    }
  }

  // Focus handling function with proper cursor positioning using tree walker
  function requestNodeFocus(nodeId: string, position: number) {
    // Use FocusManager as single source of truth for focus management
    // This replaces the old DOM-based focus approach
    focusManager.setEditingNode(nodeId, paneId, position);

    // Force textarea update to ensure merged content is visible immediately
    // Especially important for Safari which doesn't always reactive-update properly
    setTimeout(() => {
      const node = nodeManager.nodes.get(nodeId);
      if (node) {
        const textarea = document.querySelector(
          `textarea[id="textarea-${nodeId}"]`
        ) as HTMLTextAreaElement;
        if (textarea && textarea.value !== node.content) {
          textarea.value = node.content;
          textarea.selectionStart = position;
          textarea.selectionEnd = position;
        }
      }
    }, 10);
  }

  /**
   * Add appropriate formatting syntax to content based on node type
   * Used when creating new nodes from splits to preserve formatting
   *
   * NOTE: Header syntax inheritance is now handled in createNode() in reactiveNodeService.svelte.ts
   * to avoid duplication and ensure consistent behavior.
   */
  function addFormattingSyntax(content: string): string {
    // Header syntax inheritance is now handled in the createNode function
    // to ensure consistent behavior and avoid duplication

    // Return content as-is if no formatting needed
    if (!content) return content;

    // For task nodes: no automatic syntax addition
    // Task checkbox syntax ([ ]) is only added when users type it as a shortcut
    // Splitting a task node preserves the visual task state but not the syntax

    // For other node types, return as-is
    return content;
  }

  // Handle creating new nodes when Enter is pressed
  function handleCreateNewNode(
    event: CustomEvent<{
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      originalContent?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
      insertAtBeginning?: boolean;
      focusOriginalNode?: boolean;
      newNodeCursorPosition?: number;
    }>
  ) {
    const {
      afterNodeId,
      nodeType,
      currentContent,
      newContent,
      originalContent,
      inheritHeaderLevel,
      insertAtBeginning,
      focusOriginalNode,
      newNodeCursorPosition
    } = event.detail;

    // Validate node creation parameters
    if (!afterNodeId || !nodeType) {
      console.error('Invalid node creation parameters:', { afterNodeId, nodeType });
      return;
    }

    // CRITICAL FIX: Handle Enter key on viewer-local placeholder
    // The placeholder is not in nodeManager.nodes until promoted
    // If afterNodeId matches the placeholder, promote it first
    const currentPlaceholder = viewerPlaceholder;
    if (currentPlaceholder && afterNodeId === currentPlaceholder.id && nodeId) {
      console.log('[handleCreateNewNode] Promoting placeholder before creating new node:', afterNodeId);

      // Set promotion flag to prevent duplicate placeholder creation
      isPromoting = true;

      // Promote placeholder to real node (blank content is fine)
      const promotedNode = promotePlaceholderToNode(currentPlaceholder, nodeId, {
        content: currentContent ?? ''
      });

      // Add to shared store and persist immediately (not in-memory only)
      // Persist now so it exists in DB when creating the next node with insertAfterNodeId
      sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

      // Add to structure tree for immediate visibility
      reactiveStructureTree.addChild({
        parentId: nodeId,
        childId: promotedNode.id,
        order: Date.now()
      });

      // Clear promotion flag
      isPromoting = false;
    }

    // Verify the target node exists (should now exist after promotion)
    if (!nodeManager.nodes.has(afterNodeId)) {
      console.error('Target node does not exist:', afterNodeId);
      return;
    }

    // Update current node content if provided and actually changed
    if (currentContent !== undefined) {
      const existingNode = nodeManager.findNode(afterNodeId);
      if (existingNode && existingNode.content !== currentContent) {
        // Use updateNodeContent for node splitting - with new reactive architecture no forcing needed
        nodeManager.updateNodeContent(afterNodeId, currentContent);
      }
    }

    // Create new node using NodeManager - placeholder if empty, real if has content
    let newNodeId: string;

    // CRITICAL FIX: Use afterNode's actual parent from parentsCache
    // The viewer's nodeId represents the viewer's display context (e.g., date node)
    // but the actual parent is stored in sharedNodeStore.getParentsForNode()
    // After indent, the parent relationship is updated in the cache
    const parents = sharedNodeStore.getParentsForNode(afterNodeId);
    const explicitParentId = parents.length > 0 ? parents[0].id : (nodeId ?? null);

    // Add formatting syntax to the new content based on node type and header level
    // (applies to both empty and non-empty content for header inheritance)
    const formattedNewContent = addFormattingSyntax(newContent || '');

    // IMPORTANT: Enter key ALWAYS creates real persisted nodes (even if blank)
    // Only the first viewer-local placeholder uses the placeholder->promotion cycle
    // All subsequent nodes created via Enter are persisted immediately
    newNodeId = nodeManager.createNode(
      afterNodeId,
      formattedNewContent,
      nodeType,
      inheritHeaderLevel,
      insertAtBeginning || false,
      originalContent,
      !focusOriginalNode, // Focus new node when creating splits, original node when creating above
      paneId,
      false, // isInitialPlaceholder (Enter key never creates initial placeholders)
      explicitParentId // Pass viewer's nodeId as parent (e.g., date node for date viewers)
    );

    // Validate that node creation succeeded
    if (!newNodeId || !nodeManager.nodes.has(newNodeId)) {
      console.error('Node creation failed for afterNodeId:', afterNodeId, 'newNodeId:', newNodeId);
      return;
    }

    // Set cursor position using FocusManager (single source of truth)
    // Issue #664: For inherited type nodes (Enter key on typed node), use setEditingNodeFromInheritedType
    // which sets pattern state to 'inherited' (cannot revert to text).
    // This is different from pattern-detected type conversions which CAN revert.
    if (newNodeCursorPosition !== undefined && !focusOriginalNode) {
      if (nodeType !== 'text') {
        // Non-text inherited nodes: Use inherited-type signal (pattern state = 'inherited', cannot revert)
        focusManager.setEditingNodeFromInheritedType(newNodeId, newNodeCursorPosition, paneId);
      } else {
        // Text nodes: Use regular editing node
        focusManager.setEditingNode(newNodeId, paneId, newNodeCursorPosition);
      }
    }

    // Handle focus direction based on focusOriginalNode parameter
    if (focusOriginalNode) {
      // The hierarchy is correct (new node above, original below)
      // Use the nodeManager's update methods to properly trigger reactivity

      // Use updateNodeContent on original node to trigger focus
      const originalNode = nodeManager.nodes.get(afterNodeId);
      if (originalNode) {
        // Update the original node's content to itself, which should trigger focus
        nodeManager.updateNodeContent(afterNodeId, originalNode.content);
      }
    }

    // Handle HTML formatting conversion if needed
    if (newContent && newContent.includes('<span class="markdown-')) {
      setTimeout(() => {
        const markdownContent = htmlToMarkdown(newContent);
        nodeManager.updateNodeContent(newNodeId, markdownContent);
      }, 100);
    }
  }

  // Handle indenting nodes (Tab key)
  async function handleIndentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    try {
      // Validate node exists before indenting
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot indent non-existent node:', nodeId);
        return;
      }

      // Store cursor position before DOM changes
      const cursorPosition = saveCursorPosition(nodeId);

      // Use NodeManager to handle indentation
      const success = await nodeManager.indentNode(nodeId);

      if (success) {
        // NodeManager.indentNode() already persists via updateNode()
        // No need for separate saveHierarchyChange() call (was causing double-write)

        // Restore cursor position after DOM update
        setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
      }
    } catch (error) {
      console.error('Error during node indentation:', error);
    }
  }

  // Cursor position utilities
  function saveCursorPosition(nodeId: string): number {
    const element = document.getElementById(`contenteditable-${nodeId}`);
    if (!element) return 0;

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);
    const preCaretRange = range.cloneRange();
    preCaretRange.selectNodeContents(element);
    preCaretRange.setEnd(range.startContainer, range.startOffset);

    return preCaretRange.toString().length;
  }

  function restoreCursorPosition(nodeId: string, position: number) {
    const element = document.getElementById(`contenteditable-${nodeId}`);
    if (!element) return;

    try {
      const selection = window.getSelection();
      if (!selection) return;

      const range = document.createRange();
      const textNodes = getTextNodes(element);

      let currentOffset = 0;
      for (const textNode of textNodes) {
        const nodeLength = textNode.textContent?.length || 0;
        if (currentOffset + nodeLength >= position) {
          range.setStart(textNode, Math.max(0, position - currentOffset));
          range.collapse(true);
          selection.removeAllRanges();
          selection.addRange(range);
          element.focus();
          return;
        }
        currentOffset += nodeLength;
      }

      // If we couldn\'t find the exact position, place cursor at end
      if (textNodes.length > 0) {
        const lastNode = textNodes[textNodes.length - 1];
        range.setStart(lastNode, lastNode.textContent?.length || 0);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
        element.focus();
      }
    } catch {
      // Silently handle cursor restoration errors
    }
  }

  function getTextNodes(element: Element): Text[] {
    const textNodes: Text[] = [];
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);

    let node = walker.nextNode();
    while (node) {
      textNodes.push(node as Text);
      node = walker.nextNode();
    }

    return textNodes;
  }

  // Handle outdenting nodes (Shift+Tab key)
  async function handleOutdentNode(event: CustomEvent<{ nodeId: string }>) {
    const { nodeId } = event.detail;

    try {
      // Validate node exists before outdenting
      if (!nodeManager.nodes.has(nodeId)) {
        console.error('Cannot outdent non-existent node:', nodeId);
        return;
      }

      // Store cursor position before DOM changes
      const cursorPosition = saveCursorPosition(nodeId);

      // Use NodeManager to handle outdentation
      const success = await nodeManager.outdentNode(nodeId);

      if (success) {
        // NodeManager.outdentNode() already persists via updateNode()
        // No need for separate saveHierarchyChange() calls (was causing double-write)
        // Both the outdented node and transferred siblings are persisted automatically

        // Restore cursor position after DOM update
        setTimeout(() => restoreCursorPosition(nodeId, cursorPosition), 0);
      }
    } catch (error) {
      console.error('Error during node outdentation:', error);
    }
  }

  // Handle chevron click to toggle expand/collapse
  function handleToggleExpanded(toggleNodeId: string) {
    // Get the currently focused element before DOM changes
    const activeElement = document.activeElement as HTMLElement;
    const isTextEditor = activeElement && activeElement.id?.startsWith('contenteditable-');
    let focusedNodeId: string | null = null;
    let cursorPosition = 0;

    // Store cursor position if we have an active text editor
    if (isTextEditor) {
      focusedNodeId = activeElement.id.replace('contenteditable-', '');
      cursorPosition = saveCursorPosition(focusedNodeId);
    }

    // Toggle expanded state (viewer-local)
    const currentState = expandedState.get(toggleNodeId) ?? false;
    expandedState.set(toggleNodeId, !currentState);

    // TRANSITION PERIOD (Issue #580): Also update nodeManager for backward compatibility
    // This dual update ensures expansionState syncs during migration from nodeManager to reactive stores.
    // Once all state is in reactive stores, remove this call and update-only expandedState.
    nodeManager.toggleExpanded(toggleNodeId);

    // Restore focus and cursor position after DOM update
    if (focusedNodeId && isTextEditor) {
      setTimeout(() => {
        const element = document.getElementById(`contenteditable-${focusedNodeId}`);
        if (element && document.body.contains(element)) {
          restoreCursorPosition(focusedNodeId, cursorPosition);
        }
      }, 0);
    }
  }

  /**
   * Navigate to a target node using FocusManager (reactive approach)
   * Passes arrow navigation context to FocusManager for pixel-accurate positioning
   *
   * @param targetNodeId The node to navigate to
   * @param direction Navigation direction ('up' or 'down')
   * @param pixelOffset Horizontal pixel offset to maintain
   */
  function handleNavigateToNode(
    targetNodeId: string,
    direction: 'up' | 'down',
    pixelOffset: number
  ): void {
    // Find the target node
    const targetNode = nodeManager.findNode(targetNodeId);
    if (!targetNode) {
      console.warn(`[Navigation] Target node ${targetNodeId} not found`);
      return;
    }

    // Use FocusManager with arrow navigation context
    // This triggers reactive effects that handle:
    // 1. Switching from view mode to edit mode (isEditing derived value)
    // 2. Focusing the textarea (autoFocus effect in base-node.svelte)
    // 3. Calling controller.enterFromArrowNavigation() with pixel-accurate positioning
    focusManager.setEditingNodeFromArrowNavigation(targetNodeId, direction, pixelOffset, paneId);
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(
    event: CustomEvent<{
      nodeId: string;
      direction: 'up' | 'down';
      pixelOffset: number;
    }>
  ) {
    const { nodeId: eventNodeId, direction, pixelOffset } = event.detail;

    // Get visible nodes from reactive stores
    const currentVisibleNodes = visibleNodesFromStores;
    const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

    if (currentIndex === -1) return;

    // Find next navigable node that accepts navigation
    let targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;

    while (targetIndex >= 0 && targetIndex < currentVisibleNodes.length) {
      const candidateNode = currentVisibleNodes[targetIndex];

      // Check if this node accepts navigation (skip if it doesn't)
      // For now, assume all nodes accept navigation (will be refined per node type)
      const acceptsNavigation = true; // candidateNode.navigationMethods?.canAcceptNavigation() ?? true;

      if (acceptsNavigation) {
        // Navigate using reactive approach (FocusManager)
        handleNavigateToNode(candidateNode.id, direction, pixelOffset);
        return;
      }

      // This node doesn't accept navigation - try next one
      targetIndex = direction === 'up' ? targetIndex - 1 : targetIndex + 1;
    }
  }

  // Handle combining current node with previous node (Backspace at start of node)
  // CLEAN DELEGATION: All logic handled by NodeManager
  async function handleCombineWithPrevious(
    event: CustomEvent<{ nodeId: string; currentContent: string }>
  ) {
    try {
      const { nodeId: eventNodeId } = event.detail;

      // Validate node exists before combining
      if (!nodeManager.nodes.has(eventNodeId)) {
        console.error('Cannot combine non-existent node:', eventNodeId);
        return;
      }

      const currentVisibleNodes = visibleNodesFromStores;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

      if (currentIndex <= 0) {
        return; // No previous node to combine with
      }

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found or invalid:', previousNode?.id);
        return;
      }

      // Prevent merging into structured nodes (code-block, quote-block)
      // These nodes have specific formatting that can't accept arbitrary content
      if (isStructuredNode(previousNode.nodeType)) {
        return; // Silently prevent merge - user can still delete current node if empty
      }

      // Store the original content length before merge (this is where cursor should be positioned)
      const cursorPositionAfterMerge = previousNode.content.length;

      // Always use combineNodes (handles both empty and non-empty nodes with proper child promotion)
      nodeManager.combineNodes(eventNodeId, previousNode.id, paneId);

      // Always request focus at the merge point (end of original previous node content)
      // Use setTimeout to ensure DOM has updated after the merge operation
      // This ensures:
      // 1. Cursor is positioned at the merge point (not at beginning)
      // 2. Textarea updates to show merged content immediately (via forced update)
      // 3. Consistent behavior for both empty and non-empty node merges
      setTimeout(() => {
        requestNodeFocus(previousNode.id, cursorPositionAfterMerge);
      }, 0);
    } catch (error) {
      console.error('Error during node combination:', error);
    }
  }

  // Handle deleting empty node (Backspace at start of empty node)
  async function handleDeleteNode(event: CustomEvent<{ nodeId: string }>) {
    try {
      const { nodeId: eventNodeId } = event.detail;

      // Validate node exists before deletion
      if (!nodeManager.nodes.has(eventNodeId)) {
        console.error('Cannot delete non-existent node:', eventNodeId);
        return;
      }

      const currentVisibleNodes = visibleNodesFromStores;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

      if (currentIndex <= 0) return; // No previous node to focus

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        console.error('Previous node not found for focus after deletion:', previousNode?.id);
        // Can't combine without previous node - this shouldn't happen in normal usage
        return;
      }

      // Prevent merging into structured nodes (code-block, quote-block)
      // These nodes have specific formatting that can't accept arbitrary content
      if (isStructuredNode(previousNode.nodeType)) {
        // Block the action entirely - don't delete, don't merge, don't focus
        // User must manually delete the node (e.g., Cmd+Backspace) or add content first
        return;
      }

      // Use combineNodes even for empty nodes (handles child promotion properly)
      nodeManager.combineNodes(eventNodeId, previousNode.id, paneId);
      requestNodeFocus(previousNode.id, previousNode.content.length);
    } catch (error) {
      console.error('Error during node deletion:', error);
    }
  }

  // Handle icon click events
  // Note: Node-specific components handle their own icon behavior (e.g., TaskNode manages task states)
  // This handler is for any viewer-level icon click coordination if needed in the future
  function handleIconClick(
    _event: CustomEvent<{ nodeId: string; nodeType: string; currentState?: string }>
  ) {
    // Currently a no-op - individual node components handle their own icon clicks
    // This makes the system extensible for future node types that need viewer-level coordination
  }

  // Helper functions removed - NodeManager handles all node operations

  // Simple reactive access - let template handle reactivity directly

  // Dynamic component loading - all components loaded via plugin registry
  // Components are loaded on-demand and cached for subsequent renders
  // Proper Svelte 5 reactivity: use object instead of Map for reactive tracking
  let loadedNodes = $state<Record<string, unknown>>({});

  /**
   * Load a node component from the plugin registry if not already loaded
   * Components are cached in loadedNodes for subsequent renders
   */
  async function loadNodeComponent(nodeType: string): Promise<void> {
    // Skip if already loaded
    if (nodeType in loadedNodes) return;

    try {
      const component = await pluginRegistry.getNodeComponent(nodeType);
      if (component) {
        loadedNodes = { ...loadedNodes, [nodeType]: component };
      }
    } catch (error) {
      console.warn(`[BaseNodeViewer] Failed to load component for ${nodeType}:`, error);
    }
  }

  // Derive the list of nodes to render - either viewer placeholder or real children
  const nodesToRender = $derived(() => {
    const realChildren = visibleNodesFromStores;

    // If we have real children, render those
    if (realChildren.length > 0) {
      return realChildren;
    }

    // If we have a viewer placeholder, render it (with no parent, viewer-local only)
    if (viewerPlaceholder) {
      // Convert placeholder to renderable format with UI state
      return [
        {
          ...viewerPlaceholder,
          depth: 0,
          children: [],
          expanded: false,
          autoFocus: true,
          inheritHeaderLevel: 0,
          isPlaceholder: true
        }
      ];
    }

    // No children and no placeholder
    return [];
  });

  // CLEAN REACTIVE PATTERN: Compute placeholder state instead of coordinating it
  // Pure $derived - no manual effects or state coordination needed
  const shouldShowPlaceholder = $derived.by(() => {
    if (!nodeId) return false;
    if (isPromoting) return false; // Block during promotion to prevent race
    const realChildren = visibleNodesFromStores;
    return realChildren.length === 0;
  });

  // Placeholder creation is handled automatically by viewerPlaceholder $derived
  // No explicit effect needed - the derived value handles everything reactively

  // Calculate minimum depth for relative positioning
  // Children of a container node should start at depth 0 in the viewer
  const minDepth = $derived(() => {
    const nodes = nodesToRender();
    if (nodes.length === 0) return 0;
    return Math.min(...nodes.map((n) => n.depth || 0));
  });

  // Scroll position management now consolidated into main onMount above
  // This eliminates redundant onMount callbacks while preserving all functionality
  // The cleanup function returned from onMount handles scroll listener removal on unmount

  // Component loading: All components loaded dynamically via plugin registry
  // Loading is triggered in loadChildrenForParent when node data arrives (event-driven, no effects)
  // Components are cached in loadedNodes for subsequent renders

  // Clean up on component unmount and flush pending saves
  onDestroy(() => {
    // Unregister this viewer from the expansion coordinator
    NodeExpansionCoordinator.unregisterViewer(tabId);

    // CRITICAL: Commit ALL active batches globally BEFORE flushing
    // This ensures node type conversions (which use batches) are saved
    // We must commit globally because visible nodes might be empty if viewer already unmounted
    sharedNodeStore.commitAllBatches();

    // Note: PersistenceCoordinator removed in Issue #558
    // SimplePersistenceCoordinator handles debouncing inline in shared-node-store

    // Set cancellation flag to prevent stale writes
    isDestroyed = true;
  });
</script>

<!-- Base Node Viewer: Header + Scrollable Children Area -->
<div class="base-node-viewer">
  <!-- Header Section - Default editable header or custom snippet -->
  {#if header}
    <!-- Custom header provided via snippet (e.g., DateNodeViewer's date navigation) -->
    <div class="viewer-header">
      {@render header()}
    </div>
  {:else}
    <!-- Default editable header (no custom snippet provided) -->
    <div class="viewer-editable-header">
      <input
        type="text"
        class="header-input"
        value={headerContent}
        oninput={(e) => handleHeaderInput(e.currentTarget.value)}
        placeholder="Untitled"
        aria-label="Page title"
      />
    </div>
  {/if}

  <!-- Scrollable Node Content Area (children structure) -->
  <div class="node-content-area" bind:this={scrollContainer}>
    <!-- Schema-Driven Properties Panel - appears after header, before children -->
    {#if currentViewedNode && nodeId}
      <SchemaPropertyForm {nodeId} nodeType={currentViewedNode.nodeType} />
    {/if}

    {#each nodesToRender() as node (node.id)}
      {@const relativeDepth = (node.depth || 0) - minDepth()}
      <div
        class="node-container"
        data-has-children={node.children?.length > 0}
        style="margin-left: {relativeDepth * 2.5}rem"
      >
        <div class="node-content-wrapper">
          <!-- Chevron for parent nodes using design system approach -->
          {#if node.children && node.children.length > 0}
            <button
              class="chevron-icon"
              class:expanded={node.expanded}
              onclick={() => handleToggleExpanded(node.id)}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  handleToggleExpanded(node.id);
                }
              }}
              aria-label={node.expanded ? 'Collapse node' : 'Expand node'}
              aria-expanded={node.expanded}
            >
              <svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg">
                <path d="M6 3l5 5-5 5-1-1 4-4-4-4 1-1z" />
              </svg>
            </button>
          {/if}

          <!-- Node viewer with stable component references - all nodes use plugin registry -->
          {#if node.nodeType in loadedNodes}
            {#key `${node.id}-${node.nodeType}`}
              {@const NodeComponent = loadedNodes[node.nodeType] as typeof BaseNode}
              {@const nodeMetadata = extractNodeMetadata(node)}
              <NodeComponent
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={node.autoFocus}
                content={node.content}
                children={node.children}
                metadata={nodeMetadata}
                editableConfig={{ allowMultiline: true }}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e: CustomEvent<{ content: string; cursorPosition?: number }>) => {
                  const content = e.detail.content;
                  const cursorPosition = e.detail.cursorPosition ?? content.length;

                  // CRITICAL: Capture $derived viewerPlaceholder immediately to prevent race condition
                  const currentPlaceholder = viewerPlaceholder;

                  // Check if this is the viewer-local placeholder getting its first content
                  if (
                    currentPlaceholder &&
                    node.id === currentPlaceholder.id &&
                    content.trim() !== '' &&
                    nodeId
                  ) {
                    // ATOMIC PROMOTION: Set flag to block new placeholder creation
                    isPromoting = true;

                    // Promote placeholder to real node by assigning parent and adding to store
                    const promotedNode = promotePlaceholderToNode(currentPlaceholder, nodeId, {
                      content
                    });

                    // Set editing state BEFORE store update
                    // Use setEditingNodeFromTypeConversion to prevent blur handler from clearing state
                    focusManager.setEditingNodeFromTypeConversion(promotedNode.id, cursorPosition, paneId);

                    // Add to shared store with persistence enabled
                    // Setting false allows debounced content updates to persist to database
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

                    // CRITICAL: Add parent-child edge to reactiveStructureTree immediately
                    // This makes the promoted node visible in visibleNodesFromStores, which causes
                    // shouldShowPlaceholder to become false, switching the binding from placeholder to real child.
                    // Backend will also create the edge when persisting, and SSE will confirm (no-op since already added).
                    reactiveStructureTree.addChild({
                      parentId: nodeId,
                      childId: promotedNode.id,
                      order: Date.now()
                    });

                    // CRITICAL FIX: Clear viewerPlaceholder SYNCHRONOUSLY to prevent subsequent
                    // keystrokes from hitting the promotion path again while $effect is pending.
                    // Without this, rapid typing can cause multiple setNode() calls with stale content.

                    // Clear placeholder ID so fresh one is created if needed later
                    // Issue #653: Now using function instead of $state
                    resetPlaceholderId();

                    // Clear promotion flag after Svelte's microtask queue flushes.
                    // tick() ensures our synchronous state changes above have propagated
                    // through $derived computations before
                    // we allow new promotions. This is sufficient because the race condition
                    // was caused by stale state, not async operations.
                    tick().then(() => {
                      isPromoting = false;
                    });

                    // No need to reload - promoted node is already in shared store
                    // Database query will find it once persisted (CREATE is debounced)
                  } else {
                    // Regular node content update (placeholder flag is handled automatically)
                    nodeManager.updateNodeContent(node.id, content);
                  }
                  // Focus management handled by FocusManager (single source of truth)
                }}
                on:nodeTypeChanged={async (
                  e: CustomEvent<{
                    nodeType: string;
                    cleanedContent?: string;
                    cursorPosition?: number;
                  }>
                ) => {
                  const newNodeType = e.detail.nodeType;
                  const cleanedContent = e.detail.cleanedContent;
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;

                  // CRITICAL: Load component BEFORE updating node type
                  // This ensures the correct component is available when the template re-renders
                  if (!(newNodeType in loadedNodes)) {
                    await loadNodeComponent(newNodeType);
                  }

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  // Update content if cleanedContent is provided (e.g., from contentTemplate)
                  if (cleanedContent !== undefined) {
                    nodeManager.updateNodeContent(node.id, cleanedContent);
                  }

                  // Update node type through proper API (triggers component re-render)
                  nodeManager.updateNodeType(node.id, newNodeType);
                }}
                on:slashCommandSelected={async (
                  e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                ) => {
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;
                  const newNodeType = e.detail.nodeType;

                  // CRITICAL: Load component BEFORE updating node type
                  // This ensures the correct component is available when the template re-renders
                  if (!(newNodeType in loadedNodes)) {
                    await loadNodeComponent(newNodeType);
                  }

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  console.log('[BaseNodeViewer] slashCommandSelected:', {
                    nodeId: node.id,
                    newType: e.detail.nodeType,
                    isPlaceholder: node.isPlaceholder,
                    hasViewerPlaceholder: !!viewerPlaceholder
                  });

                  // CRITICAL FIX: Treat slash commands on placeholders as real node type changes
                  // They must persist to database, not just update locally
                  // Use same batching logic as real nodes to ensure atomic persistence
                  if (
                    node.isPlaceholder &&
                    nodeId &&
                    viewerPlaceholder &&
                    node.id === viewerPlaceholder.id
                  ) {
                    console.log(
                      '[BaseNodeViewer] Promoting placeholder to real node with type:',
                      e.detail.nodeType
                    );
                    // Promote placeholder to real node with the new type
                    const promotedNode = promotePlaceholderToNode(viewerPlaceholder, nodeId, {
                      content: node.content || '',
                      nodeType: e.detail.nodeType
                    });

                    // Add to store and trigger persistence
                    // Note: LIVE SELECT handles parent-child relationship via edge:created events
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

                    // CRITICAL: Add parent-child edge to reactiveStructureTree immediately
                    // This makes the promoted node visible in visibleNodesFromStores, which causes
                    // shouldShowPlaceholder to become false, switching the binding from placeholder to real child.
                    // Backend will also create the edge when persisting, and SSE will confirm (no-op since already added).
                    reactiveStructureTree.addChild({
                      parentId: nodeId,
                      childId: promotedNode.id,
                      order: Date.now()
                    });

                    // Clear placeholder ID so fresh one is created if needed later
                    resetPlaceholderId();
                  } else {
                    console.log('[BaseNodeViewer] Updating node type for real node');
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }
                }}
                on:iconClick={handleIconClick}
                on:taskStateChanged={(e) => {
                  const { nodeId: eventNodeId, state } = e.detail;
                  updateSchemaField(eventNodeId, 'status', mapNodeStateToSchemaStatus(state));
                }}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/key}
          {:else}
            <!-- Final fallback to BaseNode with key for re-rendering -->
            {#key `${node.id}-${node.nodeType}`}
              <BaseNode
                nodeId={node.id}
                nodeType={node.nodeType}
                autoFocus={node.autoFocus}
                content={node.content}
                children={node.children}
                metadata={node.properties || {}}
                editableConfig={{ allowMultiline: true }}
                on:createNewNode={handleCreateNewNode}
                on:indentNode={handleIndentNode}
                on:outdentNode={handleOutdentNode}
                on:navigateArrow={handleArrowNavigation}
                on:contentChanged={(e: CustomEvent<{ content: string }>) => {
                  const content = e.detail.content;

                  // Update node content (placeholder flag is handled automatically)
                  nodeManager.updateNodeContent(node.id, content);
                }}
                on:nodeTypeChanged={async (
                  e: CustomEvent<{
                    nodeType: string;
                    cleanedContent?: string;
                    cursorPosition?: number;
                  }>
                ) => {
                  const newNodeType = e.detail.nodeType;
                  const cleanedContent = e.detail.cleanedContent;
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;

                  // CRITICAL: Load component BEFORE updating node type
                  // This ensures the correct component is available when the template re-renders
                  if (!(newNodeType in loadedNodes)) {
                    await loadNodeComponent(newNodeType);
                  }

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  // Handle placeholder nodes - promote them to real nodes with the new type
                  if (
                    node.isPlaceholder &&
                    nodeId &&
                    viewerPlaceholder &&
                    node.id === viewerPlaceholder.id
                  ) {
                    // Promote placeholder to real node with the new type
                    const promotedNode = promotePlaceholderToNode(viewerPlaceholder, nodeId, {
                      content: cleanedContent ?? node.content ?? '',
                      nodeType: newNodeType
                    });

                    // Add to store and trigger persistence
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

                    // CRITICAL: Add parent-child edge to reactiveStructureTree immediately
                    // This makes the promoted node visible in visibleNodesFromStores, which causes
                    // shouldShowPlaceholder to become false, switching the binding from placeholder to real child.
                    reactiveStructureTree.addChild({
                      parentId: nodeId,
                      childId: promotedNode.id,
                      order: Date.now()
                    });

                    // CRITICAL: Clear viewerPlaceholder so template re-renders with promoted node
                    resetPlaceholderId();
                  } else {
                    // Update content if cleanedContent is provided (e.g., from contentTemplate)
                    if (cleanedContent !== undefined) {
                      nodeManager.updateNodeContent(node.id, cleanedContent);
                    }

                    // Update node type through proper API (triggers component re-render)
                    nodeManager.updateNodeType(node.id, newNodeType);
                  }
                }}
                on:slashCommandSelected={async (
                  e: CustomEvent<{ command: string; nodeType: string; cursorPosition?: number }>
                ) => {
                  // Use cursor position from event (captured by TextareaController)
                  const cursorPosition = e.detail.cursorPosition ?? 0;
                  const newNodeType = e.detail.nodeType;

                  // CRITICAL: Load component BEFORE updating node type
                  // This ensures the correct component is available when the template re-renders
                  if (!(newNodeType in loadedNodes)) {
                    await loadNodeComponent(newNodeType);
                  }

                  // CRITICAL: Set editing state BEFORE updating node type
                  // This ensures focus manager state is ready when the new component mounts
                  focusManager.setEditingNodeFromTypeConversion(node.id, cursorPosition, paneId);

                  console.log('[BaseNodeViewer] slashCommandSelected:', {
                    nodeId: node.id,
                    newType: e.detail.nodeType,
                    isPlaceholder: node.isPlaceholder,
                    hasViewerPlaceholder: !!viewerPlaceholder
                  });

                  // CRITICAL FIX: Treat slash commands on placeholders as real node type changes
                  // They must persist to database, not just update locally
                  // Use same batching logic as real nodes to ensure atomic persistence
                  if (
                    node.isPlaceholder &&
                    nodeId &&
                    viewerPlaceholder &&
                    node.id === viewerPlaceholder.id
                  ) {
                    console.log(
                      '[BaseNodeViewer] Promoting placeholder to real node with type:',
                      e.detail.nodeType
                    );
                    // Promote placeholder to real node with the new type
                    const promotedNode = promotePlaceholderToNode(viewerPlaceholder, nodeId, {
                      content: node.content || '',
                      nodeType: e.detail.nodeType
                    });

                    // Add to store and trigger persistence
                    // Note: LIVE SELECT handles parent-child relationship via edge:created events
                    sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

                    // CRITICAL: Add parent-child edge to reactiveStructureTree immediately
                    // This makes the promoted node visible in visibleNodesFromStores, which causes
                    // shouldShowPlaceholder to become false, switching the binding from placeholder to real child.
                    // Backend will also create the edge when persisting, and SSE will confirm (no-op since already added).
                    reactiveStructureTree.addChild({
                      parentId: nodeId,
                      childId: promotedNode.id,
                      order: Date.now()
                    });

                    // Clear placeholder ID so fresh one is created if needed later
                    resetPlaceholderId();
                  } else {
                    console.log('[BaseNodeViewer] Updating node type for real node');
                    // For real nodes, update node type with full persistence
                    nodeManager.updateNodeType(node.id, e.detail.nodeType);
                  }
                }}
                on:iconClick={handleIconClick}
                on:taskStateChanged={(e) => {
                  const { nodeId: eventNodeId, state } = e.detail;
                  updateSchemaField(eventNodeId, 'status', mapNodeStateToSchemaStatus(state));
                }}
                on:combineWithPrevious={handleCombineWithPrevious}
                on:deleteNode={handleDeleteNode}
              />
            {/key}
          {/if}
        </div>
      </div>
    {/each}

    <!-- Backlinks Panel - fixed at bottom of this viewer -->
    {#if nodeId}
      <BacklinksPanel {nodeId} />
    {/if}
  </div>
</div>

<!-- Template structure fixed -->

<style>
  /* Base container - full height layout */
  .base-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
  }

  /* Default editable header section - borderless design */
  .viewer-editable-header {
    flex-shrink: 0;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  .header-input {
    width: 100%;
    font-size: 2rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    background: transparent;
    border: none;
    outline: none;
    padding: 0;
    margin: 0;
    font-family: inherit;
  }

  .header-input::placeholder {
    color: hsl(var(--muted-foreground) / 0.5);
  }

  /* Custom header section - fixed at top, doesn't scroll */
  .viewer-header {
    flex-shrink: 0;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  /* Default header content styling - large, prominent titles */
  .viewer-header :global(h1) {
    font-size: 2rem;
    font-weight: 500;
    color: hsl(var(--muted-foreground));
    margin: 0;
  }

  /* Scrollable node content area for children structure */
  .node-content-area {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    min-height: 0; /* Critical for flex scrolling */
    padding: var(--viewer-padding-vertical) var(--viewer-padding-horizontal);
    padding-bottom: var(
      --viewer-padding-bottom
    ); /* Reduced padding - backlinks panel is now sticky within this container */
    display: flex;
    flex-direction: column;
    gap: 0; /* 0px gap - all spacing from node padding for 8px total */

    /* Autohide scrollbar - only show when scrolling */
    scrollbar-width: thin; /* Firefox */
    scrollbar-color: transparent transparent; /* Firefox - hidden by default */
  }

  /* Show scrollbar on hover or while scrolling */
  .node-content-area:hover,
  .node-content-area:active {
    scrollbar-color: hsl(var(--muted-foreground) / 0.3) transparent; /* Firefox */
  }

  /* WebKit (Chrome, Safari, Edge) scrollbar styling */
  .node-content-area::-webkit-scrollbar {
    width: 8px;
  }

  .node-content-area::-webkit-scrollbar-track {
    background: transparent;
  }

  .node-content-area::-webkit-scrollbar-thumb {
    background: transparent;
    border-radius: 4px;
  }

  /* Show scrollbar on hover or while scrolling */
  .node-content-area:hover::-webkit-scrollbar-thumb,
  .node-content-area:active::-webkit-scrollbar-thumb {
    background: hsl(var(--muted-foreground) / 0.3);
  }

  .node-content-area::-webkit-scrollbar-thumb:hover {
    background: hsl(var(--muted-foreground) / 0.5);
  }

  .base-node-viewer {
    /* Dynamic Circle Positioning System - All values configurable from here */
    --circle-offset: 22px; /* Circle center distance from container left edge - reserves space for chevrons */
    --circle-diameter: 20px; /* Circle size (width and height) */
    --circle-text-gap: 8px; /* Gap between circle edge and text content */
    --node-indent: 2.5rem; /* Indentation distance between parent and child levels */

    /* Default font values for positioning calculations */
    --font-size: 1rem;
    --line-height: 1.6;
    /* Note: --icon-vertical-position is defined globally in app.css */

    /* NodeSpace Extension Colors - Subtle Tint System (Scheme 3) */
    --node-text: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-task: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-ai-chat: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-entity: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
    --node-query: 200 40% 45%; /* Blue-gray for all nodes (Scheme 3) */
  }

  .node-container {
    /* Individual node wrapper - no additional spacing */
    /* Allow chevrons to extend outside container bounds */
    overflow: visible;
  }

  .node-content-wrapper {
    /* Wrapper for chevron + content */
    display: flex;
    align-items: flex-start;
    gap: 0.25rem; /* 4px gap between chevron/spacer and text content */
    position: relative; /* Enable absolute positioning for chevrons */
    width: 100%; /* Ensure wrapper fills parent so flex children can inherit */
  }

  /* Flex children (node wrappers) should fill available space */
  /* Use :global() to apply across component boundaries (TaskNode, CodeBlock, etc. have different scopes) */
  .node-content-wrapper > :global(:not(.chevron-icon):not(.chevron-spacer)) {
    flex: 1;
    min-width: 0; /* Allow flex item to shrink below content size if needed */
  }

  /* 
    Chevron positioning system - matches circle positioning exactly
    
    POSITIONING FORMULA:
    The chevron must be vertically centered with the circles, which use:
    top: calc(0.25rem + (var(--font-size) * var(--line-height) / 2))
    
    Where:
    - 0.25rem = container top padding (.node has padding: 0.25rem)
    - line-height-px = font-size × line-height multiplier
    - This formula positions at the visual center of the first line of text
    
    HORIZONTAL POSITION:
    - Exactly halfway between parent and child circles
    - Parent is at current depth, child is at depth + 2.5rem (--node-indent)
    - Chevron positioned at -1.25rem (half of 2.5rem) from current node
    
    INHERITANCE:
    The --line-height-px variable is inherited from .node-content-wrapper
    which detects the header level of nested content using :has() selector
  */
  .chevron-icon {
    opacity: 0; /* Hidden by default - shows on hover */
    background: none;
    border: none;
    padding: 0.125rem; /* 2px padding for clickable area */
    cursor: pointer;
    border-radius: 0.125rem; /* 2px border radius */
    transition: opacity 0.15s ease-in-out; /* Smooth fade in/out */
    pointer-events: auto; /* Ensure chevron always receives pointer events */
    flex-shrink: 0;
    width: 1.25rem; /* Fixed 20px to match circle size */
    height: 1.25rem; /* Fixed 20px to match circle size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* Position chevron exactly halfway between parent and child circles */
    position: absolute;
    left: calc(
      -1 * var(--node-indent) / 2 + var(--circle-offset)
    ); /* Halfway back to parent + parent circle offset */
    /* Use shared CSS variable from .node - single source of truth for vertical positioning */
    top: var(--icon-vertical-position);
    transform: translate(-50%, -50%); /* Center icon on coordinates, same as circles */
    z-index: 999; /* Very high z-index to ensure clickability over all other elements */
  }

  .chevron-icon svg {
    width: 16px;
    height: 16px;
    fill: hsl(var(--node-text) / 0.5);
    transition: fill 0.15s ease;
  }

  .chevron-icon:focus {
    outline: 2px solid hsl(var(--ring));
    outline-offset: 2px;
    opacity: 1; /* Always visible when focused */
  }

  .chevron-icon:hover svg {
    fill: hsl(var(--node-text) / 0.5);
  }

  /* Show chevron only when hovering directly over this node's content wrapper (not child nodes) */
  .node-content-wrapper:hover > .chevron-icon {
    opacity: 1;
  }

  /* Expanded state: rotate 90 degrees to point down */
  .chevron-icon.expanded {
    transform: translate(-50%, -50%) rotate(90deg);
  }

  /* CSS-first positioning to match base-node.svelte implementation */
  .node-content-wrapper {
    /* Default values for normal text - adjusted for better circle alignment */
    --line-height: 1.875;
    --font-size: 1rem;
  }

  /* Inherit font-size, line-height, and icon positioning from HeaderNode wrapper classes (Issue #311) */
  .node-content-wrapper:has(:global(.header-h1)) {
    --font-size: 2rem;
    --line-height: 1.2;
    --icon-vertical-position: calc(0.25rem + (2rem * 1.2 / 2));
  }

  .node-content-wrapper:has(:global(.header-h2)) {
    --font-size: 1.5rem;
    --line-height: 1.3;
    --icon-vertical-position: calc(0.25rem + (1.5rem * 1.3 / 2));
  }

  .node-content-wrapper:has(:global(.header-h3)) {
    --font-size: 1.25rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.25rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h4)) {
    --font-size: 1.125rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1.125rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h5)) {
    --font-size: 1rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (1rem * 1.4 / 2));
  }

  .node-content-wrapper:has(:global(.header-h6)) {
    --font-size: 0.875rem;
    --line-height: 1.4;
    --icon-vertical-position: calc(0.25rem + (0.875rem * 1.4 / 2));
  }

  /* Reset ordered list counter at viewer level */
  .base-node-viewer {
    counter-reset: ordered-list-counter;
  }

  /* Also reset when ordered list sequence is broken by non-list nodes */
  /* Using data attribute for semantic clarity and maintainability */
  .base-node-viewer > *:not([data-node-type='ordered-list']) {
    counter-reset: ordered-list-counter;
  }
</style>
