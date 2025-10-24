import { backendAdapter, type BackendAdapter } from './backend-adapter';

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
  private adapter: BackendAdapter;

  constructor(adapter?: BackendAdapter) {
    this.adapter = adapter ?? backendAdapter;
  }
  /**
   * Validate if a node ID is valid (UUID or date format)
   *
   * Valid formats:
   * - UUID: 36-character hex string with dashes (e.g., "abc123-...")
   * - Date: YYYY-MM-DD format (e.g., "2025-10-24")
   *
   * @param nodeId - The node ID to validate
   * @returns true if valid UUID or date format
   */
  private isValidNodeId(nodeId: string): boolean {
    // Check if it's a UUID (36 characters, hex with dashes)
    const uuidRegex = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
    if (uuidRegex.test(nodeId)) {
      return true;
    }

    // Check if it's a valid date format (YYYY-MM-DD)
    const dateRegex = /^\d{4}-\d{2}-\d{2}$/;
    if (dateRegex.test(nodeId)) {
      // Validate it's an actual valid date
      const [year, month, day] = nodeId.split('-').map(Number);
      const date = new Date(year, month - 1, day);
      return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day;
    }

    return false;
  }

  /**
   * Extract nodespace:// links from content
   *
   * Supports both markdown format and plain URIs:
   * - Markdown: [@text](nodespace://node-id) or [text](nodespace://node-id)
   * - Plain: nodespace://node-id
   *
   * Accepts both UUID and date format node IDs:
   * - UUID: abc123-def456-... (36 chars)
   * - Date: 2025-10-24 (YYYY-MM-DD format)
   *
   * Returns array of unique mentioned node IDs (duplicates removed).
   *
   * @param content - The content to parse for mentions
   * @returns Array of unique mentioned node IDs
   */
  extractMentions(content: string): string[] {
    const mentions = new Set<string>();

    // Match markdown format: [@text](nodespace://node-id) or [text](nodespace://node-id)
    // Captures any non-whitespace after nodespace:// until closing paren or query string
    const markdownRegex = /\[[^\]]+\]\(nodespace:\/\/(?:node\/)?([^\s)?]+)(?:\?[^)]*)?\)/gi;
    let match;

    while ((match = markdownRegex.exec(content)) !== null) {
      const nodeId = match[1];
      if (this.isValidNodeId(nodeId)) {
        mentions.add(nodeId);
      }
    }

    // Match plain format: nodespace://node-id (not inside markdown links)
    const plainRegex = /(?<!\]\()nodespace:\/\/(?:node\/)?([^\s)?]+)/gi;
    let plainMatch;
    const markdownMatches = new Set<string>();

    // Reset regex for second pass
    const markdownRegex2 = /\[[^\]]+\]\(nodespace:\/\/(?:node\/)?([^\s)?]+)(?:\?[^)]*)?\)/gi;

    // First, collect all markdown matches to exclude from plain matches
    while ((match = markdownRegex2.exec(content)) !== null) {
      const nodeId = match[1];
      if (this.isValidNodeId(nodeId)) {
        markdownMatches.add(nodeId);
      }
    }

    // Then, add plain format matches that aren't already in markdown
    while ((plainMatch = plainRegex.exec(content)) !== null) {
      const nodeId = plainMatch[1];
      if (this.isValidNodeId(nodeId) && !markdownMatches.has(nodeId)) {
        mentions.add(nodeId);
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
        await this.adapter.createNodeMention(nodeId, mentionedId);
      } catch (error) {
        console.warn(`Failed to create mention: ${nodeId} -> ${mentionedId}`, error);
      }
    }

    for (const mentionedId of validToRemove) {
      try {
        await this.adapter.deleteNodeMention(nodeId, mentionedId);
      } catch (error) {
        console.warn(`Failed to delete mention: ${nodeId} -> ${mentionedId}`, error);
      }
    }
  }
}

export const mentionSyncService = new MentionSyncService();
