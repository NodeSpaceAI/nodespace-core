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

// Browser and Node.js compatibility handled via conditional logic

import { eventBus } from './EventBus';
import { ContentProcessor } from './contentProcessor';
import type { NodeManager, Node } from './NodeManager';
import type { HierarchyService } from './HierarchyService';
import type { NodeOperationsService } from './NodeOperationsService';
import type { MockDatabaseService, NodeSpaceNode } from './MockDatabaseService';

// ============================================================================
// Core Types and Interfaces
// ============================================================================

export interface TriggerContext {
  trigger: string; // '@'
  query: string; // Text after @
  startPosition: number; // Position in content where trigger starts
  endPosition: number; // Current cursor position
  element: HTMLElement; // ContentEditable element
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
  private databaseService: MockDatabaseService;
  private contentProcessor: ContentProcessor;
  private readonly serviceName = 'NodeReferenceService';

  // Caching for performance (following Phase 1 patterns)
  private suggestionCache = new Map<string, { result: AutocompleteResult; timestamp: number }>();
  private uriCache = new Map<string, NodeReference>();
  private searchCache = new Map<string, NodeSpaceNode[]>();
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
    databaseService: MockDatabaseService,
    contentProcessor?: ContentProcessor
  ) {
    this.nodeManager = nodeManager;
    this.hierarchyService = hierarchyService;
    this.nodeOperationsService = nodeOperationsService;
    this.databaseService = databaseService;
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
      // Find @ symbol before cursor position
      let triggerStart = -1;
      for (let i = cursorPosition - 1; i >= 0; i--) {
        const char = content[i];

        if (char === '@') {
          triggerStart = i;
          break;
        }

        // Stop if we hit whitespace or newline
        if (/\s/.test(char) || char === '\n') {
          break;
        }

        // Stop after reasonable search distance
        if (cursorPosition - i > 50) {
          break;
        }
      }

      if (triggerStart === -1) {
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
          nodeTypes: [...new Set(finalSuggestions.map((s) => s.nodeType))]
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
      // Use globalThis to access URL constructor in both browser and Node.js
      const URLConstructor = globalThis.URL;
      const url = new URLConstructor(uri);

      if (url.protocol !== 'nodespace:') {
        return null;
      }

      // Extract node ID from path
      const pathParts = url.pathname.split('/').filter((part) => part);
      if (pathParts[0] !== 'node' || !pathParts[1]) {
        return null;
      }

      const nodeId = pathParts[1];

      // Check if node exists
      const node = this.nodeManager.findNode(nodeId);
      const isValid = !!node;

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

    // Use globalThis to access URLSearchParams constructor in both browser and Node.js
    const URLSearchParamsConstructor = globalThis.URLSearchParams;
    const params = new URLSearchParamsConstructor();

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
  public resolveNodespaceURI(uri: string): NodeSpaceNode | null {
    const startTime = performance.now();
    this.performanceMetrics.totalURIResolutions++;

    try {
      const reference = this.parseNodespaceURI(uri);
      if (!reference || !reference.isValid) {
        return null;
      }

      // Use NodeManager to find node (converts to NodeSpaceNode format)
      const managerNode = this.nodeManager.findNode(reference.nodeId);
      if (!managerNode) {
        return null;
      }

      // Convert to NodeSpaceNode format for consistency
      const nodeSpaceNode: NodeSpaceNode = {
        id: managerNode.id,
        type: managerNode.nodeType,
        content: managerNode.content,
        parent_id: managerNode.parentId || null,
        root_id: this.findRootId(managerNode.id),
        before_sibling_id: null, // Would need to calculate from hierarchy
        created_at: new Date().toISOString(), // Would come from metadata in full implementation
        mentions: [], // Initialize empty mentions array - will be populated by mentions tracking
        metadata: managerNode.metadata,
        embedding_vector: null
      };

      // Update performance metrics
      const processingTime = performance.now() - startTime;
      this.updateURIResolutionTime(processingTime);

      return nodeSpaceNode;
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

      // Get current mentions from database
      const dbSourceNode = await this.databaseService.getNode(sourceId);
      const currentMentions = dbSourceNode?.mentions || [];

      // Add reference if not already present
      if (!currentMentions.includes(targetId)) {
        const updatedMentions = [...currentMentions, targetId];
        await this.nodeOperationsService.updateNodeMentions(sourceId, updatedMentions);

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

      // Get current mentions from database
      const dbSourceNode = await this.databaseService.getNode(sourceId);
      const currentMentions = dbSourceNode?.mentions || [];

      // Remove reference if present
      if (currentMentions.includes(targetId)) {
        const updatedMentions = currentMentions.filter((id) => id !== targetId);
        await this.nodeOperationsService.updateNodeMentions(sourceId, updatedMentions);

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

    // Get mentions from local cache (kept in sync with database operations)
    const mentions = this.mentionsCache.get(nodeId) || [];
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

      return referencingNodes.map((node) => ({
        nodeId: node.id,
        uri: this.createNodespaceURI(node.id),
        title: this.extractNodeTitleFromSpaceNode(node),
        nodeType: node.type,
        isValid: true,
        lastResolved: Date.now(),
        metadata: { type: 'incoming' }
      }));
    } catch (error) {
      console.error('NodeReferenceService: Error getting incoming references', { error, nodeId });
      return [];
    }
  }

  // ========================================================================
  // Node Search and Creation
  // ========================================================================

  /**
   * Search nodes with fuzzy matching and filtering
   */
  public async searchNodes(query: string, nodeType?: string): Promise<NodeSpaceNode[]> {
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
  public async createNode(nodeType: string, content: string): Promise<NodeSpaceNode> {
    try {
      const nodeId = this.generateNodeId();

      // Use NodeOperationsService for consistent node creation
      const nodeData: Partial<NodeSpaceNode> = {
        id: nodeId,
        type: nodeType,
        content: content,
        parent_id: null, // Root node
        root_id: nodeId,
        before_sibling_id: null,
        mentions: [],
        metadata: {
          createdBy: 'NodeReferenceService',
          createdAt: Date.now()
        },
        embedding_vector: null
      };

      const createdNode = await this.nodeOperationsService.upsertNode(nodeId, nodeData);

      // Emit node creation event
      eventBus.emit({
        type: 'node:created',
        namespace: 'lifecycle',
        source: this.serviceName,
        timestamp: Date.now(),
        nodeId,
        nodeType,
        metadata: { createdViaReference: true }
      });

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
        eventBus.emit({
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
        });
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
        endPos: match.index + uri.length,
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
      const nodeEvent = event as import('./EventTypes').NodeUpdatedEvent;
      this.invalidateNodeCaches(nodeEvent.nodeId);
    });

    // Listen for node deletion to clean up references
    eventBus.subscribe('node:deleted', (event) => {
      const nodeEvent = event as import('./EventTypes').NodeDeletedEvent;
      this.cleanupDeletedNodeReferences(nodeEvent.nodeId);
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

  private async createNodeSuggestion(node: NodeSpaceNode, query: string): Promise<NodeSuggestion> {
    const title = this.extractNodeTitleFromSpaceNode(node);
    const relevanceScore = this.calculateRelevanceScore(node, query);
    const matchPositions = this.findMatchPositions(title, query);
    const hierarchy = await this.getNodeHierarchy(node.id);

    return {
      nodeId: node.id,
      title,
      content: node.content.substring(0, 200), // Truncate for performance
      nodeType: node.type,
      relevanceScore,
      matchType: title.toLowerCase().includes(query.toLowerCase()) ? 'title' : 'content',
      matchPositions,
      hierarchy,
      metadata: {
        parentId: node.parent_id,
        hasChildren: false // Would need to calculate
      }
    };
  }

  private calculateRelevanceScore(node: NodeSpaceNode, query: string): number {
    const title = this.extractNodeTitleFromSpaceNode(node);
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
    if (node.type.toLowerCase().includes(queryLower)) {
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

  private extractNodeTitle(node: Node | NodeSpaceNode): string {
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

  private extractNodeTitleFromSpaceNode(node: NodeSpaceNode): string {
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
    eventBus.emit({
      type: 'references:update-needed',
      namespace: 'coordination',
      source: this.serviceName,
      timestamp: Date.now(),
      nodeId: sourceId,
      updateType: 'content',
      affectedReferences: [targetId],
      metadata: { action, targetId }
    });
  }

  private invalidateNodeCaches(nodeId: string): void {
    // Clear suggestion cache entries that might include this node
    for (const [key] of this.suggestionCache) {
      this.suggestionCache.delete(key);
    }

    // Clear search cache
    this.searchCache.clear();

    // Remove from URI cache
    for (const [uri, reference] of this.uriCache) {
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

      // Remove references to deleted node
      for (const node of referencingNodes) {
        const updatedMentions = node.mentions.filter((id) => id !== deletedNodeId);
        await this.nodeOperationsService.updateNodeMentions(node.id, updatedMentions);
      }
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
