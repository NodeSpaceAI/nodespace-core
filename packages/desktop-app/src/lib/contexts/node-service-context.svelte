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

  // Service interface definition with proper types
  export interface NodeServices {
    nodeManager: ReactiveNodeService;
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
  import { onMount, onDestroy } from 'svelte';
  import type { Snippet } from 'svelte';

  // Service imports
  import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
  import { focusManager } from '$lib/services/focus-manager.svelte';
  import { DEFAULT_PANE_ID } from '$lib/stores/navigation.svelte';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('NodeServiceContext');

  // Get paneId from context (set by PaneContent)
  const paneId = getContext<string>('paneId') ?? DEFAULT_PANE_ID;

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
      // Database initialization now happens via Tauri backend on app start
      // No need to call tauriNodeService.initializeDatabase() - it was deleted

      // Create node manager events
      const nodeManagerEvents = {
        focusRequested: (nodeId: string, position?: number) => {
          // Use FocusManager as single source of truth for focus management
          // This replaces the old DOM-based contenteditable selector approach
          if (position !== undefined) {
            focusManager.focusNodeAtPosition(nodeId, position, paneId);
          } else {
            focusManager.focusNode(nodeId, paneId);
          }
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

      // Phase 1.3: SharedNodeStore initialization
      // SharedNodeStore is automatically initialized as singleton when ReactiveNodeService is created.
      // Each ReactiveNodeService instance subscribes to the shared store for multi-viewer sync.
      const nodeManager = createReactiveNodeService(nodeManagerEvents);

      // No more demo data initialization - we'll load from real database

      // Create service bundle and update reactive state
      // (context was already set at component init with the container reference)
      servicesContainer.services = {
        nodeManager
      };

      servicesInitialized = true;
    } catch (error) {
      log.error('Failed to initialize services:', error);
      initializationError = error instanceof Error ? error.message : 'Unknown error';
    }
  });

  // Cleanup subscriptions on unmount to prevent memory leaks
  onDestroy(() => {
    if (servicesContainer.services?.nodeManager) {
      servicesContainer.services.nodeManager.destroy();
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
