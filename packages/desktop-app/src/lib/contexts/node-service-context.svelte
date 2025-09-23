<!--
  NodeServiceContext - Service Context Provider for Node Components
  
  Provides centralized service initialization and management for all node components.
  This ensures BaseNode and its derived components can access services without
  requiring manual service passing through props.
-->

<script lang="ts" context="module">
  import { getContext, setContext } from 'svelte';

  // Context key for service access
  const NODE_SERVICE_CONTEXT_KEY = Symbol('nodeServices');

  // Import proper types for the services
  import type { ReactiveNodeService } from '$lib/services/reactiveNodeService.svelte';
  import type { HierarchyService } from '$lib/services/hierarchyService';
  import type { NodeOperationsService } from '$lib/services/nodeOperationsService';
  import type { ContentProcessor } from '$lib/services/contentProcessor';
  import type { MockDatabaseService } from '$lib/services/mockDatabaseService';
  import type NodeReferenceService from '$lib/services/nodeReferenceService';

  // Service interface definition with proper types
  export interface NodeServices {
    nodeReferenceService: NodeReferenceService;
    nodeManager: ReactiveNodeService;
    hierarchyService: HierarchyService;
    nodeOperationsService: NodeOperationsService;
    contentProcessor: ContentProcessor;
    databaseService: MockDatabaseService;
  }

  // Context accessor functions
  export function getNodeServices(): NodeServices | null {
    return getContext(NODE_SERVICE_CONTEXT_KEY) || null;
  }

  export function setNodeServices(services: NodeServices): void {
    setContext(NODE_SERVICE_CONTEXT_KEY, services);
  }
</script>

<script lang="ts">
  import { onMount } from 'svelte';

  // Service imports
  import NodeReferenceService from '$lib/services/nodeReferenceService';
  import { createReactiveNodeService } from '$lib/services/reactiveNodeService.svelte';
  import { HierarchyService } from '$lib/services/hierarchyService';
  import { NodeOperationsService } from '$lib/services/nodeOperationsService';
  import { MockDatabaseService } from '$lib/services/mockDatabaseService';
  import { ContentProcessor } from '$lib/services/contentProcessor';

  // Props - external reference only for service configuration
  export const initializationMode: 'full' | 'mock' = 'mock';

  // Services state
  let services: NodeServices | null = null;
  let servicesInitialized = false;
  let initializationError: string | null = null;

  // Initialize services on mount
  onMount(async () => {
    try {
      // Initialize services in dependency order
      const databaseService = new MockDatabaseService();

      // Create node manager events
      const nodeManagerEvents = {
        focusRequested: (nodeId: string, position: number) => {

          // Use DOM API to focus the node directly with cursor positioning
          setTimeout(() => {
            const nodeElement = document.querySelector(
              `[data-node-id="${nodeId}"] [contenteditable]`
            ) as HTMLElement;
            if (nodeElement) {
              nodeElement.focus();

              // Set cursor position using proven algorithm from working version
              if (position >= 0) {
                const selection = window.getSelection();
                if (!selection) return;

                const walker = document.createTreeWalker(nodeElement, NodeFilter.SHOW_TEXT, null);

                let currentOffset = 0;
                let currentNode;

                while ((currentNode = walker.nextNode())) {
                  const nodeLength = currentNode.textContent?.length || 0;

                  if (currentOffset + nodeLength >= position) {
                    const range = document.createRange();
                    const offsetInNode = position - currentOffset;
                    range.setStart(currentNode, Math.min(offsetInNode, nodeLength));
                    range.setEnd(currentNode, Math.min(offsetInNode, nodeLength));

                    selection.removeAllRanges();
                    selection.addRange(range);
                    return;
                  }

                  currentOffset += nodeLength;
                }

                // Fallback: place cursor at end
                const range = document.createRange();
                range.selectNodeContents(nodeElement);
                range.collapse(false);
                selection.removeAllRanges();
                selection.addRange(range);
              }

            } else {
              console.error(`❌ Could not find contenteditable element for node ${nodeId}`);
            }
          }, 10);
        },
        hierarchyChanged: () => {
          // Hierarchy change handling logic here if needed
        },
        nodeCreated: () => {
          // Node creation handling logic here if needed
        },
        nodeDeleted: () => {
          // Node deletion handling logic here if needed
        }
      };

      const nodeManager = createReactiveNodeService(nodeManagerEvents);

      // Initialize with demo data
      nodeManager.initializeWithRichDemoData();

      const hierarchyService = new HierarchyService(nodeManager);
      const contentProcessor = ContentProcessor.getInstance();
      const nodeOperationsService = new NodeOperationsService(
        nodeManager,
        hierarchyService,
        contentProcessor
      );

      const nodeReferenceService = new NodeReferenceService(
        nodeManager,
        hierarchyService,
        nodeOperationsService,
        databaseService,
        contentProcessor
      );

      // Create service bundle
      services = {
        nodeReferenceService,
        nodeManager,
        hierarchyService,
        nodeOperationsService,
        contentProcessor,
        databaseService
      };

      // Set context for child components
      setNodeServices(services);

      servicesInitialized = true;
    } catch (error) {
      console.error('NodeServiceContext: Failed to initialize services:', error);
      initializationError = error instanceof Error ? error.message : 'Unknown error';
    }
  });
</script>

<!-- Service Provider Component -->
{#if servicesInitialized}
  <slot />
{:else if initializationError}
  <div class="p-4 bg-destructive/10 border border-destructive/20 rounded-lg">
    <p class="text-destructive font-medium">Service Initialization Error</p>
    <p class="text-sm text-destructive/80 mt-1">{initializationError}</p>
  </div>
{:else}
  <div class="flex items-center gap-2 text-muted-foreground p-4">
    <div
      class="animate-spin h-4 w-4 border-2 border-primary border-t-transparent rounded-full"
    ></div>
    <span>Initializing node services...</span>
  </div>
{/if}
