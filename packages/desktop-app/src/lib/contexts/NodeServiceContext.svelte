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
  
  // Service interface definition
  export interface NodeServices {
    nodeReferenceService: any;
    nodeManager: any;
    hierarchyService: any;
    nodeOperationsService: any;
    contentProcessor: any;
    databaseService: any;
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
  import NodeReferenceService from '$lib/services/NodeReferenceService';
  import { EnhancedNodeManager } from '$lib/services/EnhancedNodeManager';
  import { HierarchyService } from '$lib/services/HierarchyService';
  import { NodeOperationsService } from '$lib/services/NodeOperationsService';
  import { MockDatabaseService } from '$lib/services/MockDatabaseService';
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
      console.log('🚀 NodeServiceContext mounted - starting service initialization');
      // Initialize services in dependency order
      const databaseService = new MockDatabaseService();
      const nodeManager = new EnhancedNodeManager();
      const hierarchyService = new HierarchyService(nodeManager);
      const nodeOperationsService = new NodeOperationsService(
        nodeManager,
        hierarchyService,
        databaseService
      );
      const contentProcessor = ContentProcessor.getInstance();
      
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
      console.log('✅ NodeServiceContext services initialized successfully');
      
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
    <div class="animate-spin h-4 w-4 border-2 border-primary border-t-transparent rounded-full"></div>
    <span>Initializing node services...</span>
  </div>
{/if}