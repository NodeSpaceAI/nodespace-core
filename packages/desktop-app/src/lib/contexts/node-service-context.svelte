<!--
  NodeServiceContext - Service Context Provider for Node Components
  
  Provides centralized service initialization and management for all node components.
  This ensures BaseNode and its derived components can access services without
  requiring manual service passing through props.
-->

<script lang="ts" module>
  import { getContext, setContext } from 'svelte';

  // Context key for service access
  const NODE_SERVICE_CONTEXT_KEY = Symbol('nodeServices');

  // Import proper types for the services
  import type { ReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
  import type { HierarchyService as HierarchyServiceType } from '$lib/services/hierarchy-service';
  import type { NodeOperationsService as NodeOperationsServiceType } from '$lib/services/node-operations-service';
  import type { ContentProcessor as ContentProcessorType } from '$lib/services/content-processor';
  import type { TauriNodeService as TauriNodeServiceType } from '$lib/services/tauri-node-service';
  import type NodeReferenceServiceType from '$lib/services/node-reference-service';

  // Service interface definition with proper types
  export interface NodeServices {
    nodeReferenceService: NodeReferenceServiceType;
    nodeManager: ReactiveNodeService;
    hierarchyService: HierarchyServiceType;
    nodeOperationsService: NodeOperationsServiceType;
    contentProcessor: ContentProcessorType;
    databaseService: TauriNodeServiceType;
  }

  // Context accessor functions
  export function getNodeServices(): NodeServices | null {
    const ctx = getContext<{ services: NodeServices | null }>(NODE_SERVICE_CONTEXT_KEY);
    return ctx?.services || null;
  }

  function setNodeServicesContext(servicesRef: { services: NodeServices | null }): void {
    setContext(NODE_SERVICE_CONTEXT_KEY, servicesRef);
  }
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import type { Snippet } from 'svelte';

  // Service imports
  import NodeReferenceService from '$lib/services/node-reference-service';
  import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
  import { HierarchyService } from '$lib/services/hierarchy-service';
  import { NodeOperationsService } from '$lib/services/node-operations-service';
  import { tauriNodeService } from '$lib/services/tauri-node-service';
  import { ContentProcessor } from '$lib/services/content-processor';

  // Props
  let {
    children
  }: {
    children: Snippet;
  } = $props();

  // Services state - wrapped in object so context can hold reference
  const servicesContainer = $state<{ services: NodeServices | null }>({ services: null });
  let servicesInitialized = $state(false);
  let initializationError = $state<string | null>(null);

  // Set context immediately with container reference (required by Svelte)
  setNodeServicesContext(servicesContainer);

  // Initialize services on mount
  onMount(async () => {
    try {
      // Try to initialize database (may fail in web mode)
      try {
        await tauriNodeService.initializeDatabase();
      } catch (dbError) {
        console.warn(
          '[NodeServiceContext] Database unavailable, continuing without persistence:',
          dbError
        );
        // Continue - services will work in memory-only mode
      }

      // Create node manager events
      const nodeManagerEvents = {
        focusRequested: (nodeId: string, position?: number) => {
          // Use DOM API to focus the node directly with cursor positioning
          setTimeout(() => {
            const nodeElement = document.querySelector(
              `[data-node-id="${nodeId}"] [contenteditable]`
            ) as HTMLElement;
            if (nodeElement) {
              nodeElement.focus();

              // Set cursor position using proven algorithm from working version
              if (position !== undefined && position >= 0) {
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

      // No more demo data initialization - we'll load from real database

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
        tauriNodeService,
        contentProcessor
      );

      // Create service bundle and update reactive state
      // (context was already set at component init with the container reference)
      servicesContainer.services = {
        nodeReferenceService,
        nodeManager,
        hierarchyService,
        nodeOperationsService,
        contentProcessor,
        databaseService: tauriNodeService
      };

      servicesInitialized = true;
    } catch (error) {
      console.error('NodeServiceContext: Failed to initialize services:', error);
      initializationError = error instanceof Error ? error.message : 'Unknown error';
    }
  });
</script>

<!-- Service Provider Component -->
{#if servicesInitialized}
  {@render children()}
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
