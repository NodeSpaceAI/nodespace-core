<!--
  BaseNodeAutocompleteDemo - Comprehensive Integration Demo
  
  Demonstrates the complete @ trigger integration between BaseNode and AutocompleteModal
  components, showcasing real-time autocomplete functionality in a contenteditable context.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import BaseNode from '$lib/design/components/BaseNode.svelte';
  import Card from '$lib/components/ui/card/card.svelte';
  import CardContent from '$lib/components/ui/card/card-content.svelte';
  import CardHeader from '$lib/components/ui/card/card-header.svelte';
  import CardTitle from '$lib/components/ui/card/card-title.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import Separator from '$lib/components/ui/separator/separator.svelte';

  // Service imports
  import NodeReferenceService from '$lib/services/NodeReferenceService';
  import { EnhancedNodeManager } from '$lib/services/EnhancedNodeManager';
  import { HierarchyService } from '$lib/services/HierarchyService';
  import { NodeOperationsService } from '$lib/services/NodeOperationsService';
  import { MockDatabaseService } from '$lib/services/MockDatabaseService';
  import { ContentProcessor } from '$lib/services/contentProcessor';

  // Demo state
  let nodes = [
    {
      id: 'demo-node-1',
      content: 'Try typing @ in this node to see autocomplete in action!',
      nodeType: 'text',
      autoFocus: false
    },
    {
      id: 'demo-node-2',
      content: '## Meeting Notes\n\nType @ to reference other nodes in your content.',
      nodeType: 'text',
      autoFocus: false
    },
    {
      id: 'demo-node-3',
      content: '- [ ] Complete task with @project references',
      nodeType: 'task',
      autoFocus: false
    }
  ];

  // Services
  let databaseService: MockDatabaseService;
  let nodeManager: EnhancedNodeManager;
  let hierarchyService: HierarchyService;
  let nodeOperationsService: NodeOperationsService;
  let nodeReferenceService: NodeReferenceService;
  let contentProcessor: ContentProcessor;

  // Demo status
  let servicesInitialized = false;
  let initializationError: string | null = null;

  // Demo reference nodes for autocomplete
  const referenceNodes = [
    {
      id: 'ref-1',
      type: 'text',
      content:
        '# Project Planning Document\n\nThis is our main project planning document with all the key information.'
    },
    { id: 'ref-2', type: 'task', content: '- [ ] Complete the user interface design by Friday' },
    {
      id: 'ref-3',
      type: 'text',
      content:
        '## Meeting Notes - 2024-01-15\n\nDiscussed the new feature requirements and timeline.'
    },
    {
      id: 'ref-4',
      type: 'user',
      content:
        'John Doe - Team Lead\n\nResponsible for overall project coordination and technical decisions.'
    },
    {
      id: 'ref-5',
      type: 'date',
      content: '2024-01-30 - Project Deadline\n\nFinal delivery date for the complete project.'
    },
    {
      id: 'ref-6',
      type: 'ai_chat',
      content:
        'AI Assistant Conversation\n\nDiscussion about implementation strategies and best practices.'
    },
    {
      id: 'ref-7',
      type: 'entity',
      content: 'NodeSpace Application\n\nThe main software project being developed.'
    },
    {
      id: 'ref-8',
      type: 'query',
      content:
        'Search: contenteditable integration\n\nQuery for finding relevant documentation and examples.'
    }
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

      // Add reference nodes to database for autocomplete
      for (const refNode of referenceNodes) {
        await databaseService.insertNode({
          id: refNode.id,
          type: refNode.type,
          content: refNode.content,
          parent_id: null,
          root_id: refNode.id,
          before_sibling_id: null,
          created_at: new Date().toISOString(),
          mentions: [],
          metadata: {},
          embedding_vector: null
        });

        // Note: Reference nodes are stored only in database for autocomplete
        // They are not part of the hierarchical node tree managed by NodeManager
      }

      servicesInitialized = true;
      console.log('BaseNodeAutocompleteDemo: Services initialized successfully');
    } catch (error) {
      console.error('BaseNodeAutocompleteDemo: Failed to initialize services:', error);
      initializationError = error instanceof Error ? error.message : 'Unknown error';
    }
  });

  // Event handlers
  function handleContentChanged(nodeId: string, event: CustomEvent<{ content: string }>): void {
    const { content } = event.detail;
    console.log(`Node ${nodeId} content changed:`, content);

    // Update the node in our local state
    const nodeIndex = nodes.findIndex((n) => n.id === nodeId);
    if (nodeIndex !== -1) {
      nodes[nodeIndex].content = content;
    }
  }

  function handleNodeReferenceSelected(
    nodeId: string,
    event: CustomEvent<{ nodeId: string; nodeTitle: string }>
  ): void {
    const { nodeId: referencedNodeId, nodeTitle } = event.detail;
    console.log(`Node ${nodeId} selected reference:`, referencedNodeId, nodeTitle);
  }

  function handleHeaderLevelChanged(nodeId: string, event: CustomEvent<{ level: number }>): void {
    const { level } = event.detail;
    console.log(`Node ${nodeId} header level changed:`, level);
  }

  function focusNode(nodeId: string): void {
    const nodeIndex = nodes.findIndex((n) => n.id === nodeId);
    if (nodeIndex !== -1) {
      nodes[nodeIndex].autoFocus = true;
      // Reset autoFocus after a short delay
      setTimeout(() => {
        nodes[nodeIndex].autoFocus = false;
      }, 100);
    }
  }

  function addNewNode(): void {
    const newId = `demo-node-${Date.now()}`;
    nodes.push({
      id: newId,
      content: '',
      nodeType: 'text',
      autoFocus: true
    });
    nodes = [...nodes]; // Trigger reactivity
  }

  function clearAllNodes(): void {
    nodes = nodes.map((node) => ({ ...node, content: '', autoFocus: false }));
  }
</script>

<!-- Demo Interface -->
<div class="p-6 space-y-6 max-w-4xl mx-auto">
  <!-- Header -->
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-2">
        <span class="text-2xl">@</span>
        BaseNode + AutocompleteModal Integration Demo
      </CardTitle>
    </CardHeader>
    <CardContent class="space-y-4">
      <p class="text-muted-foreground">
        This demo showcases the complete integration between BaseNode components and the
        AutocompleteModal. Type @ in any of the editable nodes below to trigger the autocomplete
        system.
      </p>

      <!-- Initialization Status -->
      {#if !servicesInitialized && !initializationError}
        <div class="flex items-center gap-2 text-muted-foreground">
          <div
            class="animate-spin h-4 w-4 border-2 border-primary border-t-transparent rounded-full"
          ></div>
          <span>Initializing services...</span>
        </div>
      {:else if initializationError}
        <div class="p-4 bg-destructive/10 border border-destructive/20 rounded-lg">
          <p class="text-destructive font-medium">Initialization Error</p>
          <p class="text-sm text-destructive/80 mt-1">{initializationError}</p>
        </div>
      {:else}
        <div class="flex items-center gap-2 text-green-600 dark:text-green-400">
          <div class="w-2 h-2 bg-green-500 rounded-full"></div>
          <span class="text-sm">Services initialized successfully</span>
        </div>
      {/if}

      <!-- Demo Controls -->
      <div class="flex gap-2 flex-wrap">
        <Button
          variant="outline"
          size="sm"
          on:click={() => focusNode('demo-node-1')}
          disabled={!servicesInitialized}
        >
          Focus First Node
        </Button>

        <Button variant="outline" size="sm" on:click={addNewNode} disabled={!servicesInitialized}>
          Add New Node
        </Button>

        <Button
          variant="outline"
          size="sm"
          on:click={clearAllNodes}
          disabled={!servicesInitialized}
        >
          Clear All Content
        </Button>
      </div>
    </CardContent>
  </Card>

  <!-- Reference Nodes Information -->
  <Card>
    <CardHeader>
      <CardTitle class="text-lg">Available Reference Nodes</CardTitle>
    </CardHeader>
    <CardContent>
      <p class="text-muted-foreground text-sm mb-4">
        The following nodes are available for autocomplete when you type @:
      </p>

      <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
        {#each referenceNodes as refNode}
          <div class="flex items-start gap-3 p-3 bg-muted/30 rounded-lg">
            <div
              class="flex-shrink-0 w-8 h-8 flex items-center justify-center bg-background border rounded"
            >
              {#if refNode.type === 'text'}📄
              {:else if refNode.type === 'task'}☐
              {:else if refNode.type === 'user'}👤
              {:else if refNode.type === 'date'}📅
              {:else if refNode.type === 'ai_chat'}🤖
              {:else if refNode.type === 'entity'}🏷️
              {:else if refNode.type === 'query'}🔍
              {:else}📄{/if}
            </div>

            <div class="flex-1 min-w-0">
              <div class="font-medium text-sm truncate">
                {refNode.content.split('\n')[0].replace(/^#{1,6}\s*/, '')}
              </div>
              <div class="text-xs text-muted-foreground capitalize mt-1">
                {refNode.type.replace('_', ' ')} • ID: {refNode.id}
              </div>
            </div>
          </div>
        {/each}
      </div>
    </CardContent>
  </Card>

  <!-- Interactive Demo Nodes -->
  <Card>
    <CardHeader>
      <CardTitle class="text-lg">Interactive Demo Nodes</CardTitle>
    </CardHeader>
    <CardContent class="space-y-4">
      <p class="text-muted-foreground text-sm">
        Click on any node to edit it, then type @ to trigger the autocomplete modal. The modal will
        show relevant nodes that you can select or create new ones.
      </p>

      <Separator />

      <!-- Demo Nodes -->
      <div class="space-y-4">
        {#each nodes as node (node.id)}
          <div class="border border-border/50 rounded-lg p-4 bg-background/50">
            <div class="text-xs text-muted-foreground mb-2 flex items-center justify-between">
              <span>Node ID: {node.id}</span>
              <span class="capitalize">{node.nodeType}</span>
            </div>

            {#if servicesInitialized}
              <BaseNode
                nodeId={node.id}
                nodeType={node.nodeType}
                content={node.content}
                autoFocus={node.autoFocus}
                {nodeReferenceService}
                on:contentChanged={(e) => handleContentChanged(node.id, e)}
                on:headerLevelChanged={(e) => handleHeaderLevelChanged(node.id, e)}
                on:nodeReferenceSelected={(e) => handleNodeReferenceSelected(node.id, e)}
              />
            {:else}
              <div class="text-muted-foreground italic">Waiting for services to initialize...</div>
            {/if}
          </div>
        {/each}
      </div>
    </CardContent>
  </Card>

  <!-- Usage Instructions -->
  <Card>
    <CardHeader>
      <CardTitle class="text-lg">How to Use</CardTitle>
    </CardHeader>
    <CardContent class="space-y-3">
      <div class="space-y-2 text-sm">
        <div class="flex items-start gap-3">
          <span
            class="flex-shrink-0 w-6 h-6 bg-primary text-primary-foreground rounded-full flex items-center justify-center text-xs font-bold"
            >1</span
          >
          <div>
            <p class="font-medium">Type @ to trigger autocomplete</p>
            <p class="text-muted-foreground">
              Click on any node above and type @ to see the autocomplete modal appear.
            </p>
          </div>
        </div>

        <div class="flex items-start gap-3">
          <span
            class="flex-shrink-0 w-6 h-6 bg-primary text-primary-foreground rounded-full flex items-center justify-center text-xs font-bold"
            >2</span
          >
          <div>
            <p class="font-medium">Search and navigate</p>
            <p class="text-muted-foreground">
              Continue typing to filter results, use arrow keys to navigate, Enter to select.
            </p>
          </div>
        </div>

        <div class="flex items-start gap-3">
          <span
            class="flex-shrink-0 w-6 h-6 bg-primary text-primary-foreground rounded-full flex items-center justify-center text-xs font-bold"
            >3</span
          >
          <div>
            <p class="font-medium">Select or create</p>
            <p class="text-muted-foreground">
              Choose an existing node or create a new one. The reference will be inserted as a
              markdown link.
            </p>
          </div>
        </div>

        <div class="flex items-start gap-3">
          <span
            class="flex-shrink-0 w-6 h-6 bg-primary text-primary-foreground rounded-full flex items-center justify-center text-xs font-bold"
            >4</span
          >
          <div>
            <p class="font-medium">Continue editing</p>
            <p class="text-muted-foreground">
              The cursor will be positioned after the inserted reference, ready for more content.
            </p>
          </div>
        </div>
      </div>
    </CardContent>
  </Card>
</div>
