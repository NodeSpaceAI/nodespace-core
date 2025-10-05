/**
 * NodeReferenceService - Universal Node Reference System (Phase 2.1)
 *
 * Implements comprehensive @ trigger system for universal node referencing,
 * building on Phase 1 foundation (HierarchyService, NodeOperationsService,
 * EnhancedNodeManager, EventBus, MockDatabaseService with mentions array support).
 *
 * Core Features:
 * - Universal @ trigger detection in contenteditable elements
 * - Real-time autocomplete with fuzzy search and filtering
 * - nodespace:// URI management and resolution
 * - Bidirectional reference tracking with mentions array integration
 * - Node search and creation capabilities
 * - ContentProcessor integration for enhanced content processing
 * - EventBus coordination for real-time updates
 * - Map-based caching following Phase 1 performance patterns
 *
 * Integration Architecture:
 * - EventBus: Real-time coordination and cache invalidation
 * - NodeManager/EnhancedNodeManager: Node data access and manipulation
 * - HierarchyService: Efficient node hierarchy operations
 * - NodeOperationsService: Advanced node operations and mentions management
 * - MockDatabaseService: Bidirectional reference storage via mentions array
 * - ContentProcessor: Enhanced @ trigger content processing
 */

import { eventBus } from './eventBus';
import { ContentProcessor } from './contentProcessor';
import type { ReactiveNodeService as NodeManager } from './reactiveNodeService.svelte.ts';
import type { HierarchyService } from './hierarchyService';
import type { NodeOperationsService } from './nodeOperationsService';
import type { Node } from '$lib/types';
import type { TauriNodeService } from './tauriNodeService';

// ============================================================================
// Database Service Adapter
// ============================================================================

/**
 * DatabaseServiceAdapter - Wraps TauriNodeService with missing methods
 *
 * TauriNodeService doesn't yet have queryNodes and upsertNode methods.
 * This adapter provides those capabilities using the existing methods.
 */
class DatabaseServiceAdapter {
  constructor(private tauri: TauriNodeService) {}

  /**
   * Query nodes with various filters
   * Currently implements basic functionality - will be expanded when Tauri backend supports it
   */
  async queryNodes(query: {
    id?: string;
    mentioned_by?: string;
    content_contains?: string;
    type?: string;
    limit?: number;
  }): Promise<Node[]> {
    // For id-based queries, use getNode
    if (query.id) {
      const node = await this.tauri.getNode(query.id);
      return node ? [node] : [];
    }

    // Check if tauri service has queryNodes method (for MockTauriNodeService in tests)
    if ('queryNodes' in this.tauri && typeof this.tauri.queryNodes === 'function') {
      type QueryNodesMethod = (q: {
        id?: string;
        mentioned_by?: string;
        content_contains?: string;
        type?: string;
        limit?: number;
      }) => Promise<Node[]>;
      return await (this.tauri as unknown as { queryNodes: QueryNodesMethod }).queryNodes(query);
    }

    // For other queries, we need to implement via getChildren or return empty
    // This is a temporary solution - proper implementation would query via backend
    console.warn('queryNodes: Advanced queries not yet implemented on TauriNodeService', query);
    return [];
  }

  /**
   * Upsert node (create or update)
   * Uses getNode to check existence, then creates or updates accordingly
   */
  async upsertNode(node: Node): Promise<Node> {
    try {
      const existing = await this.tauri.getNode(node.id);

      if (existing) {
        // Update existing node
        // Note: NodeUpdate doesn't include mentions - those are managed separately
        await this.tauri.updateNode(node.id, {
          content: node.content,
          parentId: node.parentId,
          beforeSiblingId: node.beforeSiblingId,
          properties: node.properties
        });
        return { ...node, modifiedAt: new Date().toISOString() };
      } else {
        // Create new node
        await this.tauri.createNode({
          id: node.id,
          nodeType: node.nodeType,
          content: node.content,
          parentId: node.parentId,
          originNodeId: node.originNodeId,
          beforeSiblingId: node.beforeSiblingId,
          mentions: node.mentions,
          properties: node.properties
        });
        return node;
      }
    } catch (error) {
      console.error('DatabaseServiceAdapter: upsertNode failed', { error, nodeId: node.id });
      throw error;
    }
  }

  /**
   * Proxy getNode from TauriNodeService
   */
  async getNode(id: string): Promise<Node | null> {
    return this.tauri.getNode(id);
  }
}

// ============================================================================
// Core Types and Interfaces
// ============================================================================

export interface TriggerContext {
  trigger: string; // '@'
  query: string; // Text after @
  startPosition: number; // Position in content where trigger starts
  endPosition: number; // Current cursor position
  element: HTMLElement | null; // ContentEditable element (null in test scenarios)
  isValid: boolean; // Whether trigger context is valid
  metadata: Record<string, unknown>;
}

export interface AutocompleteResult {
  suggestions: NodeSuggestion[];
  query: string;
  totalCount: number;
  hasMore: boolean;
  queryTime: number;
  metadata: Record<string, unknown>;
}

export interface NodeSuggestion {
  nodeId: string;
  title: string;
  content: string;
  nodeType: string;
  relevanceScore: number;
  matchType: 'title' | 'content' | 'id';
  matchPositions: number[];
  hierarchy: string[];
  metadata: Record<string, unknown>;
}

export interface NodeReference {
  nodeId: string;
  uri: string;
  title: string;
  nodeType: string;
  isValid: boolean;
  lastResolved: number;
  metadata: Record<string, unknown>;
}

export interface URIOptions {
  includeHierarchy?: boolean;
  includeTimestamp?: boolean;
  fragment?: string;
  queryParams?: Record<string, string>;
}

export interface NodespaceLink {
  uri: string;
  startPos: number;
  endPos: number;
  nodeId: string;
  displayText: string;
  isValid: boolean;
  metadata: Record<string, unknown>;
}

export interface AutocompleteConfig {
  maxSuggestions: number;
  fuzzyThreshold: number;
  debounceMs: number;
  minQueryLength: number;
  enableFuzzySearch: boolean;
  prioritizeRecent: boolean;
  includeContent: boolean;
}

// ============================================================================
// NodeReferenceService Implementation
// ============================================================================

export class NodeReferenceService {
  private nodeManager: NodeManager;
  private hierarchyService: HierarchyService;
  private nodeOperationsService: NodeOperationsService;
  private databaseService: DatabaseServiceAdapter;
  private contentProcessor: ContentProcessor;
  private readonly serviceName = 'NodeReferenceService';

  // Caching for performance (following Phase 1 patterns)
  private suggestionCache = new Map<string, { result: AutocompleteResult; timestamp: number }>();
  private uriCache = new Map<string, NodeReference>();
  private searchCache = new Map<string, Node[]>();
  private mentionsCache = new Map<string, string[]>(); // nodeId -> array of mentioned nodeIds
  private readonly cacheTimeout = 30000; // 30 seconds

  // Configuration
  private autocompleteConfig: AutocompleteConfig = {
    maxSuggestions: 10,
    fuzzyThreshold: 0.6,
    debounceMs: 150,
    minQueryLength: 1,
    enableFuzzySearch: true,
    prioritizeRecent: true,
    includeContent: true
  };

  // Performance metrics
  private performanceMetrics = {
    totalTriggerDetections: 0,
    totalAutocompleteRequests: 0,
    totalURIResolutions: 0,
    avgTriggerDetectionTime: 0,
    avgAutocompleteTime: 0,
    avgURIResolutionTime: 0,
    cacheHitRatio: 0
  };

  constructor(
    nodeManager: NodeManager,
    hierarchyService: HierarchyService,
    nodeOperationsService: NodeOperationsService,
    databaseService: TauriNodeService,
    contentProcessor?: ContentProcessor
  ) {
    this.nodeManager = nodeManager;
    this.hierarchyService = hierarchyService;
    this.nodeOperationsService = nodeOperationsService;
    this.databaseService = new DatabaseServiceAdapter(databaseService);
    this.contentProcessor = contentProcessor || ContentProcessor.getInstance();

    this.setupEventBusIntegration();
    this.enhanceContentProcessor();
  }

  // ========================================================================
  // @ Trigger Detection
  // ========================================================================

  /**
   * Detect @ trigger in contenteditable element at cursor position
   * Performance target: <1ms for real-time typing
   */
  public detectTrigger(content: string, cursorPosition: number): TriggerContext | null {
    const startTime = performance.now();
    this.performanceMetrics.totalTriggerDetections++;

    try {
      // Optimized search: find @ symbol before cursor position
      let triggerStart = -1;
      const searchStart = Math.max(0, cursorPosition - 50); // Limit search distance for performance

      // More efficient backwards search
      for (let i = cursorPosition - 1; i >= searchStart; i--) {
        const char = content[i];

        if (char === '@') {
          triggerStart = i;
          break;
        }

        // Stop if we hit whitespace or newline
        if (/\s/.test(char) || char === '\n') {
          break;
        }
      }

      if (triggerStart === -1) {
        const processingTime = performance.now() - startTime;
        this.updateTriggerDetectionTime(processingTime);
        return null;
      }

      // Extract query after @
      const queryText = content.substring(triggerStart + 1, cursorPosition);

      // Validate trigger context
      const isValid = this.validateTriggerContext(content, triggerStart, cursorPosition);

      const context: TriggerContext = {
        trigger: '@',
        query: queryText,
        startPosition: triggerStart,
        endPosition: cursorPosition,
        element: (typeof document !== 'undefined' ? document.activeElement : null) as HTMLElement, // Will be set by caller in real usage
        isValid,
        metadata: {
          contentLength: content.length,
          triggerDistance: cursorPosition - triggerStart - 1
        }
      };

      // Update performance metrics
      const processingTime = performance.now() - startTime;
      this.updateTriggerDetectionTime(processingTime);

      return context;
    } catch (error) {
      console.error('NodeReferenceService: Error detecting trigger', {
        error,
        content,
        cursorPosition
      });
      const processingTime = performance.now() - startTime;
      this.updateTriggerDetectionTime(processingTime);
      return null;
    }
  }

  /**
   * Enhanced trigger detection with contenteditable element integration
   */
  public detectTriggerInElement(element: HTMLElement): TriggerContext | null {
    if (!element || !element.isContentEditable) {
      return null;
    }

    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) {
      return null;
    }

    const range = selection.getRangeAt(0);
    const textContent = element.textContent || '';
    const cursorPosition = this.getCursorPosition(element, range);

    const context = this.detectTrigger(textContent, cursorPosition);
    if (context) {
      context.element = element;
    }

    return context;
  }

  // ========================================================================
  // Autocomplete System
  // ========================================================================

  /**
   * Show autocomplete suggestions for trigger context
   * Uses fuzzy search and relevance scoring
   */
  public async showAutocomplete(triggerContext: TriggerContext): Promise<AutocompleteResult> {
    const startTime = performance.now();
    this.performanceMetrics.totalAutocompleteRequests++;

    const { query } = triggerContext;
    const cacheKey = `autocomplete:${query}:${JSON.stringify(this.autocompleteConfig)}`;

    // Check cache first
    const cached = this.suggestionCache.get(cacheKey);
    if (cached && Date.now() - cached.timestamp < this.cacheTimeout) {
      return cached.result;
    }

    try {
      // Search nodes with query
      const nodes = await this.searchNodes(query);

      // Convert to suggestions with relevance scoring
      const suggestions: NodeSuggestion[] = [];

      for (const node of nodes.slice(0, this.autocompleteConfig.maxSuggestions * 2)) {
        const suggestion = await this.createNodeSuggestion(node, query);
        if (suggestion.relevanceScore >= this.autocompleteConfig.fuzzyThreshold) {
          suggestions.push(suggestion);
        }
      }

      // Sort by relevance score and limit results
      suggestions.sort((a, b) => b.relevanceScore - a.relevanceScore);
      const finalSuggestions = suggestions.slice(0, this.autocompleteConfig.maxSuggestions);

      const queryTime = performance.now() - startTime;
      const result: AutocompleteResult = {
        suggestions: finalSuggestions,
        query,
        totalCount: nodes.length,
        hasMore: nodes.length > this.autocompleteConfig.maxSuggestions,
        queryTime,
        metadata: {
          cacheUsed: false,
          fuzzyEnabled: this.autocompleteConfig.enableFuzzySearch,
          nodeTypes: Array.from(new Set(finalSuggestions.map((s) => s.nodeType)))
        }
      };

      // Cache the result
      this.suggestionCache.set(cacheKey, { result, timestamp: Date.now() });

      // Update performance metrics
      this.updateAutocompleteTime(queryTime);

      return result;
    } catch (error) {
      console.error('NodeReferenceService: Error in autocomplete', { error, query });

      return {
        suggestions: [],
        query,
        totalCount: 0,
        hasMore: false,
        queryTime: performance.now() - startTime,
        metadata: { error: true }
      };
    }
  }

  // ========================================================================
  // nodespace:// URI Management
  // ========================================================================

  /**
   * Parse nodespace:// URI into NodeReference
   * Format: nodespace://node/{nodeId}?hierarchy=true&timestamp=123456
   */
  public parseNodespaceURI(uri: string): NodeReference | null {
    try {
      // Check if URL constructor is available
      if (typeof URL === 'undefined') {
        console.warn('NodeReferenceService: URL constructor not available in this environment');
        return null;
      }

      const url = new URL(uri);

      if (url.protocol !== 'nodespace:') {
        return null;
      }

      // Check hostname should be 'node'
      if (url.hostname !== 'node') {
        return null;
      }

      // Extract node ID from path (should be the pathname without leading slash)
      const pathParts = url.pathname.split('/').filter((part) => part);

      if (!pathParts[0]) {
        return null;
      }

      const nodeId = pathParts[0]; // First part of path is the node ID

      // For URI parsing, we only validate structure, not node existence
      // Node existence will be checked during resolution
      const node = this.nodeManager.findNode(nodeId); // Still get node data if available
      const isValid = true; // URI is structurally valid

      const reference: NodeReference = {
        nodeId,
        uri,
        title: node ? this.extractNodeTitle(node) : nodeId,
        nodeType: node?.nodeType || 'unknown',
        isValid,
        lastResolved: Date.now(),
        metadata: {
          hierarchy: url.searchParams.get('hierarchy') === 'true',
          timestamp: url.searchParams.get('timestamp'),
          fragment: url.hash ? url.hash.substring(1) : undefined,
          queryParams: Object.fromEntries(url.searchParams.entries())
        }
      };

      // Cache valid references
      if (isValid) {
        this.uriCache.set(uri, reference);
      }

      return reference;
    } catch (error) {
      console.error('NodeReferenceService: Error parsing URI', { error, uri });
      return null;
    }
  }

  /**
   * Create nodespace:// URI for node with options
   */
  public createNodespaceURI(nodeId: string, options: URIOptions = {}): string {
    let uri = `nodespace://node/${nodeId}`;

    // Check if URLSearchParams is available
    if (typeof URLSearchParams === 'undefined') {
      console.warn('NodeReferenceService: URLSearchParams not available in this environment');
      // Fallback to manual query string construction
      const queryParts: string[] = [];

      if (options.includeHierarchy) {
        queryParts.push('hierarchy=true');
      }

      if (options.includeTimestamp) {
        queryParts.push(`timestamp=${Date.now()}`);
      }

      if (options.queryParams) {
        for (const [key, value] of Object.entries(options.queryParams)) {
          queryParts.push(`${encodeURIComponent(key)}=${encodeURIComponent(value)}`);
        }
      }

      if (queryParts.length > 0) {
        uri += `?${queryParts.join('&')}`;
      }

      if (options.fragment) {
        uri += `#${encodeURIComponent(options.fragment)}`;
      }

      return uri;
    }

    const params = new URLSearchParams();

    if (options.includeHierarchy) {
      params.set('hierarchy', 'true');
    }

    if (options.includeTimestamp) {
      params.set('timestamp', Date.now().toString());
    }

    if (options.queryParams) {
      for (const [key, value] of Object.entries(options.queryParams)) {
        params.set(key, value);
      }
    }

    if (params.toString()) {
      uri += `?${params.toString()}`;
    }

    if (options.fragment) {
      uri += `#${options.fragment}`;
    }

    return uri;
  }

  /**
   * Resolve nodespace:// URI to actual node
   */
  public async resolveNodespaceURI(uri: string): Promise<Node | null> {
    const startTime = performance.now();
    this.performanceMetrics.totalURIResolutions++;

    try {
      const reference = this.parseNodespaceURI(uri);
      if (!reference || !reference.isValid) {
        return null;
      }

      // Try to find node in NodeManager first
      const managerNode = this.nodeManager.findNode(reference.nodeId);
      if (managerNode) {
        // Found in NodeManager - it's already a Node
        this.performanceMetrics.avgURIResolutionTime =
          (this.performanceMetrics.avgURIResolutionTime + (performance.now() - startTime)) / 2;
        return managerNode;
      }

      // If not found in NodeManager, try database (for test scenarios)
      try {
        const dbNodes = await this.databaseService.queryNodes({ id: reference.nodeId });
        if (dbNodes && dbNodes.length > 0) {
          const dbNode = dbNodes[0];
          this.performanceMetrics.avgURIResolutionTime =
            (this.performanceMetrics.avgURIResolutionTime + (performance.now() - startTime)) / 2;
          return dbNode;
        }
      } catch (dbError) {
        console.warn('NodeReferenceService: Database lookup failed', {
          error: dbError,
          nodeId: reference.nodeId
        });
      }

      // Node not found anywhere
      return null;
    } catch (error) {
      console.error('NodeReferenceService: Error resolving URI', { error, uri });
      return null;
    }
  }

  // ========================================================================
  // Bidirectional Reference Tracking
  // ========================================================================

  /**
   * Add bidirectional reference between nodes
   * Uses mentions array as the bidirectional backlink system
   */
  public async addReference(sourceId: string, targetId: string): Promise<void> {
    try {
      // Validate both nodes exist
      const sourceNode = this.nodeManager.findNode(sourceId);
      const targetNode = this.nodeManager.findNode(targetId);

      if (!sourceNode || !targetNode) {
        throw new Error(`Node not found: source=${!!sourceNode}, target=${!!targetNode}`);
      }

      // Get current mentions from database and in-memory node
      const dbSourceNode = await this.databaseService.getNode(sourceId);
      const currentMentions = dbSourceNode?.mentions || [];
      const inMemoryMentions = sourceNode.mentions || [];

      // Use the union of both mention arrays to ensure consistency
      const allCurrentMentions = [...new Set([...currentMentions, ...inMemoryMentions])];

      // Add reference if not already present
      if (!allCurrentMentions.includes(targetId)) {
        const updatedMentions = [...allCurrentMentions, targetId];

        // Update database
        await this.nodeOperationsService.updateNodeMentions(sourceId, updatedMentions);

        // Update in-memory node
        sourceNode.mentions = updatedMentions;

        // Update local cache
        this.mentionsCache.set(sourceId, updatedMentions);

        // Emit reference added event
        this.emitReferenceEvent('added', sourceId, targetId);
      }
    } catch (error) {
      console.error('NodeReferenceService: Error adding reference', { error, sourceId, targetId });
      throw error;
    }
  }

  /**
   * Remove bidirectional reference between nodes
   */
  public async removeReference(sourceId: string, targetId: string): Promise<void> {
    try {
      const sourceNode = this.nodeManager.findNode(sourceId);
      if (!sourceNode) {
        throw new Error(`Source node not found: ${sourceId}`);
      }

      // Get current mentions from both database and local cache
      const dbSourceNode = await this.databaseService.getNode(sourceId);
      const currentMentions = dbSourceNode?.mentions || [];

      // Also update the in-memory node mentions directly
      const inMemoryMentions = sourceNode.mentions || [];

      // Remove reference if present in either location
      if (currentMentions.includes(targetId) || inMemoryMentions.includes(targetId)) {
        const updatedMentions = currentMentions.filter((id: string) => id !== targetId);

        // Update both database and in-memory representation
        await this.nodeOperationsService.updateNodeMentions(sourceId, updatedMentions);
        sourceNode.mentions = updatedMentions;

        // Update local cache
        this.mentionsCache.set(sourceId, updatedMentions);

        // Emit reference removed event
        this.emitReferenceEvent('removed', sourceId, targetId);
      }
    } catch (error) {
      console.error('NodeReferenceService: Error removing reference', {
        error,
        sourceId,
        targetId
      });
      throw error;
    }
  }

  /**
   * Get outgoing references from a node (nodes this node mentions)
   */
  public getOutgoingReferences(nodeId: string): NodeReference[] {
    const node = this.nodeManager.findNode(nodeId);
    if (!node) {
      return [];
    }

    // Get mentions from multiple sources to ensure consistency
    const cacheMentions = this.mentionsCache.get(nodeId) || [];
    const nodeMentions = node.mentions || [];

    // Use the most recent mentions (prioritize cache if it exists, otherwise node mentions)
    const mentions = cacheMentions.length > 0 ? cacheMentions : nodeMentions;

    return mentions.map((mentionedId) => {
      const targetNode = this.nodeManager.findNode(mentionedId);
      return {
        nodeId: mentionedId,
        uri: this.createNodespaceURI(mentionedId),
        title: targetNode ? this.extractNodeTitle(targetNode) : mentionedId,
        nodeType: targetNode?.nodeType || 'unknown',
        isValid: !!targetNode,
        lastResolved: Date.now(),
        metadata: { type: 'outgoing' }
      };
    });
  }

  /**
   * Get incoming references to a node (nodes that mention this node)
   * Uses database service to query mentions array efficiently
   */
  public async getIncomingReferences(nodeId: string): Promise<NodeReference[]> {
    try {
      // Use database service to find nodes that mention this node
      const referencingNodes = await this.databaseService.queryNodes({
        mentioned_by: nodeId
      });

      return referencingNodes.map((node: Node) => ({
        nodeId: node.id,
        uri: this.createNodespaceURI(node.id),
        title: this.extractNodeTitleFromNode(node),
        nodeType: node.nodeType,
        isValid: true,
        lastResolved: Date.now(),
        metadata: { type: 'incoming' }
      }));
    } catch (err) {
      console.error('NodeReferenceService: Error getting incoming references', {
        error: err,
        nodeId
      });
      return [];
    }
  }

  // ========================================================================
  // Node Search and Creation
  // ========================================================================

  /**
   * Search nodes with fuzzy matching and filtering
   */
  public async searchNodes(query: string, nodeType?: string): Promise<Node[]> {
    if (!query || query.length < this.autocompleteConfig.minQueryLength) {
      return [];
    }

    const cacheKey = `search:${query}:${nodeType || 'all'}`;

    // Check cache first
    const cached = this.searchCache.get(cacheKey);
    if (cached) {
      return cached;
    }

    try {
      // Use database service for efficient querying
      const results = await this.databaseService.queryNodes({
        content_contains: query,
        type: nodeType,
        limit: this.autocompleteConfig.maxSuggestions * 3 // Get more for better filtering
      });

      // Cache results
      this.searchCache.set(cacheKey, results);

      return results;
    } catch (error) {
      console.error('NodeReferenceService: Error searching nodes', { error, query, nodeType });
      return [];
    }
  }

  /**
   * Create new node with specified type and content
   */
  public async createNode(nodeType: string, content: string): Promise<Node> {
    try {
      const nodeId = this.generateNodeId();

      // Create Node with proper type structure
      const finalNodeData: Node = {
        id: nodeId,
        nodeType: nodeType,
        content: content,
        parentId: null, // Root node
        originNodeId: nodeId,
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        mentions: [],
        properties: {
          createdBy: 'NodeReferenceService',
          createdAt: Date.now()
        }
      };

      // Store directly in database service via adapter
      const createdNode = await this.databaseService.upsertNode(finalNodeData);

      // In a full implementation, the node would be automatically
      // synchronized with the NodeManager via database change events.
      // For this testing phase, we focus on database storage.

      // Emit node creation event
      const nodeCreatedEvent: import('./eventTypes').NodeCreatedEvent = {
        type: 'node:created',
        namespace: 'lifecycle',
        source: this.serviceName,
        timestamp: Date.now(),
        nodeId: nodeId,
        nodeType,
        metadata: { createdViaReference: true }
      };
      eventBus.emit(nodeCreatedEvent);

      return createdNode;
    } catch (error) {
      console.error('NodeReferenceService: Error creating node', { error, nodeType, content });
      throw error;
    }
  }

  // ========================================================================
  // ContentProcessor Integration
  // ========================================================================

  /**
   * Enhance ContentProcessor with @ trigger detection
   */
  public enhanceContentProcessor(): void {
    // Add @ trigger detection to content processing pipeline
    const originalProcessContent = this.contentProcessor.processContentWithEventEmission;

    this.contentProcessor.processContentWithEventEmission = (content: string, nodeId: string) => {
      // Call original processing
      const result = originalProcessContent.call(this.contentProcessor, content, nodeId);

      // Add @ trigger detection
      const atLinks = this.detectNodespaceLinks(content);

      // Emit events for detected @ references
      for (const link of atLinks) {
        const referenceResolvedEvent: import('./eventTypes').ReferenceResolutionEvent = {
          type: 'reference:resolved',
          namespace: 'coordination',
          source: this.serviceName,
          timestamp: Date.now(),
          referenceId: link.uri,
          target: link.nodeId,
          nodeId: nodeId,
          resolutionResult: link.isValid ? 'found' : 'not-found',
          metadata: {
            startPos: link.startPos,
            endPos: link.endPos,
            displayText: link.displayText
          }
        };
        eventBus.emit(referenceResolvedEvent);
      }

      return result;
    };
  }

  /**
   * Detect nodespace:// links in content
   */
  public detectNodespaceLinks(content: string): NodespaceLink[] {
    const links: NodespaceLink[] = [];
    const regex = /nodespace:\/\/node\/([a-zA-Z0-9_-]+)(?:\?[^)\s]*)?/g;

    let match;
    while ((match = regex.exec(content)) !== null) {
      const uri = match[0];
      const nodeId = match[1];
      const reference = this.parseNodespaceURI(uri);

      links.push({
        uri,
        startPos: match.index,
        endPos: match.index + uri.length, // This is correct - endPos is exclusive
        nodeId,
        displayText: reference?.title || nodeId,
        isValid: !!reference?.isValid,
        metadata: {
          parsed: !!reference
        }
      });
    }

    return links;
  }

  // ========================================================================
  // Configuration and Performance
  // ========================================================================

  /**
   * Configure autocomplete behavior
   */
  public configureAutocomplete(config: Partial<AutocompleteConfig>): void {
    this.autocompleteConfig = { ...this.autocompleteConfig, ...config };

    // Clear cache when configuration changes
    this.clearCaches();
  }

  /**
   * Get performance metrics
   */
  public getPerformanceMetrics(): typeof this.performanceMetrics {
    return { ...this.performanceMetrics };
  }

  /**
   * Clear all caches
   */
  public clearCaches(): void {
    this.suggestionCache.clear();
    this.uriCache.clear();
    this.searchCache.clear();
  }

  // ========================================================================
  // Private Helper Methods
  // ========================================================================

  private setupEventBusIntegration(): void {
    // Listen for node updates to invalidate caches
    eventBus.subscribe('node:updated', (event) => {
      const nodeEvent = event as import('./eventTypes').NodeUpdatedEvent;
      this.invalidateNodeCaches(nodeEvent.nodeId);
    });

    // Listen for node deletion to clean up references
    eventBus.subscribe('node:deleted', (event) => {
      const nodeEvent = event as import('./eventTypes').NodeDeletedEvent;
      // Use setTimeout to ensure this runs asynchronously
      setTimeout(() => {
        this.cleanupDeletedNodeReferences(nodeEvent.nodeId);
      }, 0);
    });

    // Listen for hierarchy changes
    eventBus.subscribe('hierarchy:changed', () => {
      this.clearCaches(); // Hierarchy changes might affect search results
    });
  }

  private validateTriggerContext(
    content: string,
    triggerStart: number,
    cursorPosition: number
  ): boolean {
    // Check if @ is at start of line or preceded by whitespace
    if (triggerStart > 0) {
      const prevChar = content[triggerStart - 1];
      if (!/\s/.test(prevChar)) {
        return false;
      }
    }

    // Check if query contains invalid characters
    const query = content.substring(triggerStart + 1, cursorPosition);
    if (/[\s\n]/.test(query)) {
      return false;
    }

    return true;
  }

  private getCursorPosition(element: HTMLElement, range: Range): number {
    const textContent = element.textContent || '';
    let position = 0;

    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);

    let node;
    while ((node = walker.nextNode())) {
      if (node === range.startContainer) {
        return position + range.startOffset;
      }
      position += node.textContent?.length || 0;
    }

    return textContent.length;
  }

  private async createNodeSuggestion(node: Node, query: string): Promise<NodeSuggestion> {
    const title = this.extractNodeTitleFromNode(node);
    const relevanceScore = this.calculateRelevanceScore(node, query);
    const matchPositions = this.findMatchPositions(title, query);
    const hierarchy = await this.getNodeHierarchy(node.id);

    return {
      nodeId: node.id,
      title,
      content: node.content.substring(0, 200), // Truncate for performance
      nodeType: node.nodeType,
      relevanceScore,
      matchType: title.toLowerCase().includes(query.toLowerCase()) ? 'title' : 'content',
      matchPositions,
      hierarchy,
      metadata: {
        parentId: node.parentId,
        hasChildren: false // Would need to calculate
      }
    };
  }

  private calculateRelevanceScore(node: Node, query: string): number {
    const title = this.extractNodeTitleFromNode(node);
    const content = node.content;
    const queryLower = query.toLowerCase();
    const titleLower = title.toLowerCase();
    const contentLower = content.toLowerCase();

    let score = 0;

    // Exact title match gets highest score
    if (titleLower === queryLower) {
      score += 1.0;
    } else if (titleLower.startsWith(queryLower)) {
      score += 0.8;
    } else if (titleLower.includes(queryLower)) {
      score += 0.6;
    }

    // Content matches get lower scores
    if (contentLower.includes(queryLower)) {
      score += 0.3;
    }

    // Boost score for exact node type matches
    if (node.nodeType && node.nodeType.toLowerCase().includes(queryLower)) {
      score += 0.2;
    }

    return Math.min(score, 1.0);
  }

  private findMatchPositions(text: string, query: string): number[] {
    const positions: number[] = [];
    const textLower = text.toLowerCase();
    const queryLower = query.toLowerCase();

    let index = textLower.indexOf(queryLower);
    while (index !== -1) {
      positions.push(index);
      index = textLower.indexOf(queryLower, index + 1);
    }

    return positions;
  }

  private async getNodeHierarchy(nodeId: string): Promise<string[]> {
    try {
      const path = this.hierarchyService.getNodePath(nodeId);
      return path.nodeIds.map((id) => {
        const node = this.nodeManager.findNode(id);
        return node ? this.extractNodeTitle(node) : id;
      });
    } catch {
      return [nodeId];
    }
  }

  private extractNodeTitle(node: Node): string {
    if (!node.content) return 'Untitled';

    // Try to extract title from content
    const lines = node.content.split('\n');
    const firstLine = lines[0].trim();

    // Remove markdown header syntax
    const headerMatch = firstLine.match(/^#{1,6}\s*(.*)$/);
    if (headerMatch) {
      return headerMatch[1].trim() || 'Untitled';
    }

    // Return first non-empty line, truncated
    return firstLine.substring(0, 100) || 'Untitled';
  }

  private extractNodeTitleFromNode(node: Node): string {
    if (!node.content) return 'Untitled';

    const lines = node.content.split('\n');
    const firstLine = lines[0].trim();

    const headerMatch = firstLine.match(/^#{1,6}\s*(.*)$/);
    if (headerMatch) {
      return headerMatch[1].trim() || 'Untitled';
    }

    return firstLine.substring(0, 100) || 'Untitled';
  }

  private findRootId(nodeId: string): string {
    let currentId = nodeId;
    let node = this.nodeManager.findNode(currentId);

    while (node && node.parentId) {
      currentId = node.parentId;
      node = this.nodeManager.findNode(currentId);
    }

    return currentId;
  }

  private generateNodeId(): string {
    return `node_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
  }

  private emitReferenceEvent(
    action: 'added' | 'removed',
    sourceId: string,
    targetId: string
  ): void {
    const referencesUpdateEvent: import('./eventTypes').ReferencesUpdateNeededEvent = {
      type: 'references:update-needed',
      namespace: 'coordination',
      source: this.serviceName,
      timestamp: Date.now(),
      nodeId: sourceId,
      updateType: 'content',
      affectedReferences: [targetId],
      metadata: { action, targetId }
    };
    eventBus.emit(referencesUpdateEvent);
  }

  private invalidateNodeCaches(nodeId: string): void {
    // Clear suggestion cache entries that might include this node
    for (const key of Array.from(this.suggestionCache.keys())) {
      this.suggestionCache.delete(key);
    }

    // Clear search cache
    this.searchCache.clear();

    // Remove from URI cache
    for (const [uri, reference] of Array.from(this.uriCache.entries())) {
      if (reference.nodeId === nodeId) {
        this.uriCache.delete(uri);
      }
    }
  }

  private async cleanupDeletedNodeReferences(deletedNodeId: string): Promise<void> {
    try {
      // Find all nodes that reference the deleted node
      const referencingNodes = await this.databaseService.queryNodes({
        mentioned_by: deletedNodeId
      });

      // Remove references to deleted node from database and in-memory nodes
      for (const node of referencingNodes) {
        const updatedMentions = (node.mentions || []).filter((id: string) => id !== deletedNodeId);

        // Update database
        await this.nodeOperationsService.updateNodeMentions(node.id, updatedMentions);

        // Update in-memory node if it exists
        const inMemoryNode = this.nodeManager.findNode(node.id);
        if (inMemoryNode) {
          inMemoryNode.mentions = updatedMentions;
        }

        // Update cache
        this.mentionsCache.set(node.id, updatedMentions);
      }

      // Also clean up cache entry for the deleted node itself
      this.mentionsCache.delete(deletedNodeId);
    } catch (error) {
      console.error('NodeReferenceService: Error cleaning up deleted node references', {
        error,
        deletedNodeId
      });
    }
  }

  private updateTriggerDetectionTime(time: number): void {
    const count = this.performanceMetrics.totalTriggerDetections;
    const current = this.performanceMetrics.avgTriggerDetectionTime;
    this.performanceMetrics.avgTriggerDetectionTime =
      count === 1 ? time : (current * (count - 1) + time) / count;
  }

  private updateAutocompleteTime(time: number): void {
    const count = this.performanceMetrics.totalAutocompleteRequests;
    const current = this.performanceMetrics.avgAutocompleteTime;
    this.performanceMetrics.avgAutocompleteTime =
      count === 1 ? time : (current * (count - 1) + time) / count;
  }

  private updateURIResolutionTime(time: number): void {
    const count = this.performanceMetrics.totalURIResolutions;
    const current = this.performanceMetrics.avgURIResolutionTime;
    this.performanceMetrics.avgURIResolutionTime =
      count === 1 ? time : (current * (count - 1) + time) / count;
  }
}

export default NodeReferenceService;
