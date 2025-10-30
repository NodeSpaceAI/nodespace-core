<!--
  BaseNode - Abstract Core Component (Internal Use Only)

  IMPORTANT: This component should NOT be used directly in application code.
  It serves as the abstract foundation for all node viewers and should only
  be consumed by concrete node viewer implementations like TextNodeViewer.

  Core Features:
  - TextareaController integration for markdown editing (textarea-based)
  - Edit/view mode switching on focus/blur
  - Universal keyboard shortcuts (Enter, Backspace, Cmd+B/I, etc.)
  - Node reference autocomplete system (@-trigger)
  - Base styling and layout for all node types

  Architecture:
  - Abstract Base: Provides core functionality but should not be instantiated directly
  - Concrete Viewers: TextNodeViewer, TaskNodeViewer, etc. wrap this component
  - Plugin System: Registered viewers extend this base with type-specific logic

  Migration Note:
  - Replaced ContentEditableController with TextareaController (Issue #274)
  - Single source of truth: textarea.value (no dual representation)
  - Simpler state management and cursor positioning
-->

<script lang="ts">
  import { createEventDispatcher, onDestroy, untrack } from 'svelte';
  // import { type NodeType } from '$lib/design/icons'; // Unused but preserved for future use

  import {
    TextareaController,
    type TextareaControllerEvents,
    type TextareaControllerConfig
  } from './textarea-controller.js';
  import { NodeAutocomplete, type NodeResult } from '$lib/components/ui/node-autocomplete';
  import { SlashCommandDropdown } from '$lib/components/ui/slash-command-dropdown';
  import { Calendar } from '$lib/components/ui/calendar';
  import { type DateValue } from '@internationalized/date';
  import {
    SlashCommandService,
    type SlashCommand,
    type SlashCommandContext
  } from '$lib/services/slash-command-service';
  import type { TriggerContext } from '$lib/services/node-reference-service';
  import { getIconConfig, resolveNodeState, type NodeType } from '$lib/design/icons/registry';
  import { getNodeServices } from '$lib/contexts/node-service-context.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import { positionCursor } from '$lib/actions/position-cursor';
  import { createMockElementForView, findCharacterFromClickFast } from './cursor-positioning';
  import { mapViewPositionToEditPosition } from '$lib/utils/view-edit-mapper';
  import { NodeSearchService } from '$lib/services/node-search-service';

  // Props (Svelte 5 runes syntax) - nodeReferenceService removed
  let {
    nodeId,
    nodeType = $bindable('text'),
    autoFocus = false,
    content = $bindable(''),
    displayContent,
    children = [],
    editableConfig = {},
    metadata = {}
  }: {
    nodeId: string;
    nodeType?: string;
    autoFocus?: boolean;
    content?: string;
    displayContent?: string; // Optional content to display in blur mode (for syntax stripping)
    children?: string[];
    editableConfig?: TextareaControllerConfig;
    metadata?: Record<string, unknown>;
  } = $props();

  // isEditing is now derived from FocusManager (single source of truth)
  // This replaces the old bindable prop approach
  let isEditing = $derived(focusManager.editingNodeId === nodeId);

  // Derive cursor positioning data for the action (reactive architecture)
  // Only provide cursor data when this node is actively being edited
  let cursorPositionData = $derived(
    isEditing && focusManager.editingNodeId === nodeId ? focusManager.cursorPosition : null
  );

  // Get services from context
  // In test environment, enable mock service to allow autocomplete testing
  // In production, use real service from context
  const services = getNodeServices();
  const nodeReferenceService =
    services?.nodeReferenceService ||
    (import.meta.env.VITEST ? ({} as Record<string, never>) : null);

  /**
   * Extract text from view element while preserving line breaks from <br> tags
   * @param element - The view div element
   * @returns Text content with \n for each <br> tag
   */
  function extractTextWithLineBreaks(element: HTMLElement): string {
    let text = '';
    const walk = (node: Node) => {
      if (node.nodeType === Node.TEXT_NODE) {
        text += node.textContent || '';
      } else if (node.nodeName === 'BR') {
        text += '\n';
      } else if (node.childNodes) {
        node.childNodes.forEach(walk);
      }
    };
    walk(element);
    return text;
  }

  // DOM element and controller - Svelte bind:this assignment
  let textareaElement = $state<HTMLTextAreaElement | undefined>(undefined);
  let controller = $state<TextareaController | null>(null);

  // View mode element for rendering markdown
  let viewElement = $state<HTMLDivElement | undefined>(undefined);

  // Autocomplete modal state
  let showAutocomplete = $state(false);
  let autocompletePosition = $state({ x: 0, y: 0 });
  let currentQuery = $state('');
  let autocompleteResults = $state<NodeResult[]>([]);
  let autocompleteLoading = $state(false);

  // Slash command modal state
  let showSlashCommands = $state(false);
  let slashCommandPosition = $state({ x: 0, y: 0 });
  let currentSlashQuery = $state('');
  let slashCommands = $state<SlashCommand[]>([]);
  let slashCommandService = SlashCommandService.getInstance();

  // Date picker popover state
  let showDatePicker = $state(false);
  let datePickerPosition = $state({ x: 0, y: 0 });
  let selectedCalendarDate = $state<DateValue | undefined>(undefined);

  // Generate mock autocomplete results for TEST ENVIRONMENT ONLY
  // Used only when import.meta.env.VITEST is true
  function generateMockResultsForTests(query: string): NodeResult[] {
    const mockNodes: NodeResult[] = [
      {
        id: 'mock-node-1',
        title: 'Welcome to NodeSpace',
        type: 'text',
        subtitle: 'Getting started with your knowledge management system',
        metadata: '2 days ago'
      },
      {
        id: 'mock-node-2',
        title: 'Project Notes',
        type: 'document',
        subtitle: 'Comprehensive project documentation and meeting notes',
        metadata: '1 week ago'
      },
      {
        id: 'mock-node-3',
        title: 'Task List',
        type: 'task',
        subtitle: 'Important tasks and action items for the current sprint',
        metadata: '3 days ago'
      },
      {
        id: 'mock-node-4',
        title: 'AI Research Chat',
        type: 'ai-chat',
        subtitle: 'Conversation about machine learning and AI development',
        metadata: '5 days ago'
      },
      {
        id: 'mock-node-5',
        title: 'User Research Findings',
        type: 'user',
        subtitle: 'Key insights from user interviews and usability testing',
        metadata: '1 week ago'
      },
      {
        id: 'mock-node-6',
        title: 'Search Query Examples',
        type: 'query',
        subtitle: 'Commonly used search patterns and filters',
        metadata: '4 days ago'
      }
    ];

    if (!query.trim()) {
      return mockNodes.slice(0, 4);
    }

    return mockNodes.filter(
      (node) =>
        node.title.toLowerCase().includes(query.toLowerCase()) ||
        (node.subtitle && node.subtitle.toLowerCase().includes(query.toLowerCase()))
    );
  }

  // ============================================================================
  // Date Shortcuts and Formatting Logic
  // ============================================================================

  /**
   * Format a Date object to YYYY-MM-DD string
   */
  function formatDate(date: Date): string {
    return date.toISOString().split('T')[0];
  }

  /**
   * Get date string from shortcut name
   */
  function getDateFromShortcut(shortcut: string): string {
    const today = new Date();

    switch (shortcut) {
      case 'today':
        return formatDate(today);
      case 'tomorrow': {
        const tomorrow = new Date(today);
        tomorrow.setDate(today.getDate() + 1);
        return formatDate(tomorrow);
      }
      case 'yesterday': {
        const yesterday = new Date(today);
        yesterday.setDate(today.getDate() - 1);
        return formatDate(yesterday);
      }
      default:
        return formatDate(today);
    }
  }

  /**
   * Generate date shortcuts for autocomplete
   */
  function getDateShortcuts(query: string): NodeResult[] {
    const shortcuts: NodeResult[] = [
      { id: 'today', title: 'Today', type: 'date', isShortcut: true },
      { id: 'tomorrow', title: 'Tomorrow', type: 'date', isShortcut: true },
      { id: 'yesterday', title: 'Yesterday', type: 'date', isShortcut: true },
      { id: 'date-picker', title: 'Select date...', type: 'date', isShortcut: true }
    ];

    // Filter shortcuts based on query
    return shortcuts.filter((shortcut) =>
      shortcut.title.toLowerCase().includes(query.toLowerCase())
    );
  }

  // Reactive effect to update autocomplete results when query changes
  $effect(() => {
    if (showAutocomplete && nodeReferenceService) {
      // Use real search if services are available
      if (services?.nodeManager) {
        performRealSearch(currentQuery);
      } else {
        // Fallback to mock results ONLY in test mode when nodeManager is not available
        autocompleteResults = generateMockResultsForTests(currentQuery);
      }
    }
  });

  // Watch calendar date selection and handle it
  $effect(() => {
    console.log(
      '[BaseNode] Calendar $effect triggered, selectedCalendarDate:',
      selectedCalendarDate
    );
    if (selectedCalendarDate) {
      console.log('[BaseNode] Calendar date selected:', selectedCalendarDate);
      const { year, month, day } = selectedCalendarDate;
      // Convert DateValue to JS Date (month is 1-indexed in DateValue, 0-indexed in JS Date)
      const jsDate = new Date(year, month - 1, day);

      console.log('[BaseNode] Calling handleDateSelection with jsDate:', jsDate);
      // Call date selection handler
      handleDateSelection(jsDate);

      // Reset selection for next time (do this AFTER handleDateSelection to avoid loop)
      selectedCalendarDate = undefined;
    }
  });

  // Perform real search using the node manager
  async function performRealSearch(query: string) {
    try {
      autocompleteLoading = true;

      // Guard clause: Early return if nodeManager is not available
      if (!services?.nodeManager) {
        autocompleteResults = [];
        return;
      }

      // Get date shortcuts first (always shown)
      const dateShortcuts = getDateShortcuts(query);

      // Get all nodes from the node manager
      const allNodes = Array.from(services.nodeManager.nodes.values());

      // Filter nodes using NodeSearchService
      // Rules: Exclude dates, match query, show all non-text + container text only
      const filtered = NodeSearchService.filterMentionableNodes(allNodes, query).slice(0, 10); // Limit to 10 results

      // Convert to NodeResult format
      const nodeResults = filtered.map((node) => ({
        id: node.id,
        title: node.content.split('\n')[0] || 'Untitled',
        type: (node.nodeType || 'text') as NodeType
      }));

      // Combine: date shortcuts first, then search results
      autocompleteResults = [...dateShortcuts, ...nodeResults];
    } catch (error) {
      console.error('[NodeSearch] Search failed:', error);
      autocompleteResults = [];
    } finally {
      autocompleteLoading = false;
    }
  }

  // Reactive effect to update slash commands when query changes
  $effect(() => {
    if (showSlashCommands) {
      slashCommands = slashCommandService.filterCommands(currentSlashQuery);
    }
  });

  // Close date picker when autocomplete closes
  $effect(() => {
    if (!showAutocomplete && showDatePicker) {
      showDatePicker = false;
    }
  });

  // Autocomplete event handlers
  async function handleAutocompleteSelect(result: NodeResult) {
    // Handle date picker special case
    if (result.id === 'date-picker') {
      // Capture submenu position if available
      const resultWithPosition = result as NodeResult & {
        submenuPosition?: { x: number; y: number };
      };
      if (resultWithPosition.submenuPosition) {
        datePickerPosition = resultWithPosition.submenuPosition;
      }
      // Keep autocomplete open (it will show the date picker alongside)
      // Just toggle the date picker popover
      showDatePicker = true;
      return;
    }

    // Handle date shortcuts (today, tomorrow, yesterday)
    if (result.isShortcut && result.id !== 'date-picker') {
      const dateStr = getDateFromShortcut(result.id);
      await handleDateSelection(new Date(dateStr));
      return;
    }

    // Handle normal node references
    if (controller) {
      if (result.id === 'new') {
        const newNodeId = await createNewNodeFromMention(result.title);
        if (newNodeId) {
          controller.insertNodeReference(newNodeId, result.title);
        }
      } else {
        controller.insertNodeReference(result.id, result.title);
      }
    }

    // Hide autocomplete and clear state
    showAutocomplete = false;
    currentQuery = '';
    autocompleteResults = [];

    // Emit event for parent components to handle if needed
    dispatch('nodeReferenceSelected', {
      nodeId: result.id,
      nodeTitle: result.title
    });
  }

  /**
   * Helper function to check if a node is a container node
   * Container nodes have containerNodeId === null (they ARE containers, not contained)
   */
  function isContainerNode(node: { containerNodeId: string | null; id: string }): boolean {
    return node.containerNodeId === null;
  }

  /**
   * Creates a new standalone top-level node from an @mention query
   * Returns the new node ID if successful, null otherwise
   */
  async function createNewNodeFromMention(title: string): Promise<string | null> {
    try {
      if (!services?.nodeManager) {
        return null;
      }

      const nodeManager = services.nodeManager;
      const { v4: uuidv4 } = await import('uuid');
      const { backendAdapter } = await import('$lib/services/backend-adapter');

      const newNodeId = uuidv4();

      // Find the last root node to get the correct order
      const allNodes = Array.from(nodeManager.nodes.values());
      const rootNodes = allNodes.filter(isContainerNode);
      const lastRootNode = rootNodes[rootNodes.length - 1];
      const beforeSiblingId = lastRootNode ? lastRootNode.id : null;

      // Create node using backend adapter (works in both Tauri and browser)
      await backendAdapter.createNode({
        id: newNodeId,
        content: title,
        nodeType: 'text',
        parentId: null,
        containerNodeId: null,
        beforeSiblingId: beforeSiblingId,
        properties: {},
        embeddingVector: null
      });

      // The backend adapter emits events that update nodeManager automatically
      // So we don't need to manually update nodeManager.nodes here

      return newNodeId;
    } catch (error) {
      console.error('[NodeCreation] Failed to create node:', error);
      return null;
    }
  }

  /**
   * Handle date selection from shortcuts or date picker
   * Date nodes are virtual - they only get persisted when children are added
   * Here we just create the markdown link without database creation
   */
  async function handleDateSelection(date: Date) {
    const dateStr = formatDate(date);

    if (controller) {
      // Insert as a node reference (date nodes are virtual, no DB creation needed)
      controller.insertNodeReference(dateStr, dateStr);
    }

    // Hide date picker and autocomplete
    showDatePicker = false;
    showAutocomplete = false;
    currentQuery = '';
    autocompleteResults = [];

    // Emit event
    dispatch('nodeReferenceSelected', {
      nodeId: dateStr,
      nodeTitle: dateStr
    });
  }

  // Slash command event handlers
  function handleSlashCommandSelect(command: SlashCommand) {
    let finalCursorPosition = 0;

    if (controller) {
      // Execute the command and get the content to insert
      const result = slashCommandService.executeCommand(command);

      // Insert the command content and get the calculated cursor position
      // Skip cursor positioning here since the component will re-render and we'll position via requestNodeFocus
      // Pass the target node type so insertSlashCommand can clean header syntax appropriately
      const calculatedCursorPosition = controller.insertSlashCommand(
        result.content,
        true,
        result.nodeType
      );

      // Use explicit desiredCursorPosition from plugin definition if available,
      // otherwise fall back to calculated position from insertSlashCommand
      finalCursorPosition = result.desiredCursorPosition ?? calculatedCursorPosition;

      // Don't update nodeType locally - let parent handle it to avoid double re-renders
      // The parent (base-node-viewer) will update nodeType via nodeManager and trigger autoFocus
      //
      // ARCHITECTURE (Issue #311): Header level changes are handled by HeaderNode component via $effect
      // TextareaController only detects pattern → header conversion, NOT level changes within headers
      // This separation ensures BaseNode remains node-type agnostic
    }

    // Hide slash commands and clear state first
    showSlashCommands = false;
    currentSlashQuery = '';
    slashCommands = [];

    // Ensure controller knows dropdown is closed immediately to prevent double Enter handling
    if (controller) {
      controller.setSlashCommandDropdownActive(false);
    }

    // Emit event for parent components to handle node type changes
    // This will trigger the component switch, so we don't focus here - let autoFocus handle it
    dispatch('slashCommandSelected', {
      command: command.id,
      nodeType: command.nodeType,
      cursorPosition: finalCursorPosition // Use plugin's desired position or fallback to calculated
    });
  }

  // Handle icon click events (for task state changes, etc.)
  function handleIconClick(event: MouseEvent | KeyboardEvent) {
    const currentState = resolveNodeState(nodeType as NodeType, undefined, metadata);
    event.stopPropagation(); // Prevent triggering content editable focus

    dispatch('iconClick', {
      nodeId: nodeId,
      nodeType: nodeType,
      currentState: currentState
    });
  }

  // Import markdown renderer for view mode
  import { markdownToHtml } from '$lib/utils/marked-config';

  // Event dispatcher - aligned with NodeViewerEventDetails interface
  const dispatch = createEventDispatcher<{
    contentChanged: { content: string };
    focus: void;
    blur: void;
    createNewNode: {
      afterNodeId: string;
      nodeType: string;
      currentContent?: string;
      newContent?: string;
      originalContent?: string;
      inheritHeaderLevel?: number;
      cursorAtBeginning?: boolean;
    };
    indentNode: { nodeId: string };
    outdentNode: { nodeId: string };
    navigateArrow: { nodeId: string; direction: 'up' | 'down'; pixelOffset: number };
    combineWithPrevious: { nodeId: string; currentContent: string };
    deleteNode: { nodeId: string };
    nodeReferenceSelected: { nodeId: string; nodeTitle: string };
    slashCommandSelected: { command: string; nodeType: string; cursorPosition?: number };
    iconClick: { nodeId: string; nodeType: string; currentState?: string };
    nodeTypeChanged: { nodeType: string; cleanedContent?: string; cursorPosition?: number };
  }>();

  // Controller event handlers
  const controllerEvents: TextareaControllerEvents = {
    contentChanged: (content: string) => dispatch('contentChanged', { content }),
    focus: () => {
      // Use FocusManager as single source of truth
      // Only update if this node isn't already set as editing
      // (Don't overwrite arrow navigation context set by setEditingNodeFromArrowNavigation)
      if (focusManager.editingNodeId !== nodeId) {
        focusManager.setEditingNode(nodeId);
      }

      // CRITICAL: Clear node type conversion flag after successful focus
      // This allows normal blur behavior to resume after the conversion is complete
      if (focusManager.cursorPosition?.type === 'node-type-conversion') {
        focusManager.clearNodeTypeConversionCursorPosition();
      }

      dispatch('focus');
    },
    blur: () => {
      // Use FocusManager as single source of truth
      // Only clear editing if THIS node is still the editing node
      // (Don't clear if focus has already moved to another node via arrow navigation)
      // CRITICAL: Don't clear if this is a node type conversion (component is re-mounting)
      untrack(() => {
        const isNodeTypeConversion = focusManager.cursorPosition?.type === 'node-type-conversion';
        if (focusManager.editingNodeId === nodeId && !isNodeTypeConversion) {
          focusManager.clearEditing();
        }
      });

      // Hide autocomplete modal when losing focus
      setTimeout(() => {
        if (showAutocomplete && controller && !controller.isProcessingInput()) {
          showAutocomplete = false;
        }
      }, 50);
      dispatch('blur');
    },
    createNewNode: (data) => dispatch('createNewNode', data),
    indentNode: (data) => dispatch('indentNode', data),
    outdentNode: (data) => dispatch('outdentNode', data),
    navigateArrow: (data) => dispatch('navigateArrow', data),
    combineWithPrevious: (data) => dispatch('combineWithPrevious', data),
    deleteNode: (data) => dispatch('deleteNode', data),
    // @ Trigger System Events
    triggerDetected: (data: {
      triggerContext: TriggerContext;
      cursorPosition: { x: number; y: number };
    }) => {
      if (nodeReferenceService) {
        currentQuery = data.triggerContext.query;
        autocompletePosition = data.cursorPosition;
        showAutocomplete = true;
      }
    },
    triggerHidden: () => {
      showAutocomplete = false;
      currentQuery = '';
      autocompleteResults = [];
    },
    nodeReferenceSelected: (data: { nodeId: string; nodeTitle: string }) => {
      // Forward the event for potential parent component handling
      dispatch('nodeReferenceSelected', data);
    },
    // / Slash Command System Events
    slashCommandDetected: (data: {
      commandContext: SlashCommandContext;
      cursorPosition: { x: number; y: number };
    }) => {
      currentSlashQuery = data.commandContext.query;
      slashCommandPosition = data.cursorPosition;
      slashCommands = slashCommandService.filterCommands(currentSlashQuery);
      showSlashCommands = true;
    },
    slashCommandHidden: () => {
      showSlashCommands = false;
      currentSlashQuery = '';
      slashCommands = [];
    },
    slashCommandSelected: (data: {
      command: {
        content: string;
        nodeType: string;
        headerLevel?: number;
      };
    }) => {
      // This is handled directly in handleSlashCommandSelect
      // Validate data structure for interface compatibility
      if (data?.command?.content !== undefined) {
        // Interface validation passed
      }
    },
    directSlashCommand: (data: { command: string; nodeType: string; cursorPosition?: number }) => {
      // Handle direct slash command typing by simulating dropdown selection

      // Emit the same event that dropdown selection emits, including cursor position
      dispatch('slashCommandSelected', {
        command: data.command,
        nodeType: data.nodeType,
        cursorPosition: data.cursorPosition
      });
    },
    // Node Type Conversion Events
    nodeTypeConversionDetected: (data: {
      nodeId: string;
      newNodeType: string;
      cleanedContent: string;
      cursorPosition: number;
    }) => {
      // Don't dispatch contentChanged here - it causes Svelte 5 state_unsafe_mutation error
      // The cleaned content is passed in nodeTypeChanged event and handled by parent
      dispatch('nodeTypeChanged', {
        nodeType: data.newNodeType,
        cleanedContent: data.cleanedContent,
        cursorPosition: data.cursorPosition
      });
    }
  };

  // REMOVED: Manual sync $effect no longer needed
  // isEditing is now $derived from FocusManager automatically
  // This ensures perfect sync without manual coordination

  // Initialize controller when element is available (Svelte 5 $effect)
  // Note: Must explicitly access textareaElement to track dependency
  $effect(() => {
    const element = textareaElement; // Force dependency tracking
    if (element && !controller) {
      // Create controller immediately - DOM should be ready when effect runs
      controller = new TextareaController(
        element,
        nodeId,
        nodeType,
        controllerEvents,
        editableConfig
      );
      controller.initialize(content, autoFocus);
    } else if (element && controller) {
      // If autoFocus is true and controller exists, still call focus to restore pending cursor position
      if (autoFocus) {
        setTimeout(() => controller?.focus(), 10);
      }
    } else if (!element && controller) {
      // Element was destroyed (switched to view mode) - clean up controller
      controller.destroy();
      controller = null;
    }
  });

  // Update view mode rendering when content or editing state changes
  $effect(() => {
    if (!isEditing && viewElement) {
      // Use displayContent if provided (for node types that strip syntax in blur mode), otherwise use content
      let processedContent = displayContent ?? content;

      // Check if markdown processing should be disabled (e.g., for code blocks)
      const disableMarkdown = metadata?.disableMarkdown === true;

      let html: string;

      if (disableMarkdown) {
        // Raw text mode - just convert newlines to <br>, no markdown processing
        html = processedContent
          .split('\n')
          .map((line) => line || '') // Keep empty lines
          .join('<br>');
      } else {
        // Normal markdown processing for text/header/task nodes
        // IMPORTANT: Process blank lines BEFORE markdownToHtml
        // marked.js with breaks:true converts all \n to <br>, losing the ability to detect \n\n
        const BLANK_LINE_PLACEHOLDER = '___BLANK___';

        // Replace consecutive newlines with placeholders
        processedContent = processedContent.replace(/\n\n+/g, (match) => {
          // Number of blank lines = (number of \n) - 1
          // "\n\n" = 1 blank line, "\n\n\n" = 2 blank lines
          const blankLineCount = match.length - 1;
          return '\n' + BLANK_LINE_PLACEHOLDER.repeat(blankLineCount);
        });

        // Process markdown (converts \n to <br>, handles formatting)
        html = markdownToHtml(processedContent);

        // Replace placeholders with <br> tags for blank lines
        html = html.replace(new RegExp(BLANK_LINE_PLACEHOLDER, 'g'), '<br>');
      }

      // Preserve leading newlines
      const leadingNewlines = content.match(/^\n+/);
      if (leadingNewlines) {
        html = '<br>'.repeat(leadingNewlines[0].length) + html;
      }

      // Preserve trailing newlines
      const trailingNewlines = content.match(/\n+$/);
      if (trailingNewlines) {
        html += '<br>'.repeat(trailingNewlines[0].length + 1);
      }

      viewElement.innerHTML = html;
    }
  });

  // Update content when prop changes
  $effect(() => {
    if (controller && content !== undefined) {
      controller.updateContent(content);
    }
  });

  // Update controller config when editableConfig prop changes
  $effect(() => {
    if (controller && editableConfig) {
      controller.updateConfig(editableConfig);
    }
  });

  // REMOVED: Old imperative $effect blocks for cursor positioning
  // Replaced with declarative positionCursor action (see template below)
  // The action reactively positions the cursor based on cursorPositionData from FocusManager

  // Update controller when slash command dropdown state changes
  $effect(() => {
    if (controller) {
      controller.setSlashCommandDropdownActive(showSlashCommands);
    }
  });

  // Update controller when autocomplete dropdown state changes
  $effect(() => {
    if (controller) {
      controller.setAutocompleteDropdownActive(showAutocomplete);
    }
  });

  onDestroy(() => {
    if (controller) {
      controller.destroy();
    }
  });

  // ============================================================================
  // Autocomplete Modal Event Handlers
  // ============================================================================

  function handleAutocompleteClose(): void {
    showAutocomplete = false;
    currentQuery = '';

    if (controller) {
      controller.focus();
    }
  }

  function handleSlashCommandClose(): void {
    showSlashCommands = false;
    currentSlashQuery = '';

    // Return focus to the content editable element
    if (controller) {
      controller.focus();
    }
  }

  // ============================================================================
  // Utility Functions
  // ============================================================================

  // Compute CSS classes
  const containerClasses = $derived(
    ['node', `node--${nodeType}`, children.length > 0 && 'node--has-children']
      .filter(Boolean)
      .join(' ')
  );

  // REACTIVITY FIX: Create reactive derived state for icon configuration
  // This ensures template re-renders when nodeType changes
  const iconConfig = $derived(getIconConfig(nodeType as NodeType));
  const nodeState = $derived(resolveNodeState(nodeType as NodeType, undefined, metadata));
  const hasChildren = $derived(children.length > 0);

  // Note: Semantic classes handled internally by Icon component
</script>

<div class={containerClasses} data-node-id={nodeId}>
  <!-- Node indicator -->
  <div
    class="node__indicator"
    onclick={handleIconClick}
    onkeydown={(e) => e.key === 'Enter' && handleIconClick(e)}
    role="button"
    tabindex="0"
    aria-label="Toggle node state"
  >
    {#if iconConfig}
      <div class={iconConfig.semanticClass}>
        {#snippet iconComponent()}
          {@const IconComponent = iconConfig.component}
          <IconComponent
            size={20}
            state={iconConfig.hasState ? nodeState : undefined}
            hasChildren={iconConfig.hasRingEffect ? hasChildren : undefined}
            color={iconConfig.colorVar}
          />
        {/snippet}
        {@render iconComponent()}
      </div>
    {:else}
      <!-- Fallback for unrecognized node types -->
      <div class="node-icon">
        <div class="text-circle"></div>
      </div>
    {/if}
  </div>

  <!-- Content area: textarea for editing, div for viewing -->
  {#if isEditing}
    <textarea
      bind:this={textareaElement}
      use:positionCursor={{ data: cursorPositionData, controller }}
      class="node__content node__content--textarea"
      id="textarea-{nodeId}"
      rows="1"
      tabindex="0"
    ></textarea>
  {:else}
    <div
      bind:this={viewElement}
      class="node__content node__content--view"
      id="view-{nodeId}"
      tabindex="0"
      onclick={(e) => {
        // Capture click coordinates
        const clickX = e.pageX;
        const clickY = e.pageY;

        // Get the actual rendered text from the view element
        // This is what the user sees (syntax stripped by markdown renderer)
        // IMPORTANT: We need to preserve line breaks from <br> tags
        const viewText = extractTextWithLineBreaks(viewElement!);

        // Create temporary mock element with character spans using the rendered view text
        const mockElement = createMockElementForView(viewElement!, viewText);
        const mockRect = mockElement.getBoundingClientRect();

        // Find character position in VIEW content
        const viewPositionResult = findCharacterFromClickFast(mockElement, clickX, clickY, {
          left: mockRect.left,
          top: mockRect.top,
          width: mockRect.width,
          height: mockRect.height
        });

        // Clean up mock element immediately
        mockElement.remove();

        // Map view position → edit position (accounting for syntax)
        const editPosition = mapViewPositionToEditPosition(
          viewPositionResult.index,
          viewText, // View content (actual rendered text)
          content // Edit content (with syntax)
        );

        // Set focus with cursor position via FocusManager
        focusManager.focusNodeAtPosition(nodeId, editPosition);
      }}
      onkeydown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          // Use FocusManager instead of directly setting isEditing
          focusManager.setEditingNode(nodeId);
          setTimeout(() => controller?.focus(), 0);
        }
      }}
      role="button"
    ></div>
  {/if}
</div>

<!-- Professional Node Autocomplete Component -->
{#if nodeReferenceService}
  <NodeAutocomplete
    position={autocompletePosition}
    query={currentQuery}
    results={autocompleteResults}
    loading={autocompleteLoading}
    visible={showAutocomplete}
    onselect={handleAutocompleteSelect}
    onclose={handleAutocompleteClose}
  />
{/if}

<!-- Slash Command Dropdown Component -->
<SlashCommandDropdown
  position={slashCommandPosition}
  query={currentSlashQuery}
  commands={slashCommands}
  loading={false}
  visible={showSlashCommands}
  onselect={handleSlashCommandSelect}
  onclose={handleSlashCommandClose}
/>

<!-- Date Picker Component (positioned as submenu relative to "Select date" item) -->
{#if showDatePicker}
  <div
    style="position: fixed; left: {datePickerPosition.x}px; top: {datePickerPosition.y}px; z-index: 1001; background: hsl(var(--popover)); border: 1px solid hsl(var(--border)); border-radius: var(--radius); box-shadow: 0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1); padding: 0.5rem;"
  >
    <Calendar
      type="single"
      bind:value={selectedCalendarDate}
      onValueChange={(v) => {
        console.log('[BaseNode] onValueChange fired with value:', v);
        if (v) {
          // Manually set the value since bind:value isn't working through the wrapper
          selectedCalendarDate = v;
        }
        // Close picker when date is selected (the $effect will handle the actual selection)
        showDatePicker = false;
      }}
    />
  </div>
{/if}

<style>
  .node {
    position: relative;
    padding: 0.25rem;
    padding-left: calc(
      var(--circle-offset, 26px) + var(--circle-diameter, 20px) + var(--circle-text-gap, 8px)
    ); /* Dynamic: circle position + circle width + gap */
    width: 100%;
    box-sizing: border-box; /* Include padding in width calculation */
    /*
      Circle positioning system using CSS-first dynamic calculation

      MATHEMATICAL FOUNDATION:
      The circle needs to be positioned at the visual center of the first line of text.

      Formula: containerPaddingPx + (fontSize × 0.75)
      - containerPaddingPx = 0.25rem (top padding of .node container)
      - 0.75 coefficient empirically calibrated for optical text center

      DERIVATION:
      1. Line-height creates vertical space above/below text (descenders/ascenders)
      2. Visual center of text is 75% of font-size from container top
      3. This accounts for typical font metrics (baseline, x-height, cap-height)
      4. Works consistently across different line-heights and font sizes

      BROWSER COMPATIBILITY:
      - CSS custom properties: IE 11+ (95%+ browser support)
      - CSS calc(): IE 9+ (98%+ browser support)
      - Graceful fallback to 1.05rem for older browsers
    */

    /* Default values for normal text - dynamic font-responsive alignment */
    --line-height: 1.5;
    --font-size: 1rem;
    /* Note: --icon-vertical-position is defined globally in app.css */
  }

  .node__indicator {
    position: absolute;
    left: var(--circle-offset, 26px); /* Dynamic circle positioning */
    /* Use shared CSS variable for vertical position - single source of truth */
    top: var(--icon-vertical-position);
    transform: translate(-50%, -50%); /* Center icon on coordinates */
    width: var(--circle-diameter, 20px); /* Dynamic circle size */
    height: var(--circle-diameter, 20px); /* Dynamic circle size */
    display: flex;
    align-items: center;
    justify-content: center;
    /* No transition - position updates should be instant */
  }

  /* Design system semantic classes for icon containers */
  .node-icon,
  .task-icon,
  .ai-icon {
    /* Classes applied dynamically based on node type */
    /* Base styling inherited from .node__indicator */
    position: relative;
  }

  .node__content {
    flex: 1;
    min-height: 1.25rem;
    padding: 0;
    border: none;
    border-radius: 0;
    background: transparent;
    font-family: inherit;
    font-size: 1rem;
    line-height: 1.5; /* Unified line-height: 16px * 1.5 = 24px (prevents layout shift between modes) */
    color: hsl(var(--foreground));
    outline: none;
    white-space: pre-wrap;
    word-wrap: break-word;
    box-sizing: border-box; /* Match textarea box model to prevent subpixel shifts */
    transform: translateZ(0); /* Force GPU rendering for pixel-perfect positioning */
  }

  .node__content--textarea {
    /* Textarea-specific styles */
    resize: none;
    overflow: hidden;
    width: 100%;
    /* Explicitly set line-height to match view div */
    line-height: 1.5;
    box-sizing: border-box;
    /* Match view div display to prevent layout shift */
    display: block;
    /* Always maintain min-height to match empty view divs (line-height) */
    min-height: 1.5rem;
  }

  .node__content--view {
    /* View mode styles */
    cursor: text;
    /* Override flex-basis to take full available width, not just content size */
    flex: 1 1 100%;
  }

  .node__content:empty,
  .node__content--view:empty {
    /* Ensure empty nodes maintain their height and are clickable */
    /* Use line-height (24px / 1.5rem) to match textarea's single-line height */
    min-height: 1.5rem;
    cursor: text;
  }

  /* Markdown formatting styles */
  :global(.markdown-syntax) {
    /* Wrapper for consistent double-click selection behavior */
    display: inline;
    word-break: keep-all;
  }

  :global(.markdown-bold) {
    font-weight: 700;
    /* Make bold more visible in monospace */
    text-shadow: 0.5px 0 0 currentColor;
  }

  :global(.markdown-italic) {
    font-style: italic;
  }

  :global(.markdown-underline) {
    text-decoration: underline;
  }

  :global(.markdown-code-inline) {
    /* Current inline code styling (used by marked-config.ts) */
    /* Matches code-block styling for consistency */
    background: hsl(var(--muted));
    padding: 0.125rem 0.25rem;
    border-radius: var(--radius);
    font-family: 'SF Mono', Monaco, 'Cascadia Code', 'Roboto Mono', Consolas, monospace;
    font-size: 0.875rem;
  }

  :global(.markdown-code),
  :global(.ns-markdown-code) {
    /* Legacy inline code styling (from markdown-utils.ts custom parser) */
    /* TODO: Consolidate with .markdown-code-inline once markdown-utils.ts is deprecated */
    font-family: 'Fira Code', 'Monaco', 'Menlo', 'Ubuntu Mono', monospace;
    font-size: 0.9em;
    background-color: hsl(var(--muted));
    border: 1px solid hsl(var(--border));
    border-radius: 3px;
    padding: 0.1em 0.3em;
    color: hsl(var(--foreground));
  }

  :global(.markdown-strikethrough) {
    text-decoration: line-through;
    opacity: 0.7;
  }

  /* Node colors now managed globally in app.css - Subtle Tint System */
</style>
