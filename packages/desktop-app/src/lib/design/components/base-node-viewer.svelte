<!--
  BaseNodeViewer - Container that manages a collection of nodes
  Handles node creation, deletion, and organization

  Now uses NodeServiceContext to provide @ autocomplete functionality
  to all TextNode components automatically via proper inheritance.
-->

<script lang="ts">
  import { onMount, onDestroy, getContext, tick } from 'svelte';
  import { htmlToMarkdown } from '$lib/utils/markdown.js';
  import BacklinksPanel from '$lib/design/components/backlinks-panel.svelte';
  import GenericSchemaForm from '$lib/components/schema/generic-schema-form.svelte';
  import NodeRow from '$lib/design/components/node-row.svelte';
  import { createLogger } from '$lib/utils/logger';

  // Logger instance for BaseNodeViewer component
  const log = createLogger('BaseNodeViewer');
  // Plugin registry provides all node components dynamically
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import type { SchemaFormComponent } from '$lib/plugins/types';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { buildCrossNodeCopy } from '$lib/utils/cross-node-copy';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';
  import { structureTree as reactiveStructureTree } from '$lib/stores/reactive-structure-tree.svelte';
  import type { Node } from '$lib/types';
  import type { Snippet } from 'svelte';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';
  import { getViewerId, saveScrollPosition, getScrollPosition } from '$lib/stores/scroll-state';
  import { onDaemonReconnect } from '$lib/services/daemon-status';
  import { NodeComponentLoader } from '$lib/design/components/node-component-loader.svelte';
  import { SchemaFormLoader } from '$lib/design/components/schema-form-loader.svelte';
  import {
    computeHeaderDisplayValue,
    rendersAsEntityRow
  } from '$lib/design/components/node-type-predicates';
  import { updateSchemaField } from '$lib/design/components/schema-field-update';
  import { normalizeCodeBlockContent } from '$lib/design/components/fallback-node-render';
  import {
    saveCursorPosition,
    restoreCursorPosition
  } from '$lib/design/components/viewer-cursor-utils';
  import type {
    ViewerRenderNode,
    ContentChangedDetail,
    NodeTypeChangedDetail,
    SlashCommandSelectedDetail,
    CreateNewNodeDetail,
    NavigateArrowDetail,
    TaskStateChangedDetail,
    CombineWithPreviousDetail,
    DeleteNodeDetail
  } from '$lib/design/components/node-row-types';

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
    onNodeNotFound
  }: {
    header?: Snippet;
    nodeId?: string | null;
    tabId?: string;
    onNodeIdChange?: (_nodeId: string) => void; // In type for interface, not used by BaseNodeViewer
    onNodeNotFound?: () => void;
  } = $props();

  // Get nodeManager from shared context
  const services = getNodeServices();
  if (!services) {
    throw new Error(
      'NodeServices not available. Make sure base-node-viewer is wrapped in NodeServiceContext.'
    );
  }

  const nodeManager = services.nodeManager;

  // Lazy component + schema-form loaders (reactive $state lives in these instances)
  const nodeLoader = new NodeComponentLoader();
  const schemaFormLoader = new SchemaFormLoader();

  // Cancellation flag to prevent database writes after component unmounts
  let isDestroyed = false;

  // Placeholder promotion flag - blocks new placeholder creation during async promotion window
  // Prevents race condition where promotion triggers reactive effects before updates complete
  let isPromoting = $state(false);

  // Stable placeholder ID - cached outside reactive system to avoid mutations during derived evaluation
  // Eliminated $effect by using a non-reactive cache variable
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
  // Now uses lazy ID generation instead of $effect-managed state
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
  // sharedNodeStore.nodes is a SvelteMap, so getNode() tracks at per-node granularity.
  const currentViewedNode = $derived.by(() => {
    return nodeId ? sharedNodeStore.getNode(nodeId) : null;
  });

  // Scroll position tracking
  // Reference to the scroll container element
  let scrollContainer: HTMLElement | null = null;
  // Generate unique viewer ID for this viewer instance
  // Use IIFE to capture props at initialization and avoid Svelte state_referenced_locally warning
  // ViewerId is computed once at component creation - this is intentional one-time capture
  // Includes nodeId to ensure each node has its own scroll position, even when
  // navigating between nodes in the same tab
  const viewerId = (() => getViewerId(nodeId ?? 'default', tabId, paneId))();

  // Track auto-focus nodes (viewer-local UI state)
  // Use $state for reactive Set mutations
  let autoFocusNodes = $state(new Set<string>());

  // Track if the viewer header input is being edited (for view/edit mode display)
  let isHeaderBeingEdited = $state(false);

  /**
   * Visible nodes derived from ReactiveStructureTree + SharedNodeStore
   */
  const visibleNodesFromStores = $derived.by<ViewerRenderNode[]>(() => {
    if (!nodeId) return [];
    // Reactive dependency on structure tree is automatic with $state.raw()
    // Svelte will re-run this derived when reactiveStructureTree.children changes.
    // Node content/property reads below go through sharedNodeStore's SvelteMap,
    // which tracks per-node so this also re-runs on relevant node changes.

    // Helper function to recursively flatten visible nodes with depth
    function flattenNodes(
      parentId: string,
      depth: number,
      result: ViewerRenderNode[] = []
    ): ViewerRenderNode[] {
      // TRANSITION PERIOD: Try reactive structure tree first, fall back to sharedNodeStore
      // This supports gradual migration where reactive stores are being populated asynchronously.
      // Once all nodes are in reactive stores, remove fallback and use only reactiveStructureTree.
      let childIds = reactiveStructureTree.getChildren(parentId);
      if (childIds.length === 0) {
        const cachedNodes = sharedNodeStore.getNodesForParent(parentId);
        if (cachedNodes && cachedNodes.length > 0) {
          childIds = cachedNodes.map((n) => n.id);
        }
      }

      for (const id of childIds) {
        // sharedNodeStore is now single source of truth (consolidated from nodeData)
        const node = sharedNodeStore.getNode(id);
        if (!node) continue;

        // Get children IDs for this node
        // TRANSITION PERIOD: Same fallback pattern as above
        let children = reactiveStructureTree.getChildren(node.id);
        if (children.length === 0) {
          const cachedChildren = sharedNodeStore.getNodesForParent(node.id);
          if (cachedChildren) {
            children = cachedChildren.map((c) => c.id);
          }
        }

        // Build node with UI state
        // Get UI state from ReactiveNodeService (has expanded: true by default)
        const uiState = nodeManager.getUIState(node.id);
        const nodeWithUI: ViewerRenderNode = {
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
    // Restore previously loaded node components from the registry's persistent cache.
    nodeLoader.seedFromRegistry();

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

      // Cross-node copy: a text selection spanning more than one node
      // copies the underlying source markdown (clipped to the selection, nesting
      // preserved as indentation) instead of the browser default (stripped,
      // boundary-less rendered text). Single-node selections fall through to
      // native copy. The `copy` event bubbles to this container from the node
      // that holds the selection.
      const container = scrollContainer;
      const handleCopy = (event: ClipboardEvent) => {
        const depths = new Map(visibleNodesFromStores.map((n) => [n.id, n.depth]));
        const markdown = buildCrossNodeCopy({
          selection: typeof window !== 'undefined' ? window.getSelection() : null,
          root: container,
          resolveNode: (id) => {
            const node = sharedNodeStore.getNode(id);
            if (!node) return null;
            return { content: node.content, depth: depths.get(id) ?? 0 };
          }
        });
        if (markdown === null) return; // leave native copy behavior untouched
        event.preventDefault();
        event.clipboardData?.setData('text/plain', markdown);
      };

      scrollContainer.addEventListener('copy', handleCopy);

      // Store cleanup function
      scrollCleanup = () => {
        if (scrollContainer) {
          scrollContainer.removeEventListener('scroll', handleScroll);
          scrollContainer.removeEventListener('copy', handleCopy);
        }
      };
    }

    if (!nodeId) {
      // currentViewedNode is now $derived - no manual assignment needed
      return scrollCleanup || undefined;
    }

    // Capture into a const so the closures below retain the narrowed
    // (non-null) type even though `nodeId` is a mutable prop.
    const currentNodeId = nodeId;
    let loadFailed = false;

    async function loadAndSettle(forceRefresh = false) {
      try {
        // Load children asynchronously
        // Note: loadChildrenForParent has internal cache checking, so this is efficient
        await loadChildrenForParent(currentNodeId, forceRefresh);
        loadFailed = false;

        // CRITICAL: Prevent state updates after component destruction
        if (isDestroyed) {
          return;
        }

        // No longer need viewedNodeCache workaround
        // sharedNodeStore.nodes is now $state, so currentViewedNode $derived updates automatically
        // Header content derived from currentViewedNode - no manual assignment needed

        // Check if node exists after loading
        const node = sharedNodeStore.getNode(currentNodeId);
        if (!node) {
          log.warn(`Node ${currentNodeId} not found — closing stale tab`);
          onNodeNotFound?.();
          return;
        }

        // Preload type-specific schema form for viewed node if available
        // This triggers lazy loading of TaskSchemaForm, PersonSchemaForm, etc.
        if (node.nodeType) {
          // Reset generic schema when navigating to a different node
          schemaFormLoader.resetGenericSchema();
          schemaFormLoader.loadForm(node.nodeType);
        }

        // Tab title is derived directly from node data by tab-system.svelte's
        // computeTabTitle — no push needed here.
      } catch (error) {
        loadFailed = true;
        log.error('Failed to load children:', error);
      }
    }

    // Load children asynchronously (non-blocking)
    loadAndSettle();

    // If the initial load failed (e.g. daemon still starting up), retry once
    // the daemon reconnects instead of leaving this viewer permanently empty.
    const unsubscribeReconnect = onDaemonReconnect(() => {
      if (isDestroyed || !loadFailed) return;
      loadAndSettle(true);
    });

    // Return cleanup function
    return () => {
      unsubscribeReconnect();
      scrollCleanup?.();
    };
  });

  /** Header value shown in view mode (not focused) — see computeHeaderDisplayValue. */
  let headerDisplayValue = $derived(
    computeHeaderDisplayValue(currentViewedNode, schemaFormLoader.hasTitleTemplate)
  );

  /**
   * Handle header content changes (for default editable header).
   * Persists to database; the tab title updates automatically since tab-system.svelte
   * derives it from sharedNodeStore, which nodeManager.updateNodeContent writes to
   * synchronously (titles are computed, never pushed).
   */
  function handleHeaderInput(newValue: string) {
    if (nodeId) {
      try {
        nodeManager.updateNodeContent(nodeId, newValue);
      } catch (error) {
        log.error('Failed to update header content:', error);
        // TODO: Show user-facing error notification via toast/notification system
      }
    }
  }

  // BaseNodeViewer has no $effect blocks watching derived state. Persistence is
  // event-driven instead:
  //
  // 1. Content changes: on:contentChanged → nodeManager.updateNodeContent()
  //    → sharedNodeStore.updateNode() → PersistenceCoordinator (debounced)
  //
  // 2. New nodes (placeholder promotion): on:contentChanged → sharedNodeStore.setNode()
  //    → PersistenceCoordinator (immediate for new nodes via isNewNode check)
  //
  // 3. Node deletions: handleCombineWithPrevious/handleDeleteNode
  //    → nodeManager.combineNodes() → sharedNodeStore.deleteNode()

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
  ): Node {
    // parentId is derived from structureTree at CREATE time (persistence path calls getParentId).
    // IMPORTANT: Caller MUST call reactiveStructureTree.addChild({ parentId: parentNodeId, ... })
    // BEFORE the setNode persistence debounce fires so getParentId returns the correct parent.
    log.debug(
      `[promotePlaceholder] promoting ${placeholder.id.substring(0, 8)} under parent ${parentNodeId.substring(0, 8)}`
    );
    return {
      id: placeholder.id,
      nodeType: overrides.nodeType ?? placeholder.nodeType,
      content: overrides.content ?? placeholder.content,
      version: placeholder.version,
      createdAt: placeholder.createdAt,
      modifiedAt: new Date().toISOString(),
      properties: placeholder.properties,
      mentions: placeholder.mentions || []
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
      const uniqueTypes = [...new Set(allNodes.map((n) => n.nodeType))];
      for (const nodeType of uniqueTypes) {
        if (!nodeLoader.has(nodeType)) {
          nodeLoader.load(nodeType);
        }
      }

      // Check if we have any nodes at all (reuse allNodes - no redundant cache check needed)
      if (allNodes.length === 0) {
        // No persisted children - create initial placeholder if needed
        // Note: We already checked cache/DB above, so if allNodes is empty, no persisted children exist
        // No children at all - placeholder will be created automatically by viewerPlaceholder derived
        // Removed manual ID creation - getOrCreatePlaceholderId() handles it lazily
        // Focus is handled by BaseNode's onMount when autoFocus=true
        // DON'T call initializeNodes() - keep placeholder completely viewer-local!
      } else {
        // Real children exist - initialize with ALL nodes
        // Removed lastSavedContent tracking - no longer needed without content watcher effect
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
      log.error(`Failed to load children for parent: ${nodeId}`, error);
    }
  }

  // Focus handling function with proper cursor positioning using tree walker
  function requestNodeFocus(nodeId: string, position: number) {
    // Use FocusManager as single source of truth for focus management
    // This replaces the old DOM-based focus approach
    focusManager.focusNodeAtPosition(nodeId, position, paneId);

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
  function handleCreateNewNode(detail: CreateNewNodeDetail) {
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
    } = detail;

    // Validate node creation parameters
    if (!afterNodeId || !nodeType) {
      log.error('Invalid node creation parameters:', { afterNodeId, nodeType });
      return;
    }

    // CRITICAL FIX: Handle Enter key on viewer-local placeholder
    // The placeholder is not in nodeManager.nodes until promoted
    // If afterNodeId matches the placeholder, promote it first
    const currentPlaceholder = viewerPlaceholder;
    if (currentPlaceholder && afterNodeId === currentPlaceholder.id && nodeId) {
      log.debug('Promoting placeholder before creating new node:', afterNodeId);

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
      log.error('Target node does not exist:', afterNodeId);
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
      log.error(`Node creation failed for afterNodeId: ${afterNodeId}, newNodeId: ${newNodeId}`);
      return;
    }

    // Set cursor position using FocusManager (single source of truth)
    // For inherited type nodes (Enter key on typed node), use focusNodeFromInheritedType
    // which sets pattern state to 'inherited' (cannot revert to text).
    // This is different from pattern-detected type conversions which CAN revert.
    if (newNodeCursorPosition !== undefined && !focusOriginalNode) {
      if (nodeType !== 'text') {
        // Non-text inherited nodes: Use inherited-type signal (pattern state = 'inherited', cannot revert)
        focusManager.focusNodeFromInheritedType(newNodeId, newNodeCursorPosition, paneId);
      } else {
        // Text nodes: Use regular editing node
        focusManager.focusNodeAtPosition(newNodeId, newNodeCursorPosition, paneId);
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
  async function handleIndentNode(detail: { nodeId: string }) {
    const { nodeId } = detail;

    try {
      // Validate node exists before indenting
      if (!nodeManager.nodes.has(nodeId)) {
        log.error('Cannot indent non-existent node:', nodeId);
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
      log.error('Error during node indentation:', error);
    }
  }

  // Handle outdenting nodes (Shift+Tab key)
  async function handleOutdentNode(detail: { nodeId: string }) {
    const { nodeId } = detail;

    try {
      // Validate node exists before outdenting
      if (!nodeManager.nodes.has(nodeId)) {
        log.error('Cannot outdent non-existent node:', nodeId);
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
      log.error('Error during node outdentation:', error);
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

    // Toggle expanded state via nodeManager
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
      log.warn(`Target node ${targetNodeId} not found`);
      return;
    }

    // Use FocusManager with arrow navigation context
    // This triggers reactive effects that handle:
    // 1. Switching from view mode to edit mode (isEditing derived value)
    // 2. Focusing the textarea (autoFocus effect in base-node.svelte)
    // 3. Calling controller.enterFromArrowNavigation() with pixel-accurate positioning
    focusManager.focusNodeFromArrowNav(targetNodeId, direction, pixelOffset, paneId);
  }

  // Handle arrow key navigation between nodes using entry/exit methods
  function handleArrowNavigation(detail: NavigateArrowDetail) {
    const { nodeId: eventNodeId, direction, pixelOffset } = detail;

    // Get visible nodes from reactive stores
    const currentVisibleNodes = visibleNodesFromStores;
    const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

    if (currentIndex === -1) return;

    // Find next navigable node that accepts navigation
    let targetIndex = direction === 'up' ? currentIndex - 1 : currentIndex + 1;

    while (targetIndex >= 0 && targetIndex < currentVisibleNodes.length) {
      const candidateNode = currentVisibleNodes[targetIndex];

      // Check if this node accepts navigation (skip if it doesn't).
      // Entity rows (no inline node component, rendered read-only via the BaseNode
      // fallback) have nothing to put a caret into — skip them so arrow navigation
      // passes through to the next editable node.
      const acceptsNavigation = !rendersAsEntityRow(candidateNode.nodeType);

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
  async function handleCombineWithPrevious(detail: CombineWithPreviousDetail) {
    try {
      const { nodeId: eventNodeId } = detail;

      // Validate node exists before combining
      if (!nodeManager.nodes.has(eventNodeId)) {
        log.error('Cannot combine non-existent node:', eventNodeId);
        return;
      }

      // Protect node types whose data lives outside `.content` (e.g. ai-chat,
      // whose conversation is in properties.messages) from being merged away by
      // a start-of-node Backspace. Guards the current/event node's own type.
      const eventNode = nodeManager.nodes.get(eventNodeId);
      if (eventNode && !pluginRegistry.deletableViaBackspace(eventNode.nodeType)) {
        return;
      }

      const currentVisibleNodes = visibleNodesFromStores;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

      if (currentIndex <= 0) {
        return; // No previous node to combine with
      }

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        log.error('Previous node not found or invalid:', previousNode?.id);
        return;
      }

      // Prevent merging into structured nodes (code-block, quote-block)
      // These nodes have specific formatting that can't accept arbitrary content
      if (!pluginRegistry.acceptsContentMerge(previousNode.nodeType)) {
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
      log.error('Error during node combination:', error);
    }
  }

  // Handle deleting empty node (Backspace at start of empty node)
  async function handleDeleteNode(detail: DeleteNodeDetail) {
    try {
      const { nodeId: eventNodeId } = detail;

      // Validate node exists before deletion
      if (!nodeManager.nodes.has(eventNodeId)) {
        log.error('Cannot delete non-existent node:', eventNodeId);
        return;
      }

      // Protect node types whose data lives outside `.content` (e.g. ai-chat,
      // whose conversation is in properties.messages) from being deleted away by
      // a start-of-node Backspace. Guards the current/event node's own type.
      const eventNode = nodeManager.nodes.get(eventNodeId);
      if (eventNode && !pluginRegistry.deletableViaBackspace(eventNode.nodeType)) {
        return;
      }

      const currentVisibleNodes = visibleNodesFromStores;
      const currentIndex = currentVisibleNodes.findIndex((n) => n.id === eventNodeId);

      if (currentIndex <= 0) return; // No previous node to focus

      const previousNode = currentVisibleNodes[currentIndex - 1];

      if (!previousNode || !nodeManager.nodes.has(previousNode.id)) {
        log.error('Previous node not found for focus after deletion:', previousNode?.id);
        // Can't combine without previous node - this shouldn't happen in normal usage
        return;
      }

      // Prevent merging into structured nodes (code-block, quote-block)
      // These nodes have specific formatting that can't accept arbitrary content
      if (!pluginRegistry.acceptsContentMerge(previousNode.nodeType)) {
        // Block the action entirely - don't delete, don't merge, don't focus
        // User must manually delete the node (e.g., Cmd+Backspace) or add content first
        return;
      }

      // Use combineNodes even for empty nodes (handles child promotion properly)
      nodeManager.combineNodes(eventNodeId, previousNode.id, paneId);
      requestNodeFocus(previousNode.id, previousNode.content.length);
    } catch (error) {
      log.error('Error during node deletion:', error);
    }
  }

  // Handle icon click events
  // Note: Node-specific components handle their own icon behavior (e.g., TaskNode manages task states)
  // This handler is for any viewer-level icon click coordination if needed in the future
  function handleIconClick() {
    // Currently a no-op - individual node components handle their own icon clicks
    // This makes the system extensible for future node types that need viewer-level coordination
  }

  // Handle task state changes (checkbox toggles) — route through schema field update
  function handleTaskStateChanged(node: ViewerRenderNode, detail: TaskStateChangedDetail) {
    const { nodeId: eventNodeId, state } = detail;
    updateSchemaField(
      viewerId,
      eventNodeId,
      'status',
      pluginRegistry.mapStateToSchema(node.nodeType, state, 'status')
    );
  }

  /**
   * Handle node content changes.
   * Promotes the viewer-local placeholder to a real persisted node on first content,
   * deferring store mutations to the next tick (mutating $state during template render
   * throws state_unsafe_mutation). Otherwise routes through nodeManager.
   */
  function handleContentChanged(node: ViewerRenderNode, detail: ContentChangedDetail) {
    const content = detail.content;
    const cursorPosition = detail.cursorPosition ?? content.length;

    // Capture $derived values immediately to prevent race condition
    const currentPlaceholder = viewerPlaceholder;
    const nodeExistsInStore = sharedNodeStore.hasNode(node.id);

    if (
      currentPlaceholder &&
      node.id === currentPlaceholder.id &&
      content.trim() !== '' &&
      nodeId &&
      !nodeExistsInStore &&
      !isPromoting
    ) {
      // ATOMIC PROMOTION: Set flag to block new placeholder creation
      isPromoting = true;

      // Prepare promoted node data synchronously
      const promotedNode = promotePlaceholderToNode(currentPlaceholder, nodeId, { content });

      // Clear placeholder ID synchronously to prevent re-entry
      resetPlaceholderId();

      // Defer store mutations to next tick
      // sharedNodeStore.setNode() triggers notifySubscribers() which calls wildcard
      // subscription callbacks that mutate $state. If called during template render,
      // Svelte throws "state_unsafe_mutation". tick() ensures we're outside render.
      const promotionParentId = nodeId;
      tick().then(() => {
        // Set editing state BEFORE store update
        focusManager.focusNodeFromTypeConversion(promotedNode.id, cursorPosition, paneId);

        // Add to shared store with persistence enabled
        sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

        // Add parent-child edge to reactiveStructureTree
        reactiveStructureTree.addChild({
          parentId: promotionParentId,
          childId: promotedNode.id,
          order: Date.now()
        });

        // Clear promotion flag after state updates complete
        isPromoting = false;
      });
    } else {
      // Regular node content update (placeholder flag is handled automatically)
      nodeManager.updateNodeContent(node.id, content);
    }
  }

  /**
   * Handle node type changes (pattern-detected conversions).
   * When the target is the viewer-local placeholder, promotes it to a real node of the
   * new type using the same tick-deferred, isPromoting-guarded path as contentChanged /
   * slashCommandSelected. Otherwise updates the existing node's content + type.
   */
  async function handleNodeTypeChanged(node: ViewerRenderNode, detail: NodeTypeChangedDetail) {
    const newNodeType = detail.nodeType;
    let cleanedContent = detail.cleanedContent;
    // Use cursor position from event (captured by TextareaController)
    const cursorPosition = detail.cursorPosition ?? 0;

    // Load component BEFORE updating node type (only if plugin has one)
    if (!nodeLoader.has(newNodeType) && pluginRegistry.hasNodeComponent(newNodeType)) {
      await nodeLoader.load(newNodeType);
    }

    // Normalize content for code-block conversion
    if (newNodeType === 'code-block') {
      cleanedContent = normalizeCodeBlockContent(cleanedContent);
    }

    // Handle placeholder nodes - promote them to real nodes with the new type
    const currentPlaceholder = viewerPlaceholder;
    const nodeExistsInStore = sharedNodeStore.hasNode(node.id);
    if (
      node.isPlaceholder &&
      nodeId &&
      currentPlaceholder &&
      node.id === currentPlaceholder.id &&
      !nodeExistsInStore &&
      !isPromoting
    ) {
      // ATOMIC PROMOTION: Set flag to block new placeholder creation
      isPromoting = true;

      // Promote placeholder to real node with the new type
      const promotedNode = promotePlaceholderToNode(currentPlaceholder, nodeId, {
        content: cleanedContent ?? node.content ?? '',
        nodeType: newNodeType
      });

      // Clear placeholder ID synchronously to prevent re-entry
      resetPlaceholderId();

      // Defer store mutations to next tick to avoid
      // state_unsafe_mutation during template render (matches contentChanged path).
      const promotionParentId = nodeId;
      tick().then(() => {
        // Set editing state so the promoted component sees focus on mount (deferred:
        // a synchronous focus write runs during the render flush and also throws).
        focusManager.focusNodeFromTypeConversion(promotedNode.id, cursorPosition, paneId);

        // Add to store and trigger persistence
        sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

        // CRITICAL: Add parent-child edge to reactiveStructureTree immediately
        // This makes the promoted node visible in visibleNodesFromStores, which causes
        // shouldShowPlaceholder to become false, switching from placeholder to real child.
        reactiveStructureTree.addChild({
          parentId: promotionParentId,
          childId: promotedNode.id,
          order: Date.now()
        });

        // Clear promotion flag after state updates complete
        isPromoting = false;
      });
    } else {
      // Defer the focus + content/type writes to next tick: run synchronously they
      // mutate $state during the keystroke's render flush and throw state_unsafe_mutation.
      // No explicit hasNode guard needed here (unlike the slash handler): these route
      // through nodeManager.updateNodeContent/updateNodeType, which internally
      // `if (!node) return` when node.id was promoted/removed since the event fired.
      tick().then(() => {
        focusManager.focusNodeFromTypeConversion(node.id, cursorPosition, paneId);

        // Update content if cleanedContent is provided (e.g., from contentTemplate)
        if (cleanedContent !== undefined) {
          nodeManager.updateNodeContent(node.id, cleanedContent);
        }

        // Update node type through proper API (triggers component re-render)
        nodeManager.updateNodeType(node.id, newNodeType);
      });
    }
  }

  /**
   * Handle slash command selection.
   * Placeholder promotion uses the tick-deferred, isPromoting-guarded path; existing
   * nodes get contentTemplate + nodeType applied atomically in one store update.
   */
  async function handleSlashCommandSelected(
    node: ViewerRenderNode,
    detail: SlashCommandSelectedDetail
  ) {
    // Use cursor position from event (captured by TextareaController)
    const cursorPosition = detail.cursorPosition ?? 0;
    const newNodeType = detail.nodeType;

    // Load component BEFORE updating node type (only if plugin has one)
    if (!nodeLoader.has(newNodeType) && pluginRegistry.hasNodeComponent(newNodeType)) {
      await nodeLoader.load(newNodeType);
    }

    log.debug('slashCommandSelected:', {
      nodeId: node.id,
      newType: detail.nodeType,
      isPlaceholder: node.isPlaceholder,
      hasViewerPlaceholder: !!viewerPlaceholder
    });

    // CRITICAL FIX: Treat slash commands on placeholders as real node type changes
    // They must persist to database, not just update locally
    // Use same batching logic as real nodes to ensure atomic persistence
    // Also check if node doesn't already exist in store (prevents duplicate promotion)
    const cmdDef = pluginRegistry.findSlashCommand(detail.command);
    const currentPlaceholder = viewerPlaceholder;
    const nodeExistsInStore = sharedNodeStore.hasNode(node.id);
    if (
      node.isPlaceholder &&
      nodeId &&
      currentPlaceholder &&
      node.id === currentPlaceholder.id &&
      !nodeExistsInStore &&
      !isPromoting
    ) {
      // ATOMIC PROMOTION: Set flag to block new placeholder creation
      isPromoting = true;

      log.debug('Promoting placeholder to real node with type:', detail.nodeType);
      // Promote placeholder to real node with the new type
      const promotedNode = promotePlaceholderToNode(currentPlaceholder, nodeId, {
        content: node.content || '',
        nodeType: detail.nodeType
      });

      // Clear placeholder ID synchronously to prevent re-entry
      resetPlaceholderId();

      // Defer store mutations to next tick
      // sharedNodeStore.setNode() triggers notifySubscribers() which calls wildcard
      // subscription callbacks that mutate $state. If called during template render,
      // Svelte throws "state_unsafe_mutation". tick() ensures we're outside render.
      const promotionParentId = nodeId;
      tick().then(() => {
        // Set editing state so the promoted component sees focus on mount.
        // Deferred with the store writes — a synchronous focus write here runs during
        // the keystroke's render flush and also throws state_unsafe_mutation.
        focusManager.focusNodeFromTypeConversion(promotedNode.id, cursorPosition, paneId);

        // Add to store and trigger persistence
        // Note: domain events handles parent-child relationship via edge:created events
        sharedNodeStore.setNode(promotedNode, { type: 'viewer', viewerId }, false);

        // CRITICAL: Add parent-child edge to reactiveStructureTree immediately
        // This makes the promoted node visible in visibleNodesFromStores, which causes
        // shouldShowPlaceholder to become false, switching the binding from placeholder to real child.
        // Backend will also create the edge when persisting, and SSE will confirm (no-op since already added).
        reactiveStructureTree.addChild({
          parentId: promotionParentId,
          childId: promotedNode.id,
          order: Date.now()
        });

        // Clear promotion flag after state updates complete
        isPromoting = false;
      });
    } else {
      log.debug('Updating node type for real node');
      // Apply contentTemplate + nodeType atomically via a single store update.
      // Two separate updates would race (the second cancels the first's persist).
      const contentTemplate = cmdDef?.contentTemplate;
      const updatePayload: Record<string, unknown> = { nodeType: detail.nodeType };
      if (contentTemplate !== undefined) {
        updatePayload.content = contentTemplate;
      }
      // Defer the focus + store writes to next tick: run synchronously they mutate
      // $state during the keystroke's render flush and throw state_unsafe_mutation.
      // Guard the whole operation with hasNode — if node.id was promoted/removed since
      // the event fired, there is nothing left to convert: skip focus, the update
      // (which would log "Cannot update non-existent node"), and the custom-entity flow.
      tick().then(() => {
        if (!sharedNodeStore.hasNode(node.id)) return;
        focusManager.focusNodeFromTypeConversion(node.id, cursorPosition, paneId);
        sharedNodeStore.updateNode(node.id, updatePayload, { type: 'viewer', viewerId });
      });
    }
  }

  // Derive the list of nodes to render - either viewer placeholder or real children
  const nodesToRender = $derived<() => ViewerRenderNode[]>(() => {
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
  // Components are cached in nodeLoader for subsequent renders

  // Clean up on component unmount and flush pending saves
  onDestroy(() => {
    // Unregister this viewer from the expansion coordinator
    NodeExpansionCoordinator.unregisterViewer(tabId);

    // CRITICAL: Commit ALL active batches globally BEFORE flushing
    // This ensures node type conversions (which use batches) are saved
    // We must commit globally because visible nodes might be empty if viewer already unmounted
    sharedNodeStore.commitAllBatches();

    // Note: PersistenceCoordinator removed
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
        id="viewer-header-{paneId}-{nodeId ?? 'default'}"
        class="header-input"
        class:header-input--readonly={schemaFormLoader.hasTitleTemplate}
        value={isHeaderBeingEdited ? currentViewedNode?.content || '' : headerDisplayValue}
        oninput={(e) =>
          !schemaFormLoader.hasTitleTemplate && handleHeaderInput(e.currentTarget.value)}
        onfocus={() => {
          if (!schemaFormLoader.hasTitleTemplate) isHeaderBeingEdited = true;
        }}
        onblur={() => (isHeaderBeingEdited = false)}
        readonly={schemaFormLoader.hasTitleTemplate}
        placeholder={schemaFormLoader.hasTitleTemplate
          ? (schemaFormLoader.genericSchema?.titleTemplate ?? 'Untitled')
          : 'Untitled'}
        aria-label="Page title"
      />
    </div>
  {/if}

  <!-- Schema-Driven Properties Panel - fixed between header and content area -->
  <!-- Type-specific schema forms use plugin registry for smart dispatch -->
  <!-- Types with a hardcoded form (task, person) use it; everything else falls back to the
       generic GenericSchemaForm — including core types with no registered form (project) -->
  <!-- Only render when a schema form is known to exist: null means "checked, none registered" -->
  {#if currentViewedNode && nodeId && schemaFormLoader.getForm(currentViewedNode.nodeType)}
    {@const TypedSchemaForm = schemaFormLoader.getForm(
      currentViewedNode.nodeType
    ) as SchemaFormComponent}
    <div class="schema-form-container">
      <TypedSchemaForm {nodeId} />
    </div>
  {:else if currentViewedNode && nodeId && schemaFormLoader.genericSchema}
    <!-- Generic schema-driven form. No extra type check is needed: genericSchema is only
         populated by the loader's no-hardcoded-form path, and only when the backend
         actually returned a schema — so a type with no schema renders nothing here. -->
    <!-- autoOpen is captured once at GenericSchemaForm mount time (not reactively synced).
         Safe only because this branch doesn't render until genericSchema is loaded, so
         hasTitleTemplate is already final by the time autoOpen is read. -->
    <div class="schema-form-container">
      <GenericSchemaForm
        {nodeId}
        schema={schemaFormLoader.genericSchema}
        autoOpen={schemaFormLoader.hasTitleTemplate}
      />
    </div>
  {/if}

  <!-- Scrollable Node Content Area (children structure) -->
  <div class="node-content-area" bind:this={scrollContainer}>
    {#each nodesToRender() as node (node.id)}
      {@const relativeDepth = (node.depth || 0) - minDepth()}
      <div
        class="node-container"
        data-has-children={node.children?.length > 0}
        style="margin-left: {relativeDepth * 2.5}rem"
      >
        <NodeRow
          {node}
          loadedNodeComponent={nodeLoader.get(node.nodeType)}
          {paneId}
          onCreateNewNode={handleCreateNewNode}
          onIndentNode={handleIndentNode}
          onOutdentNode={handleOutdentNode}
          onNavigateArrow={handleArrowNavigation}
          onContentChanged={handleContentChanged}
          onNodeTypeChanged={handleNodeTypeChanged}
          onSlashCommandSelected={handleSlashCommandSelected}
          onIconClick={handleIconClick}
          onTaskStateChanged={handleTaskStateChanged}
          onCombineWithPrevious={handleCombineWithPrevious}
          onDeleteNode={handleDeleteNode}
          onToggleExpanded={handleToggleExpanded}
        />
      </div>
    {/each}
  </div>

  <!-- Backlinks Panel - outside scroll area, fixed at bottom of viewer -->
  {#if nodeId}
    <BacklinksPanel backlinks={currentViewedNode?.mentionedIn ?? []} />
  {/if}
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

  .header-input--readonly {
    cursor: default;
    color: hsl(var(--foreground));
  }

  .header-input--readonly::placeholder {
    color: hsl(var(--muted-foreground) / 0.5);
    font-style: italic;
  }

  /* Custom header section - fixed at top, doesn't scroll */
  .viewer-header {
    flex-shrink: 0;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  /* Schema form container - fixed between header and content, doesn't scroll */
  .schema-form-container {
    flex-shrink: 0;
    padding: 0 var(--viewer-padding-horizontal);
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
  }

  .node-container {
    /* Individual node wrapper - no additional spacing */
    /* Allow chevrons to extend outside container bounds */
    overflow: visible;
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
