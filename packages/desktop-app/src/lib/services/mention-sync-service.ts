/**
 * MentionSyncService - Link Text Synchronization for Node Mentions
 *
 * Automatically updates markdown link display text when referenced nodes change.
 * Handles both content changes and node deletions.
 *
 * Features:
 * - Subscribes to EventBus events (node:updated, node:deleted)
 * - Queries all nodes mentioning the changed node
 * - Parses and updates markdown link text in node content
 * - Handles broken links (deleted nodes) with visual indicators
 * - Persists updated content to database
 * - Emits update events for changed nodes
 *
 * Integration:
 * - EventBus: Real-time coordination via node:updated and node:deleted events
 * - TauriNodeService: Queries nodes via mentionedBy and updates content
 * - ContentProcessor: Title extraction from node content
 */

import { eventBus } from './event-bus';
import type { TauriNodeService } from './tauri-node-service';
import type { Node } from '$lib/types';
import type { NodeUpdatedEvent, NodeDeletedEvent } from './event-types';

// ============================================================================
// Types and Interfaces
// ============================================================================

export interface LinkUpdateResult {
  nodeId: string;
  oldContent: string;
  newContent: string;
  linksUpdated: number;
}

export interface MarkdownLink {
  fullMatch: string;
  displayText: string;
  uri: string;
  nodeId: string;
  startIndex: number;
  endIndex: number;
}

// ============================================================================
// MentionSyncService Implementation
// ============================================================================

export class MentionSyncService {
  private databaseService: TauriNodeService;
  private readonly serviceName = 'MentionSyncService';

  // Markdown link pattern: [display text](nodespace://nodeId)
  private readonly LINK_PATTERN = /\[([^\]]+)\]\(nodespace:\/\/([a-zA-Z0-9_-]+)(?:\?[^)]*)?\)/g;

  // Performance tracking
  private metrics = {
    totalSyncs: 0,
    totalLinksUpdated: 0,
    totalNodesUpdated: 0,
    avgSyncTime: 0
  };

  constructor(databaseService: TauriNodeService) {
    this.databaseService = databaseService;
    this.setupEventBusIntegration();
  }

  // ========================================================================
  // EventBus Integration
  // ========================================================================

  private setupEventBusIntegration(): void {
    // Listen for node content updates
    eventBus.subscribe('node:updated', async (event) => {
      const nodeEvent = event as NodeUpdatedEvent;

      // Only sync when content changes (title might have changed)
      if (nodeEvent.updateType === 'content' && nodeEvent.newValue) {
        const updatedNode = nodeEvent.newValue as Node;

        // Skip sync if node has no content (blank nodes don't have titles to sync)
        if (!updatedNode.content || updatedNode.content.trim() === '') {
          return;
        }

        await this.syncLinkTextForNodeWithData(nodeEvent.nodeId, updatedNode);
      }
    });

    // Listen for node deletions
    eventBus.subscribe('node:deleted', async (event) => {
      const nodeEvent = event as NodeDeletedEvent;
      await this.markLinksAsBroken(nodeEvent.nodeId);
    });
  }

  // ========================================================================
  // Link Text Synchronization
  // ========================================================================

  /**
   * Sync link text for all nodes mentioning the changed node
   * Fetches the node from the database (legacy method, prefer syncLinkTextForNodeWithData)
   */
  public async syncLinkTextForNode(nodeId: string): Promise<LinkUpdateResult[]> {
    try {
      const updatedNode = await this.databaseService.getNode(nodeId);
      if (!updatedNode) {
        console.warn(`MentionSyncService: Node ${nodeId} not found`);
        return [];
      }

      // Skip sync if node has no content (blank nodes don't have titles to sync)
      if (!updatedNode.content || updatedNode.content.trim() === '') {
        return [];
      }

      return this.syncLinkTextForNodeWithData(nodeId, updatedNode);
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      if (!errorMessage.includes('Phase 2/3')) {
        console.error('MentionSyncService: Error syncing link text', { error, nodeId });
      }
      return [];
    }
  }

  /**
   * Sync link text for all nodes mentioning the changed node
   * Uses provided node data (from event or cache) - preferred method to avoid fetching
   */
  private async syncLinkTextForNodeWithData(
    nodeId: string,
    updatedNode: Node
  ): Promise<LinkUpdateResult[]> {
    const startTime = performance.now();
    this.metrics.totalSyncs++;

    try {
      const newTitle = this.extractTitle(updatedNode.content);

      // Query all nodes that mention this node
      const mentioningNodes = await this.databaseService.queryNodes({
        mentionedBy: nodeId
      });

      // Update link text in each mentioning node
      const results: LinkUpdateResult[] = [];
      for (const node of mentioningNodes) {
        const result = await this.updateLinkTextInNode(node, nodeId, newTitle);
        results.push(result); // Always push result, even if no links updated
        if (result.linksUpdated > 0) {
          this.metrics.totalLinksUpdated += result.linksUpdated;
          this.metrics.totalNodesUpdated++;
        }
      }

      // Update performance metrics
      const syncTime = performance.now() - startTime;
      this.metrics.avgSyncTime =
        (this.metrics.avgSyncTime * (this.metrics.totalSyncs - 1) + syncTime) /
        this.metrics.totalSyncs;

      return results;
    } catch (error) {
      // Suppress expected Phase 2/3 implementation errors - these are not critical issues
      const errorMessage = error instanceof Error ? error.message : String(error);
      if (!errorMessage.includes('Phase 2/3')) {
        console.error('MentionSyncService: Error syncing link text', { error, nodeId });
      }
      return [];
    }
  }

  /**
   * Update link text in a single node
   */
  private async updateLinkTextInNode(
    node: Node,
    targetNodeId: string,
    newTitle: string
  ): Promise<LinkUpdateResult> {
    const oldContent = node.content;
    let newContent = oldContent;
    let linksUpdated = 0;

    // Find and replace all links to the target node
    const links = this.findLinksInContent(oldContent, targetNodeId);

    for (const link of links) {
      // Only update if the title has changed
      if (link.displayText !== newTitle) {
        const newLink = `[${newTitle}](nodespace://${targetNodeId})`;
        newContent = newContent.replace(link.fullMatch, newLink);
        linksUpdated++;
      }
    }

    // Persist updated content if changes were made
    if (linksUpdated > 0) {
      await this.databaseService.updateNode(node.id, node.version, { content: newContent });

      // Emit update event for the changed node
      // Note: Use a different reason to prevent re-triggering sync
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: this.serviceName,
        nodeId: node.id,
        updateType: 'metadata', // Use 'metadata' instead of 'content' to avoid re-triggering sync
        previousValue: oldContent,
        newValue: newContent,
        metadata: {
          reason: 'link-text-sync',
          targetNodeId,
          linksUpdated
        }
      });
    }

    return {
      nodeId: node.id,
      oldContent,
      newContent,
      linksUpdated
    };
  }

  // ========================================================================
  // Broken Link Handling
  // ========================================================================

  /**
   * Mark all links to a deleted node as broken
   */
  public async markLinksAsBroken(deletedNodeId: string): Promise<LinkUpdateResult[]> {
    try {
      // Query all nodes that mention the deleted node
      const mentioningNodes = await this.databaseService.queryNodes({
        mentionedBy: deletedNodeId
      });

      // Update links in each mentioning node
      const results: LinkUpdateResult[] = [];
      for (const node of mentioningNodes) {
        const result = await this.markBrokenLinksInNode(node, deletedNodeId);
        if (result.linksUpdated > 0) {
          results.push(result);
          this.metrics.totalLinksUpdated += result.linksUpdated;
          this.metrics.totalNodesUpdated++;
        }
      }

      return results;
    } catch (error) {
      // Suppress expected Phase 2/3 implementation errors - these are not critical issues
      const errorMessage = error instanceof Error ? error.message : String(error);
      if (!errorMessage.includes('Phase 2/3')) {
        console.error('MentionSyncService: Error marking links as broken', { error, deletedNodeId });
      }
      return [];
    }
  }

  /**
   * Mark broken links in a single node
   */
  private async markBrokenLinksInNode(
    node: Node,
    deletedNodeId: string
  ): Promise<LinkUpdateResult> {
    const oldContent = node.content;
    let newContent = oldContent;
    let linksUpdated = 0;

    // Find all links to the deleted node
    const links = this.findLinksInContent(oldContent, deletedNodeId);

    for (const link of links) {
      // Replace with broken link indicator
      // Using data attribute for styling and tooltip support
      const brokenLink = `[Deleted Node](nodespace://${deletedNodeId}?deleted=true)`;
      newContent = newContent.replace(link.fullMatch, brokenLink);
      linksUpdated++;
    }

    // Persist updated content if changes were made
    if (linksUpdated > 0) {
      await this.databaseService.updateNode(node.id, node.version, { content: newContent });

      // Emit update event for the changed node
      // Note: Use 'metadata' instead of 'content' to avoid re-triggering sync
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: this.serviceName,
        nodeId: node.id,
        updateType: 'metadata', // Use 'metadata' instead of 'content' to avoid re-triggering sync
        previousValue: oldContent,
        newValue: newContent,
        metadata: {
          reason: 'mark-broken-links',
          deletedNodeId,
          linksUpdated
        }
      });
    }

    return {
      nodeId: node.id,
      oldContent,
      newContent,
      linksUpdated
    };
  }

  // ========================================================================
  // Markdown Link Parsing
  // ========================================================================

  /**
   * Find all markdown links to a specific node in content
   */
  private findLinksInContent(content: string, targetNodeId: string): MarkdownLink[] {
    const links: MarkdownLink[] = [];
    const pattern = new RegExp(
      `\\[([^\\]]+)\\]\\(nodespace://${targetNodeId}(?:\\?[^)]*)?\\)`,
      'g'
    );

    let match;
    while ((match = pattern.exec(content)) !== null) {
      links.push({
        fullMatch: match[0],
        displayText: match[1],
        uri: `nodespace://${targetNodeId}`,
        nodeId: targetNodeId,
        startIndex: match.index,
        endIndex: match.index + match[0].length
      });
    }

    return links;
  }

  /**
   * Parse all markdown links in content (for testing/debugging)
   */
  public parseAllLinks(content: string): MarkdownLink[] {
    const links: MarkdownLink[] = [];
    this.LINK_PATTERN.lastIndex = 0;

    let match;
    while ((match = this.LINK_PATTERN.exec(content)) !== null) {
      links.push({
        fullMatch: match[0],
        displayText: match[1],
        uri: `nodespace://${match[2]}`,
        nodeId: match[2],
        startIndex: match.index,
        endIndex: match.index + match[0].length
      });
    }

    return links;
  }

  // ========================================================================
  // Title Extraction
  // ========================================================================

  /**
   * Extract title from node content (first line)
   */
  private extractTitle(content: string): string {
    if (!content) return 'Untitled';

    const lines = content.split('\n');
    const firstLine = lines[0].trim();

    // Remove markdown header syntax if present
    const headerMatch = firstLine.match(/^#{1,6}\s*(.*)$/);
    if (headerMatch) {
      return headerMatch[1].trim() || 'Untitled';
    }

    // Return first non-empty line, truncated to reasonable length
    return firstLine.substring(0, 100) || 'Untitled';
  }

  // ========================================================================
  // Performance Metrics
  // ========================================================================

  /**
   * Get performance metrics
   */
  public getMetrics(): typeof this.metrics {
    return { ...this.metrics };
  }

  /**
   * Reset metrics (for testing)
   */
  public resetMetrics(): void {
    this.metrics = {
      totalSyncs: 0,
      totalLinksUpdated: 0,
      totalNodesUpdated: 0,
      avgSyncTime: 0
    };
  }
}

export default MentionSyncService;
