<!--
  NodeReferenceDemo - Comprehensive Demonstration of BaseNode Decoration System
  
  Showcases all node types with rich decorations, different display contexts,
  and interactive features. This component serves as both a demo and a test
  environment for the complete Phase 2.2 implementation.
-->

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { writable } from 'svelte/store';
  import Card from '$lib/components/ui/card/card.svelte';
  import CardContent from '$lib/components/ui/card/card-content.svelte';
  import CardHeader from '$lib/components/ui/card/card-header.svelte';
  import CardTitle from '$lib/components/ui/card/card-title.svelte';
  import Button from '$lib/components/ui/button/button.svelte';
  import Separator from '$lib/components/ui/separator/separator.svelte';
  import { NodeReferenceService } from '$lib/services/NodeReferenceService';
  import {
    NodeReferenceRenderer,
    initializeNodeReferenceRenderer
  } from '$lib/services/NodeReferenceRenderer';
  import { decorationCoordinator } from '$lib/services/DecorationCoordinator';
  import { EnhancedNodeManager } from '$lib/services/EnhancedNodeManager';
  import { HierarchyService } from '$lib/services/HierarchyService';
  import { NodeOperationsService } from '$lib/services/NodeOperationsService';
  import { MockDatabaseService, type NodeSpaceNode } from '$lib/services/MockDatabaseService';

  // ============================================================================
  // Component State
  // ============================================================================

  let mounted = false;
  let nodeReferenceService: NodeReferenceService;
  let nodeReferenceRenderer: NodeReferenceRenderer;
  // Svelte bind:this assignment - populated by framework at runtime
  let demoContainer: HTMLDivElement | undefined = undefined;
  let status = writable('Initializing...');
  let selectedContext = writable<'inline' | 'popup' | 'preview'>('inline');

  // Demo data
  let demoNodes: NodeSpaceNode[] = [];
  let renderMetrics = writable({
    totalReferences: 0,
    renderedReferences: 0,
    viewportReferences: 0,
    renderTime: 0
  });

  // ============================================================================
  // Service Initialization
  // ============================================================================

  onMount(async () => {
    try {
      status.set('Initializing services...');

      // Initialize core services
      const databaseService = new MockDatabaseService();
      const nodeManager = new EnhancedNodeManager(databaseService);
      const hierarchyService = new HierarchyService(nodeManager, databaseService);
      const nodeOperationsService = new NodeOperationsService(
        nodeManager,
        hierarchyService,
        databaseService
      );

      // Initialize node reference service
      nodeReferenceService = new NodeReferenceService(
        nodeManager,
        hierarchyService,
        nodeOperationsService,
        databaseService
      );

      // Initialize renderer
      nodeReferenceRenderer = initializeNodeReferenceRenderer(nodeReferenceService);

      status.set('Creating demo data...');
      await createDemoData();

      status.set('Setting up interaction handlers...');
      setupInteractionHandlers();

      status.set('Ready - Try the interactive decorations!');
      mounted = true;

      // Initial render
      await renderAllExamples();
      updateMetrics();
    } catch (error) {
      console.error('NodeReferenceDemo: Initialization failed', error);
      status.set(`Error: ${error instanceof Error ? error.message : 'Unknown error'}`);
    }
  });

  onDestroy(() => {
    if (nodeReferenceRenderer) {
      nodeReferenceRenderer.cleanup();
    }
  });

  // ============================================================================
  // Demo Data Creation
  // ============================================================================

  async function createDemoData(): Promise<void> {
    if (!nodeReferenceService) return;

    // Create demo nodes of different types
    const demoData = [
      {
        type: 'task',
        content: `# Complete Project Documentation
status: pending
priority: high
due_date: 2024-12-31

Need to finish the comprehensive documentation for the NodeSpace project including:
- API documentation
- User guides  
- Architecture overview`
      },
      {
        type: 'user',
        content: `# Alice Johnson
email: alice@nodespace.com
role: admin
department: Engineering

Lead frontend developer working on the NodeSpace UI components and user experience.`
      },
      {
        type: 'date',
        content: `# Project Deadline
2024-12-31

Final deadline for the NodeSpace Phase 2 implementation including all core features.`
      },
      {
        type: 'document',
        content: `# API Specification Document
type: pdf
size: 2.4MB
version: 3.1

Comprehensive API documentation covering all NodeSpace backend endpoints and data models.`
      },
      {
        type: 'ai_chat',
        content: `# Chat: NodeSpace Architecture Discussion
model: Claude 3.5 Sonnet
messages: 47
last_activity: 2024-08-21T14:30:00Z

Ongoing discussion about the optimal architecture patterns for the NodeSpace knowledge management system.`
      },
      {
        type: 'entity',
        content: `# NodeSpace Project
category: software
status: in-development

AI-native knowledge management system built with modern web technologies.`
      }
    ];

    demoNodes = [];
    for (const data of demoData) {
      try {
        const node = await nodeReferenceService.createNode(data.type, data.content);
        demoNodes.push(node);
      } catch (error) {
        console.error('Error creating demo node:', error);
      }
    }
  }

  // ============================================================================
  // Interaction Handlers
  // ============================================================================

  function setupInteractionHandlers(): void {
    // Handle decoration clicks
    const clickUnsubscribe = decorationCoordinator.registerClickHandler({
      decorationType: '*', // Handle all types
      handler: async (event) => {
        console.log('NodeReferenceDemo: Decoration clicked', event);

        // For demo purposes, show different actions based on node type
        const nodeId = event.nodeId;
        const node = demoNodes.find((n) => n.id === nodeId);

        if (node) {
          switch (node.type) {
            case 'task':
              await toggleTaskStatus(node);
              break;
            case 'user':
              showUserProfile(node);
              break;
            case 'date':
              showDateDetails(node);
              break;
            case 'document':
              openDocument(node);
              break;
            case 'ai_chat':
              openAIChat(node);
              break;
            default:
              showNodeDetails(node);
          }
        }
      },
      priority: 10
    });

    // Handle decoration hover
    const hoverUnsubscribe = decorationCoordinator.registerHoverHandler({
      decorationType: '*',
      handler: (event) => {
        if (event.hoverState === 'enter') {
          status.set(`Hovering over ${event.decorationType} reference: ${event.nodeId}`);
        } else {
          status.set('Ready - Try the interactive decorations!');
        }
      },
      priority: 5
    });

    // Store unsubscribe functions for cleanup
    if (demoContainer) {
      (
        demoContainer as HTMLElement & { _unsubscribeFunctions?: (() => void)[] }
      )._unsubscribeFunctions = [clickUnsubscribe, hoverUnsubscribe];
    }
  }

  // ============================================================================
  // Node Interaction Handlers
  // ============================================================================

  async function toggleTaskStatus(node: NodeSpaceNode): Promise<void> {
    const currentStatus = node.content.includes('status: completed') ? 'completed' : 'pending';
    const newStatus = currentStatus === 'completed' ? 'pending' : 'completed';

    // Update node content
    const updatedContent = node.content.replace(/status: \w+/, `status: ${newStatus}`);

    // Update in database (mock)
    node.content = updatedContent;

    // Re-render the decoration
    await nodeReferenceRenderer.updateDecoration(node.id, 'content-changed');

    status.set(`Task ${node.id} marked as ${newStatus}`);
  }

  function showUserProfile(node: NodeSpaceNode): void {
    const title = node.content.split('\n')[0].replace(/^# /, '');
    status.set(`Opening profile for ${title}`);
  }

  function showDateDetails(node: NodeSpaceNode): void {
    const title = node.content.split('\n')[0].replace(/^# /, '');
    status.set(`Showing details for ${title}`);
  }

  function openDocument(node: NodeSpaceNode): void {
    const title = node.content.split('\n')[0].replace(/^# /, '');
    status.set(`Opening document: ${title}`);
  }

  function openAIChat(node: NodeSpaceNode): void {
    const title = node.content.split('\n')[0].replace(/^# /, '');
    status.set(`Opening AI chat: ${title}`);
  }

  function showNodeDetails(node: NodeSpaceNode): void {
    const title = node.content.split('\n')[0].replace(/^# /, '');
    status.set(`Showing details for ${title}`);
  }

  // ============================================================================
  // Rendering Functions
  // ============================================================================

  async function renderAllExamples(): Promise<void> {
    if (!demoContainer || !nodeReferenceRenderer) return;

    try {
      await nodeReferenceRenderer.renderContainer(demoContainer, {
        displayContext: $selectedContext,
        viewportOptimization: false,
        batchSize: 10,
        debounceMs: 0
      });

      updateMetrics();
    } catch (error) {
      console.error('Error rendering examples:', error);
      status.set('Error rendering decorations');
    }
  }

  async function changeDisplayContext(context: 'inline' | 'popup' | 'preview'): Promise<void> {
    selectedContext.set(context);
    if (mounted) {
      await renderAllExamples();
    }
  }

  function updateMetrics(): void {
    if (nodeReferenceRenderer) {
      const metrics = nodeReferenceRenderer.getMetrics();
      renderMetrics.set(metrics);
    }
  }

  async function forceRefresh(): Promise<void> {
    if (nodeReferenceRenderer) {
      nodeReferenceRenderer.clearCache();
      await renderAllExamples();
      status.set('Decorations refreshed');
    }
  }

  // ============================================================================
  // Demo Content Generation
  // ============================================================================

  function generateDemoContent(): string {
    if (!demoNodes.length) return 'Loading demo content...';

    const taskNode = demoNodes.find((n) => n.type === 'task');
    const userNode = demoNodes.find((n) => n.type === 'user');
    const dateNode = demoNodes.find((n) => n.type === 'date');
    const docNode = demoNodes.find((n) => n.type === 'document');
    const aiNode = demoNodes.find((n) => n.type === 'ai_chat');
    const entityNode = demoNodes.find((n) => n.type === 'entity');

    return `
# NodeSpace Phase 2.2 - BaseNode Decoration System Demo

This demo showcases the rich visual decorations for different node types in the universal node reference system.

## Task References
Here's a task that needs completion: nodespace://node/${taskNode?.id || 'task-1'}
Click the checkbox to toggle between pending and completed states.

## User References  
Assigned to team lead: nodespace://node/${userNode?.id || 'user-1'}
Notice the avatar, online status indicator, and role badge.

## Date References
Project deadline is approaching: nodespace://node/${dateNode?.id || 'date-1'}
Shows relative time and special highlighting for today's date.

## Document References
See the full specification: nodespace://node/${docNode?.id || 'doc-1'}
Displays file type icon, preview, and metadata.

## AI Chat References
Previous discussion: nodespace://node/${aiNode?.id || 'ai-1'}
Shows model information, message count, and activity status.

## Entity References
This is part of the larger: nodespace://node/${entityNode?.id || 'entity-1'}
Displays categorization and status information.

## Interactive Features
- **Click** any reference to see type-specific actions
- **Hover** for status updates
- **Keyboard navigation** with Tab and Enter/Space
- **Different contexts** change the decoration complexity
- **Accessibility** support with proper ARIA labels

Try changing the display context above to see how decorations adapt!
    `.trim();
  }
</script>

<!-- ============================================================================ -->
<!-- Template -->
<!-- ============================================================================ -->

<div class="node-reference-demo p-6 space-y-6">
  <!-- Header -->
  <Card>
    <CardHeader>
      <CardTitle class="flex items-center gap-3">
        <span class="text-2xl">🎨</span>
        BaseNode Decoration System Demo
      </CardTitle>
    </CardHeader>
    <CardContent>
      <div class="flex flex-col gap-4">
        <!-- Status -->
        <div class="flex items-center gap-2">
          <span class="text-sm text-muted-foreground">Status:</span>
          <span class="text-sm font-medium">{$status}</span>
        </div>

        <!-- Controls -->
        <div class="flex flex-wrap gap-3">
          <div class="flex items-center gap-2">
            <span class="text-sm text-muted-foreground">Display Context:</span>
            <div class="flex gap-1">
              <Button
                variant={$selectedContext === 'inline' ? 'default' : 'outline'}
                size="sm"
                on:click={() => changeDisplayContext('inline')}
              >
                Inline
              </Button>
              <Button
                variant={$selectedContext === 'popup' ? 'default' : 'outline'}
                size="sm"
                on:click={() => changeDisplayContext('popup')}
              >
                Popup
              </Button>
              <Button
                variant={$selectedContext === 'preview' ? 'default' : 'outline'}
                size="sm"
                on:click={() => changeDisplayContext('preview')}
              >
                Preview
              </Button>
            </div>
          </div>

          <Button variant="outline" size="sm" on:click={forceRefresh}>🔄 Refresh</Button>
        </div>

        <!-- Metrics -->
        <div class="flex flex-wrap gap-4 text-sm text-muted-foreground">
          <span>References: {$renderMetrics.totalReferences}</span>
          <span>Rendered: {$renderMetrics.renderedReferences}</span>
          <span>Viewport: {$renderMetrics.viewportReferences}</span>
          <span>Render Time: {$renderMetrics.renderTime.toFixed(1)}ms</span>
        </div>
      </div>
    </CardContent>
  </Card>

  <Separator />

  <!-- Demo Content -->
  <Card>
    <CardHeader>
      <CardTitle>Interactive Node References</CardTitle>
    </CardHeader>
    <CardContent>
      <div
        bind:this={demoContainer}
        class="prose prose-sm max-w-none dark:prose-invert"
        style="white-space: pre-wrap; line-height: 1.6;"
      >
        {generateDemoContent()}
      </div>
    </CardContent>
  </Card>

  <!-- Legend -->
  <Card>
    <CardHeader>
      <CardTitle>Decoration Features</CardTitle>
    </CardHeader>
    <CardContent>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-sm">
        <div>
          <h4 class="font-semibold mb-2">Task Decorations</h4>
          <ul class="space-y-1 text-muted-foreground">
            <li>• Interactive checkboxes</li>
            <li>• Status indicators (pending/completed)</li>
            <li>• Priority markers</li>
            <li>• Due date display</li>
          </ul>
        </div>

        <div>
          <h4 class="font-semibold mb-2">User Decorations</h4>
          <ul class="space-y-1 text-muted-foreground">
            <li>• Avatar with initials</li>
            <li>• Online/offline status</li>
            <li>• Role indicators</li>
            <li>• Compact name display</li>
          </ul>
        </div>

        <div>
          <h4 class="font-semibold mb-2">Date Decorations</h4>
          <ul class="space-y-1 text-muted-foreground">
            <li>• Calendar icons</li>
            <li>• Relative time display</li>
            <li>• Today highlighting</li>
            <li>• Past/future indicators</li>
          </ul>
        </div>

        <div>
          <h4 class="font-semibold mb-2">Document Decorations</h4>
          <ul class="space-y-1 text-muted-foreground">
            <li>• File type icons</li>
            <li>• Content previews</li>
            <li>• Size and metadata</li>
            <li>• Type-specific styling</li>
          </ul>
        </div>

        <div>
          <h4 class="font-semibold mb-2">AI Chat Decorations</h4>
          <ul class="space-y-1 text-muted-foreground">
            <li>• Model information</li>
            <li>• Message counters</li>
            <li>• Activity indicators</li>
            <li>• Animated status</li>
          </ul>
        </div>

        <div>
          <h4 class="font-semibold mb-2">Accessibility Features</h4>
          <ul class="space-y-1 text-muted-foreground">
            <li>• Keyboard navigation</li>
            <li>• Screen reader support</li>
            <li>• High contrast mode</li>
            <li>• Reduced motion support</li>
          </ul>
        </div>
      </div>
    </CardContent>
  </Card>
</div>

<style>
  .node-reference-demo {
    max-width: 1200px;
    margin: 0 auto;
  }
</style>
