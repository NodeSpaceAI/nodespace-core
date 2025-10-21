import { backendAdapter } from './backend-adapter';

/**
 * Service for automatically syncing mention relationships when node content changes.
 *
 * Responsibilities:
 * - Extract nodespace:// links from node content (both markdown and plain formats)
 * - Detect changes in mentions between old and new content
 * - Automatically create/delete mention relationships in the database
 * - Prevent self-references and invalid mentions
 *
 * Integration Example:
 * Called automatically by `SharedNodeStore.setNode()` whenever node content changes.
 * ```typescript
 * // User edits node content from "See [@NodeA](...)" to "See [@NodeB](...)"
 * // SharedNodeStore automatically calls:
 * const oldContent = 'See [@Node A](nodespace://abc123-...)';
 * const newContent = 'See [@Node B](nodespace://def456-...)';
 * await mentionSyncService.syncMentions('node-123', oldContent, newContent);
 * // Result: Mention relationship to abc123 deleted, relationship to def456 created
 * ```
 *
 * Manual Usage (rarely needed - typically handled by SharedNodeStore):
 * ```typescript
 * await mentionSyncService.syncMentions('node-123', oldContent, newContent);
 * ```
 */
export class MentionSyncService {
  /**
   * Extract nodespace:// links from content
   *
   * Supports both markdown format and plain URIs:
   * - Markdown: [@text](nodespace://node-id)
   * - Plain: nodespace://node-id
   *
   * Returns array of unique mentioned node IDs (duplicates removed).
   *
   * @param content - The content to parse for mentions
   * @returns Array of unique mentioned node IDs
   */
  extractMentions(content: string): string[] {
    const mentions = new Set<string>();

    // Match markdown format: [@text](nodespace://node-id)
    // Handles optional node/ prefix and query parameters
    const markdownRegex = /\[@[^\]]+\]\(nodespace:\/\/(?:node\/)?([a-f0-9-]{36})(?:\?[^)]*)?\)/gi;
    let match;

    while ((match = markdownRegex.exec(content)) !== null) {
      mentions.add(match[1]); // Extract node ID
    }

    // Match plain format: nodespace://node-id (not inside markdown links)
    // To avoid matching nodespace:// that's already in markdown format, we look for
    // nodespace:// that's NOT preceded by ](
    const plainRegex = /nodespace:\/\/(?:node\/)?([a-f0-9-]{36})/gi;
    let plainMatch;
    const markdownRegex2 = /\[@[^\]]+\]\(nodespace:\/\/(?:node\/)?([a-f0-9-]{36})(?:\?[^)]*)?\)/gi;
    const markdownMatches = new Set<string>();

    // First, collect all markdown matches to exclude from plain matches
    while ((match = markdownRegex2.exec(content)) !== null) {
      markdownMatches.add(match[1]);
    }

    // Then, add plain format matches that aren't already in markdown
    while ((plainMatch = plainRegex.exec(content)) !== null) {
      if (!markdownMatches.has(plainMatch[1])) {
        mentions.add(plainMatch[1]);
      }
    }

    return Array.from(mentions);
  }

  /**
   * Sync mentions when node content changes
   *
   * Compares old vs new mentions and updates database:
   * - Adds new mention relationships
   * - Removes deleted mention relationships
   * - Prevents self-references
   * - Ignores errors (logs warnings but doesn't block saves)
   *
   * This is called automatically when node content is saved.
   *
   * @param nodeId - The node whose content changed
   * @param oldContent - Previous content (undefined if new node)
   * @param newContent - New content
   */
  async syncMentions(
    nodeId: string,
    oldContent: string | undefined,
    newContent: string
  ): Promise<void> {
    const oldMentions = new Set(oldContent ? this.extractMentions(oldContent) : []);
    const newMentions = new Set(this.extractMentions(newContent));

    // Calculate diff
    const toAdd = [...newMentions].filter((id) => !oldMentions.has(id));
    const toRemove = [...oldMentions].filter((id) => !newMentions.has(id));

    // Don't create self-references
    const validToAdd = toAdd.filter((id) => id !== nodeId);
    const validToRemove = toRemove.filter((id) => id !== nodeId);

    // Update database
    for (const mentionedId of validToAdd) {
      try {
        await backendAdapter.createNodeMention(nodeId, mentionedId);
      } catch (error) {
        console.warn(`Failed to create mention: ${nodeId} -> ${mentionedId}`, error);
      }
    }

    for (const mentionedId of validToRemove) {
      try {
        await backendAdapter.deleteNodeMention(nodeId, mentionedId);
      } catch (error) {
        console.warn(`Failed to delete mention: ${nodeId} -> ${mentionedId}`, error);
      }
    }
  }
}

export const mentionSyncService = new MentionSyncService();
