<!--
  AutocompleteModalDemo - Demo Component for AutocompleteModal
  
  Demonstrates how to integrate the AutocompleteModal with the @ trigger system
  in a TextNode or other contenteditable context.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import AutocompleteModal from './AutocompleteModal.svelte';
  import Card from '$lib/components/ui/card/card.svelte';
  import CardContent from '$lib/components/ui/card/card-content.svelte';
  import CardHeader from '$lib/components/ui/card/card-header.svelte';
  import CardTitle from '$lib/components/ui/card/card-title.svelte';
  import Button from '$lib/components/ui/button/button.svelte';

  // Service imports
  import NodeReferenceService from '$lib/services/NodeReferenceService';
  import { EnhancedNodeManager } from '$lib/services/EnhancedNodeManager';
  import { HierarchyService } from '$lib/services/HierarchyService';
  import { NodeOperationsService } from '$lib/services/NodeOperationsService';
  import { MockDatabaseService } from '$lib/services/MockDatabaseService';
  import ContentProcessor from '$lib/services/contentProcessor';

  // Demo state
  let demoText = 'Try typing @ to see the autocomplete modal in action!';
  let modalVisible = false;
  let modalPosition = { x: 200, y: 150 };
  let currentQuery = '';
  let contentEditableElement: HTMLDivElement | undefined = undefined;

  // Services
  let databaseService: MockDatabaseService;
  let nodeManager: EnhancedNodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let nodeReferenceService: NodeReferenceService;
  let contentProcessor: ContentProcessor;

  // Demo nodes for testing
  const demoNodes = [
    {
      id: 'node-1',
      type: 'text',
      content: '# Project Planning\n\nThis is our main project planning document.'
    },
    { id: 'node-2', type: 'task', content: '- [ ] Complete the user interface design' },
    {
      id: 'node-3',
      type: 'text',
      content: '## Meeting Notes\n\nDiscussed the new feature requirements.'
    },
    { id: 'node-4', type: 'user', content: 'John Doe - Team Lead' },
    { id: 'node-5', type: 'date', content: '2024-01-15 - Project Deadline' }
  ];

  // Initialize services
  onMount(async () => {
    try {
      // Initialize services in dependency order
      databaseService = new MockDatabaseService();
      nodeManager = new EnhancedNodeManager();
      hierarchyService = new HierarchyService(nodeManager);
      nodeOperationsService = new NodeOperationsService(
        nodeManager,
        hierarchyService,
        databaseService
      );
      contentProcessor = ContentProcessor.getInstance();

      nodeReferenceService = new NodeReferenceService(
        nodeManager,
        hierarchyService,
        nodeOperationsService,
        databaseService,
        contentProcessor
      );

      // Add demo nodes
      for (const node of demoNodes) {
        await databaseService.insertNode({
          id: node.id,
          type: node.type,
          content: node.content,
          parent_id: null,
          root_id: node.id,
          before_sibling_id: null,
          created_at: new Date().toISOString(),
          mentions: [],
          metadata: {},
          embedding_vector: null
        });

        // Note: Demo nodes are stored only in database for autocomplete
        // They are not part of the hierarchical node tree managed by NodeManager
      }

      console.log('AutocompleteModal demo initialized successfully');
    } catch (error) {
      console.error('Failed to initialize AutocompleteModal demo:', error);
    }
  });

  // Handle @ trigger detection
  function handleInput(event: Event): void {
    const target = event.target as HTMLDivElement;
    const content = target.textContent || '';
    const selection = window.getSelection();

    if (!selection || !nodeReferenceService) return;

    const cursorPosition = getCursorPosition(target, selection);
    const triggerContext = nodeReferenceService.detectTrigger(content, cursorPosition);

    if (triggerContext && triggerContext.isValid) {
      // Show modal at cursor position
      const range = selection.getRangeAt(0);
      const rangeRect = range.getBoundingClientRect();

      modalPosition = {
        x: rangeRect.left,
        y: rangeRect.bottom + 5
      };

      currentQuery = triggerContext.query;
      modalVisible = true;
    } else {
      modalVisible = false;
      currentQuery = '';
    }
  }

  // Get cursor position in text content
  function getCursorPosition(element: HTMLElement, selection: Selection): number {
    if (selection.rangeCount === 0) return 0;

    const range = selection.getRangeAt(0);
    const textContent = element.textContent || '';
    let position = 0;

    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null, false);

    let node;
    while ((node = walker.nextNode())) {
      if (node === range.startContainer) {
        return position + range.startOffset;
      }
      position += node.textContent?.length || 0;
    }

    return textContent.length;
  }

  // Handle node selection from modal
  function handleNodeSelect(event: CustomEvent): void {
    const { node } = event.detail;

    if (node.type === 'create') {
      console.log('Creating new node:', node.content);
      // In a real implementation, this would create the node and insert the reference
    } else {
      console.log('Selected existing node:', node.id, node.content);
      // In a real implementation, this would insert the node reference
    }

    // Insert a placeholder reference for demo purposes
    if (contentEditableElement) {
      const selection = window.getSelection();
      if (selection && selection.rangeCount > 0) {
        const range = selection.getRangeAt(0);
        range.deleteContents();

        const referenceText =
          node.type === 'create' ? `@${node.content}` : `@${extractNodeTitle(node.content)}`;

        range.insertNode(document.createTextNode(referenceText));
        range.collapse(false);
        selection.removeAllRanges();
        selection.addRange(range);
      }
    }

    modalVisible = false;
  }

  // Handle modal close
  function handleModalClose(): void {
    modalVisible = false;
    currentQuery = '';
  }

  // Extract title from node content
  function extractNodeTitle(content: string): string {
    const lines = content.split('\n');
    const firstLine = lines[0].trim();

    // Remove markdown header syntax
    const headerMatch = firstLine.match(/^#{1,6}\s*(.*)$/);
    if (headerMatch) {
      return headerMatch[1].trim() || 'Untitled';
    }

    return firstLine.substring(0, 30) || 'Untitled';
  }

  // Demo functions
  function showModalAt(x: number, y: number, query: string): void {
    modalPosition = { x, y };
    currentQuery = query;
    modalVisible = true;
  }

  function hideModal(): void {
    modalVisible = false;
    currentQuery = '';
  }
</script>

<!-- Demo Interface -->
<div class="p-6 space-y-6 max-w-4xl mx-auto">
  <Card>
    <CardHeader>
      <CardTitle>AutocompleteModal Demo</CardTitle>
    </CardHeader>
    <CardContent class="space-y-4">
      <p class="text-muted-foreground">
        This demo shows the AutocompleteModal component in action. Try typing @ in the editor below
        or use the demo buttons to see different scenarios.
      </p>

      <!-- Demo Controls -->
      <div class="flex gap-2 flex-wrap">
        <Button variant="outline" size="sm" on:click={() => showModalAt(200, 200, 'project')}>
          Demo: @project
        </Button>

        <Button variant="outline" size="sm" on:click={() => showModalAt(300, 250, 'task')}>
          Demo: @task
        </Button>

        <Button variant="outline" size="sm" on:click={() => showModalAt(150, 300, 'meeting')}>
          Demo: @meeting
        </Button>

        <Button variant="outline" size="sm" on:click={() => showModalAt(250, 180, 'nonexistent')}>
          Demo: @nonexistent
        </Button>

        <Button variant="outline" size="sm" on:click={hideModal}>Hide Modal</Button>
      </div>

      <!-- Interactive Text Editor -->
      <div class="space-y-2">
        <label class="text-sm font-medium" for="interactive-editor"
          >Interactive Editor (Type @ to trigger autocomplete):</label
        >
        <div
          bind:this={contentEditableElement}
          id="interactive-editor"
          contenteditable="true"
          class="min-h-32 p-4 border border-border rounded-lg bg-background focus:outline-none focus:ring-2 focus:ring-ring"
          on:input={handleInput}
          on:keyup={handleInput}
          role="textbox"
          tabindex="0"
        >
          {demoText}
        </div>
      </div>

      <!-- Status Information -->
      <div class="text-sm text-muted-foreground space-y-1">
        <div><strong>Modal Visible:</strong> {modalVisible}</div>
        <div><strong>Current Query:</strong> "{currentQuery}"</div>
        <div><strong>Position:</strong> x:{modalPosition.x}, y:{modalPosition.y}</div>
      </div>
    </CardContent>
  </Card>
</div>

<!-- AutocompleteModal Instance -->
{#if nodeReferenceService}
  <AutocompleteModal
    visible={modalVisible}
    position={modalPosition}
    query={currentQuery}
    {nodeReferenceService}
    on:nodeSelect={handleNodeSelect}
    on:close={handleModalClose}
  />
{/if}
