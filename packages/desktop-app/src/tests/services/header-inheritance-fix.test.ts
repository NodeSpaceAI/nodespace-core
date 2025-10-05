/**
 * Header Syntax Inheritance Fix Test
 *
 * Tests the fix for the bug where pressing Enter after header syntax
 * (e.g., "# |") should create a new node above with the same header syntax.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import {
  createReactiveNodeService,
  type NodeManagerEvents
} from '$lib/services/reactiveNodeService.test';

// Helper to create unified Node format
function createNode(
  id: string,
  content: string,
  nodeType: string = 'text',
  parentId: string | null = null
) {
  return {
    id,
    nodeType: nodeType,
    content,
    parentId: parentId,
    originNodeId: null,
    beforeSiblingId: null,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    mentions: [] as string[],
    properties: {}
  };
}

describe('Header Syntax Inheritance Fix', () => {
  let nodeService: ReturnType<typeof createReactiveNodeService>;
  let mockEvents: NodeManagerEvents;

  beforeEach(() => {
    mockEvents = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };
    nodeService = createReactiveNodeService(mockEvents);

    // Initialize with basic structure to have nodes to work with
    nodeService.initializeNodes([createNode('root-node', 'Root node')], {
      expanded: true,
      autoFocus: false,
      inheritHeaderLevel: 0
    });
  });

  describe('Header Syntax Inheritance on Node Creation', () => {
    it('should inherit header syntax when creating empty node after header node', () => {
      // Create a header node using the pre-existing root node
      const headerNodeId = nodeService.createNode('root-node', '# My Header', 'text', 1, false);
      expect(headerNodeId).toBeTruthy();

      const headerNode = nodeService.findNode(headerNodeId);
      expect(headerNode).toBeTruthy();
      expect(headerNode!.content).toBe('# My Header');

      // Create an empty node after the header node (simulating Enter key press)
      // This should inherit the header syntax
      const newNodeId = nodeService.createNode(headerNodeId, '', 'text', undefined, false);
      expect(newNodeId).toBeTruthy();

      const newNode = nodeService.findNode(newNodeId);
      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe('# '); // Should inherit the header syntax
      expect(newNode!.isPlaceholder).toBe(true); // Should be marked as placeholder
    });

    it('should inherit header syntax when creating node with content after header node', () => {
      // Create a header node
      const headerNodeId = nodeService.createNode(
        'root-node',
        '## Section Header',
        'text',
        2,
        false
      );
      expect(headerNodeId).toBeTruthy();

      // Create a node with content after the header node
      // This should inherit the header syntax and prepend it to the content
      const newNodeId = nodeService.createNode(
        headerNodeId,
        'New content',
        'text',
        undefined,
        false
      );
      expect(newNodeId).toBeTruthy();

      const newNode = nodeService.findNode(newNodeId);
      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe('## New content'); // Should inherit header syntax and prepend it
      expect(newNode!.isPlaceholder).toBe(false); // Should not be placeholder since it has content
    });

    it('should not duplicate header syntax if content already has it', () => {
      // Create a header node
      const headerNodeId = nodeService.createNode(
        'root-node',
        '### Original Header',
        'text',
        3,
        false
      );
      expect(headerNodeId).toBeTruthy();

      // Create a node with content that already has header syntax
      // This should not duplicate the header syntax
      const newNodeId = nodeService.createNode(
        headerNodeId,
        '### Already has syntax',
        'text',
        undefined,
        false
      );
      expect(newNodeId).toBeTruthy();

      const newNode = nodeService.findNode(newNodeId);
      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe('### Already has syntax'); // Should not duplicate
      expect(newNode!.isPlaceholder).toBe(false);
    });

    it('should handle different header levels correctly', () => {
      // Test all header levels (1-6)
      for (let level = 1; level <= 6; level++) {
        const headerSyntax = '#'.repeat(level) + ' ';
        const headerContent = headerSyntax + `Header Level ${level}`;

        const headerNodeId = nodeService.createNode(
          'root-node',
          headerContent,
          'text',
          level,
          false
        );
        const newNodeId = nodeService.createNode(headerNodeId, '', 'text', undefined, false);

        const newNode = nodeService.findNode(newNodeId);
        expect(newNode).toBeTruthy();
        expect(newNode!.content).toBe(headerSyntax); // Should inherit correct level

        // Cleanup for next iteration
        nodeService.deleteNode(newNodeId);
        nodeService.deleteNode(headerNodeId);
      }
    });

    it('should not inherit header syntax from non-header nodes', () => {
      // Create a regular text node
      const textNodeId = nodeService.createNode(
        'root-node',
        'Regular text content',
        'text',
        0,
        false
      );
      expect(textNodeId).toBeTruthy();

      // Create a new node after the text node
      // This should not inherit any header syntax
      const newNodeId = nodeService.createNode(textNodeId, '', 'text', undefined, false);
      expect(newNodeId).toBeTruthy();

      const newNode = nodeService.findNode(newNodeId);
      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe(''); // Should be empty, no inheritance
      expect(newNode!.isPlaceholder).toBe(true);
    });

    it('should work correctly with insertAtBeginning parameter', () => {
      // Create a header node
      const headerNodeId = nodeService.createNode('root-node', '# Main Header', 'text', 1, false);
      expect(headerNodeId).toBeTruthy();

      // Create a node at the beginning (above the header node)
      const newNodeId = nodeService.createNode(headerNodeId, '', 'text', undefined, true);
      expect(newNodeId).toBeTruthy();

      const newNode = nodeService.findNode(newNodeId);
      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe('# '); // Should still inherit header syntax
      expect(newNode!.isPlaceholder).toBe(true);
    });

    it('should handle placeholder node creation correctly', () => {
      // Create a header node
      const headerNodeId = nodeService.createNode(
        'root-node',
        '#### Test Header',
        'text',
        4,
        false
      );
      expect(headerNodeId).toBeTruthy();

      // Create a placeholder node after the header node
      const newNodeId = nodeService.createPlaceholderNode(headerNodeId, 'text', undefined, false);
      expect(newNodeId).toBeTruthy();

      const newNode = nodeService.findNode(newNodeId);
      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe('#### '); // Should inherit header syntax
      expect(newNode!.isPlaceholder).toBe(true);
    });
  });

  describe('Enter Key Simulation Integration', () => {
    it('should simulate the Enter key behavior correctly', () => {
      // Simulate the scenario described in the bug report:
      // 1. User has a node with content "# Some text"
      // 2. User positions cursor after "# " (so "# |Some text")
      // 3. User presses Enter
      // 4. Expected: new node above with "# " syntax, current node becomes "Some text"

      const originalNodeId = nodeService.createNode('root-node', '# Some text', 'text', 1, false);
      expect(originalNodeId).toBeTruthy();

      // Simulate the split that happens on Enter press
      // The current node content becomes the part before cursor: "# "
      nodeService.updateNodeContent(originalNodeId, '# ');

      // Create the new node with the part after cursor: "Some text"
      // This simulates what splitMarkdownContent would return as afterContent
      const newNodeId = nodeService.createNode(
        originalNodeId,
        'Some text',
        'text',
        undefined,
        false
      );
      expect(newNodeId).toBeTruthy();

      // Verify the results
      const originalNode = nodeService.findNode(originalNodeId);
      const newNode = nodeService.findNode(newNodeId);

      expect(originalNode).toBeTruthy();
      expect(originalNode!.content).toBe('# '); // Original node keeps header syntax

      expect(newNode).toBeTruthy();
      expect(newNode!.content).toBe('# Some text'); // New node inherits header syntax and gets the content
    });
  });
});
