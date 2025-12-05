import { describe, it, expect, beforeEach } from 'vitest';
import {
  type NavigableNode,
  type NavigationState,
  getVisibleNodes,
  findNodeById,
  getNodeDepth,
  calculateVisualCursorPosition,
  calculateCursorPosition,
  isAtNodeBoundary,
  getTargetNode,
  getNavigationContext,
  resetNavigationState
} from '$lib/utils/navigation-utils';

describe('navigation-utils', () => {
  // Helper function to create test nodes
  function createNode(id: string, children: NavigableNode[] = [], expanded?: boolean): NavigableNode {
    return { id, children, expanded };
  }

  describe('getVisibleNodes', () => {
    it('should return single node with no children', () => {
      const nodes = [createNode('node-1')];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(1);
      expect(visible[0].id).toBe('node-1');
    });

    it('should return all nodes when all are expanded', () => {
      const nodes = [
        createNode('node-1', [
          createNode('node-1-1'),
          createNode('node-1-2')
        ], true)
      ];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(3);
      expect(visible[0].id).toBe('node-1');
      expect(visible[1].id).toBe('node-1-1');
      expect(visible[2].id).toBe('node-1-2');
    });

    it('should only include expanded nodes and their children', () => {
      const nodes = [
        createNode('node-1', [
          createNode('node-1-1'),
          createNode('node-1-2')
        ], false),
        createNode('node-2', [
          createNode('node-2-1')
        ], true)
      ];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(3);
      expect(visible[0].id).toBe('node-1');
      expect(visible[1].id).toBe('node-2');
      expect(visible[2].id).toBe('node-2-1');
    });

    it('should treat undefined expanded as expanded', () => {
      const nodes = [
        createNode('node-1', [
          createNode('node-1-1')
        ])
      ];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(2);
      expect(visible[0].id).toBe('node-1');
      expect(visible[1].id).toBe('node-1-1');
    });

    it('should handle deeply nested hierarchy', () => {
      const nodes = [
        createNode('node-1', [
          createNode('node-1-1', [
            createNode('node-1-1-1', [
              createNode('node-1-1-1-1')
            ], true)
          ], true)
        ], true)
      ];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(4);
      expect(visible.map(n => n.id)).toEqual([
        'node-1',
        'node-1-1',
        'node-1-1-1',
        'node-1-1-1-1'
      ]);
    });

    it('should handle multiple root nodes', () => {
      const nodes = [
        createNode('node-1', [createNode('node-1-1')], true),
        createNode('node-2', [createNode('node-2-1')], true),
        createNode('node-3')
      ];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(5);
      expect(visible.map(n => n.id)).toEqual([
        'node-1',
        'node-1-1',
        'node-2',
        'node-2-1',
        'node-3'
      ]);
    });

    it('should handle empty array', () => {
      const visible = getVisibleNodes([]);
      expect(visible).toHaveLength(0);
    });

    it('should handle collapsed parent with nested children', () => {
      const nodes = [
        createNode('node-1', [
          createNode('node-1-1', [
            createNode('node-1-1-1')
          ], true)
        ], false)
      ];
      const visible = getVisibleNodes(nodes);

      expect(visible).toHaveLength(1);
      expect(visible[0].id).toBe('node-1');
    });
  });

  describe('findNodeById', () => {
    let testNodes: NavigableNode[];

    beforeEach(() => {
      testNodes = [
        createNode('node-1', [
          createNode('node-1-1'),
          createNode('node-1-2', [
            createNode('node-1-2-1')
          ])
        ]),
        createNode('node-2', [
          createNode('node-2-1')
        ])
      ];
    });

    it('should find root level node', () => {
      const found = findNodeById(testNodes, 'node-1');
      expect(found).not.toBeNull();
      expect(found?.id).toBe('node-1');
    });

    it('should find nested child node', () => {
      const found = findNodeById(testNodes, 'node-1-1');
      expect(found).not.toBeNull();
      expect(found?.id).toBe('node-1-1');
    });

    it('should find deeply nested node', () => {
      const found = findNodeById(testNodes, 'node-1-2-1');
      expect(found).not.toBeNull();
      expect(found?.id).toBe('node-1-2-1');
    });

    it('should return null for non-existent node', () => {
      const found = findNodeById(testNodes, 'non-existent');
      expect(found).toBeNull();
    });

    it('should return null for empty array', () => {
      const found = findNodeById([], 'node-1');
      expect(found).toBeNull();
    });

    it('should find node in second root branch', () => {
      const found = findNodeById(testNodes, 'node-2-1');
      expect(found).not.toBeNull();
      expect(found?.id).toBe('node-2-1');
    });

    it('should handle node with no children', () => {
      const nodes = [createNode('single-node')];
      const found = findNodeById(nodes, 'single-node');
      expect(found).not.toBeNull();
      expect(found?.id).toBe('single-node');
    });
  });

  describe('getNodeDepth', () => {
    let testNodes: NavigableNode[];

    beforeEach(() => {
      testNodes = [
        createNode('node-1', [
          createNode('node-1-1', [
            createNode('node-1-1-1', [
              createNode('node-1-1-1-1')
            ])
          ]),
          createNode('node-1-2')
        ]),
        createNode('node-2', [
          createNode('node-2-1')
        ])
      ];
    });

    it('should return 0 for root level node', () => {
      const depth = getNodeDepth(testNodes, 'node-1');
      expect(depth).toBe(0);
    });

    it('should return 1 for first level child', () => {
      const depth = getNodeDepth(testNodes, 'node-1-1');
      expect(depth).toBe(1);
    });

    it('should return 2 for second level child', () => {
      const depth = getNodeDepth(testNodes, 'node-1-1-1');
      expect(depth).toBe(2);
    });

    it('should return 3 for third level child', () => {
      const depth = getNodeDepth(testNodes, 'node-1-1-1-1');
      expect(depth).toBe(3);
    });

    it('should return 0 for non-existent node', () => {
      const depth = getNodeDepth(testNodes, 'non-existent');
      expect(depth).toBe(0);
    });

    it('should return 0 for empty array', () => {
      const depth = getNodeDepth([], 'node-1');
      expect(depth).toBe(0);
    });

    it('should handle multiple branches correctly', () => {
      const depth1 = getNodeDepth(testNodes, 'node-1-2');
      const depth2 = getNodeDepth(testNodes, 'node-2-1');
      expect(depth1).toBe(1);
      expect(depth2).toBe(1);
    });

    it('should return correct depth for second root node', () => {
      const depth = getNodeDepth(testNodes, 'node-2');
      expect(depth).toBe(0);
    });
  });

  describe('calculateVisualCursorPosition', () => {
    describe('up arrow navigation', () => {
      it('should position at end of last line for single line content', () => {
        const content = 'Hello world';
        const position = calculateVisualCursorPosition(content, true, 5, 0, 0);
        expect(position).toBe(5); // column 5 in single line
      });

      it('should position at end of last line for multi-line content', () => {
        const content = 'Line 1\nLine 2\nLine 3';
        const position = calculateVisualCursorPosition(content, true, 3, 0, 0);
        // lastLineStart = 20 - 6 = 14, adjustedColumn = 3, position = 14 + 3 = 17
        expect(position).toBe(17); // column 3 in last line
      });

      it('should respect column position in last line', () => {
        const content = 'Line 1\nLine 2\nLonger last line';
        const position = calculateVisualCursorPosition(content, true, 5, 0, 0);
        // lastLineStart = 30 - 16 = 14, adjustedColumn = 5, position = 14 + 5 = 19
        expect(position).toBe(19);
      });

      it('should adjust for depth difference when moving to shallower node', () => {
        const content = 'Target content';
        // Moving from depth 2 to depth 0 = -2 depth diff
        // adjustedColumn = 10 - (-2)*2 = 10 - (-4) = 14
        // limited to lastLine.length = 14, so position = 0 + 14 = 14
        const position = calculateVisualCursorPosition(content, true, 10, 2, 0);
        expect(position).toBe(14); // end of line
      });

      it('should adjust for depth difference when moving to deeper node', () => {
        const content = 'Short';
        // Moving from depth 0 to depth 2 = +2 depth diff = -4 visual columns
        const position = calculateVisualCursorPosition(content, true, 10, 0, 2);
        expect(position).toBe(5); // min(10 - 2*2, 5) = min(6, 5) = 5
      });

      it('should not go below 0 for adjusted column', () => {
        const content = 'Content';
        const position = calculateVisualCursorPosition(content, true, 2, 0, 5);
        expect(position).toBe(0); // max(0, 2 - 5*2) = max(0, -8) = 0
      });

      it('should handle empty content', () => {
        const content = '';
        const position = calculateVisualCursorPosition(content, true, 5, 0, 0);
        expect(position).toBe(0);
      });

      it('should handle content with empty last line', () => {
        const content = 'Line 1\nLine 2\n';
        const position = calculateVisualCursorPosition(content, true, 5, 0, 0);
        expect(position).toBe(14); // start of empty last line
      });
    });

    describe('down arrow navigation', () => {
      it('should position at start of first line', () => {
        const content = 'Hello world';
        const position = calculateVisualCursorPosition(content, false, 5, 0, 0);
        expect(position).toBe(5);
      });

      it('should position in first line for multi-line content', () => {
        const content = 'Line 1\nLine 2\nLine 3';
        const position = calculateVisualCursorPosition(content, false, 3, 0, 0);
        expect(position).toBe(3);
      });

      it('should respect column position in first line', () => {
        const content = 'Longer first line\nLine 2';
        const position = calculateVisualCursorPosition(content, false, 10, 0, 0);
        expect(position).toBe(10);
      });

      it('should limit to first line length', () => {
        const content = 'Short\nLonger second line';
        const position = calculateVisualCursorPosition(content, false, 10, 0, 0);
        expect(position).toBe(5); // limited by first line length
      });

      it('should adjust for depth difference when moving to shallower node', () => {
        const content = 'Target content';
        const position = calculateVisualCursorPosition(content, false, 10, 2, 0);
        expect(position).toBe(14); // min(10 - (-2)*2, 14) = min(14, 14) = 14
      });

      it('should adjust for depth difference when moving to deeper node', () => {
        const content = 'Target';
        const position = calculateVisualCursorPosition(content, false, 10, 0, 2);
        expect(position).toBe(6); // min(10 - 2*2, 6) = min(6, 6) = 6
      });

      it('should handle empty content', () => {
        const content = '';
        const position = calculateVisualCursorPosition(content, false, 5, 0, 0);
        expect(position).toBe(0);
      });

      it('should handle content with empty first line', () => {
        const content = '\nLine 2\nLine 3';
        const position = calculateVisualCursorPosition(content, false, 5, 0, 0);
        expect(position).toBe(0);
      });

      it('should not go below 0 for adjusted column', () => {
        const content = 'Content';
        const position = calculateVisualCursorPosition(content, false, 2, 0, 5);
        expect(position).toBe(0); // max(0, 2 - 5*2) = 0
      });
    });
  });

  describe('calculateCursorPosition', () => {
    it('should call calculateVisualCursorPosition with same parameters', () => {
      const content = 'Test content';
      const result1 = calculateCursorPosition(content, true, 5, 1, 0);
      const result2 = calculateVisualCursorPosition(content, true, 5, 1, 0);
      expect(result1).toBe(result2);
    });

    it('should handle up arrow', () => {
      const content = 'Line 1\nLine 2';
      const position = calculateCursorPosition(content, true, 3, 0, 0);
      expect(position).toBeGreaterThanOrEqual(0);
    });

    it('should handle down arrow', () => {
      const content = 'Line 1\nLine 2';
      const position = calculateCursorPosition(content, false, 3, 0, 0);
      expect(position).toBeGreaterThanOrEqual(0);
    });
  });

  describe('isAtNodeBoundary', () => {
    describe('single line content', () => {
      it('should detect boundary at start for up arrow', () => {
        const content = 'Hello world';
        const result = isAtNodeBoundary(content, 0, true);
        expect(result.isAtBoundary).toBe(true);
        expect(result.currentLineIndex).toBe(0);
        expect(result.columnInCurrentLine).toBe(0);
      });

      it('should detect boundary at end for down arrow', () => {
        const content = 'Hello world';
        const result = isAtNodeBoundary(content, 11, false);
        expect(result.isAtBoundary).toBe(true);
        expect(result.currentLineIndex).toBe(0);
        expect(result.columnInCurrentLine).toBe(11);
      });

      it('should detect boundary in middle for up arrow (single line)', () => {
        const content = 'Hello world';
        const result = isAtNodeBoundary(content, 5, true);
        expect(result.isAtBoundary).toBe(true);
        expect(result.columnInCurrentLine).toBe(5);
      });

      it('should detect boundary in middle for down arrow (single line)', () => {
        const content = 'Hello world';
        const result = isAtNodeBoundary(content, 5, false);
        expect(result.isAtBoundary).toBe(true);
        expect(result.columnInCurrentLine).toBe(5);
      });
    });

    describe('multi-line content', () => {
      it('should detect boundary at first line for up arrow', () => {
        const content = 'Line 1\nLine 2\nLine 3';
        const result = isAtNodeBoundary(content, 3, true);
        expect(result.isAtBoundary).toBe(true);
        expect(result.currentLineIndex).toBe(0);
        expect(result.columnInCurrentLine).toBe(3);
      });

      it('should not detect boundary at middle line for up arrow', () => {
        const content = 'Line 1\nLine 2\nLine 3';
        const result = isAtNodeBoundary(content, 10, true); // middle of line 2
        expect(result.isAtBoundary).toBe(false);
        expect(result.currentLineIndex).toBe(1);
        expect(result.columnInCurrentLine).toBe(3);
      });

      it('should detect boundary at last line for down arrow', () => {
        const content = 'Line 1\nLine 2\nLine 3';
        const result = isAtNodeBoundary(content, 17, false); // in line 3
        expect(result.isAtBoundary).toBe(true);
        expect(result.currentLineIndex).toBe(2);
        expect(result.columnInCurrentLine).toBe(3);
      });

      it('should not detect boundary at middle line for down arrow', () => {
        const content = 'Line 1\nLine 2\nLine 3';
        const result = isAtNodeBoundary(content, 10, false); // middle of line 2
        expect(result.isAtBoundary).toBe(false);
        expect(result.currentLineIndex).toBe(1);
      });

      it('should handle cursor at line boundary', () => {
        const content = 'Line 1\nLine 2';
        const result = isAtNodeBoundary(content, 6, true); // at newline after Line 1
        expect(result.currentLineIndex).toBe(0);
        expect(result.columnInCurrentLine).toBe(6);
      });

      it('should handle cursor at start of second line', () => {
        const content = 'Line 1\nLine 2';
        const result = isAtNodeBoundary(content, 7, false); // start of Line 2
        expect(result.currentLineIndex).toBe(1);
        expect(result.columnInCurrentLine).toBe(0);
      });
    });

    describe('edge cases', () => {
      it('should handle empty content', () => {
        const content = '';
        const result = isAtNodeBoundary(content, 0, true);
        expect(result.isAtBoundary).toBe(true);
        expect(result.currentLineIndex).toBe(0);
        expect(result.columnInCurrentLine).toBe(0);
      });

      it('should handle content with only newlines', () => {
        const content = '\n\n';
        const result = isAtNodeBoundary(content, 1, false);
        expect(result.currentLineIndex).toBe(1);
      });

      it('should handle cursor at very end', () => {
        const content = 'Line 1\nLine 2';
        const result = isAtNodeBoundary(content, content.length, false);
        expect(result.isAtBoundary).toBe(true);
        expect(result.currentLineIndex).toBe(1);
      });
    });
  });

  describe('getTargetNode', () => {
    let testNodes: NavigableNode[];

    beforeEach(() => {
      testNodes = [
        createNode('node-1', [
          createNode('node-1-1'),
          createNode('node-1-2')
        ], true),
        createNode('node-2', [
          createNode('node-2-1')
        ], false),
        createNode('node-3')
      ];
    });

    describe('down direction', () => {
      it('should return next sibling', () => {
        const target = getTargetNode(testNodes, 'node-1', 'down');
        expect(target).not.toBeNull();
        expect(target?.id).toBe('node-1-1');
      });

      it('should return next visible node skipping collapsed children', () => {
        const target = getTargetNode(testNodes, 'node-2', 'down');
        expect(target).not.toBeNull();
        expect(target?.id).toBe('node-3');
      });

      it('should return null at end of list', () => {
        const target = getTargetNode(testNodes, 'node-3', 'down');
        expect(target).toBeNull();
      });

      it('should navigate through expanded children', () => {
        const target1 = getTargetNode(testNodes, 'node-1-1', 'down');
        expect(target1?.id).toBe('node-1-2');

        const target2 = getTargetNode(testNodes, 'node-1-2', 'down');
        expect(target2?.id).toBe('node-2');
      });
    });

    describe('up direction', () => {
      it('should return previous sibling', () => {
        const target = getTargetNode(testNodes, 'node-1-2', 'up');
        expect(target).not.toBeNull();
        expect(target?.id).toBe('node-1-1');
      });

      it('should return parent when at first child', () => {
        const target = getTargetNode(testNodes, 'node-1-1', 'up');
        expect(target).not.toBeNull();
        expect(target?.id).toBe('node-1');
      });

      it('should return null at start of list', () => {
        const target = getTargetNode(testNodes, 'node-1', 'up');
        expect(target).toBeNull();
      });

      it('should navigate through visible nodes correctly', () => {
        const target = getTargetNode(testNodes, 'node-3', 'up');
        expect(target?.id).toBe('node-2');
      });
    });

    describe('edge cases', () => {
      it('should return null for non-existent node', () => {
        const target = getTargetNode(testNodes, 'non-existent', 'down');
        expect(target).toBeNull();
      });

      it('should return null for empty array', () => {
        const target = getTargetNode([], 'node-1', 'down');
        expect(target).toBeNull();
      });

      it('should handle single node', () => {
        const nodes = [createNode('single')];
        const targetDown = getTargetNode(nodes, 'single', 'down');
        const targetUp = getTargetNode(nodes, 'single', 'up');
        expect(targetDown).toBeNull();
        expect(targetUp).toBeNull();
      });
    });
  });

  describe('getNavigationContext', () => {
    let testNodes: NavigableNode[];

    beforeEach(() => {
      testNodes = [
        createNode('node-1', [
          createNode('node-1-1'),
          createNode('node-1-2', [
            createNode('node-1-2-1')
          ]),
          createNode('node-1-3')
        ]),
        createNode('node-2', [
          createNode('node-2-1')
        ]),
        createNode('node-3')
      ];
    });

    describe('root level nodes', () => {
      it('should return context for first root node', () => {
        const context = getNavigationContext(testNodes, 'node-1');
        expect(context.currentNode?.id).toBe('node-1');
        expect(context.parent).toBeNull();
        expect(context.depth).toBe(0);
        expect(context.isFirstChild).toBe(true);
        expect(context.isLastChild).toBe(false);
        expect(context.nextSibling?.id).toBe('node-2');
        expect(context.prevSibling).toBeNull();
      });

      it('should return context for middle root node', () => {
        const context = getNavigationContext(testNodes, 'node-2');
        expect(context.currentNode?.id).toBe('node-2');
        expect(context.parent).toBeNull();
        expect(context.depth).toBe(0);
        expect(context.isFirstChild).toBe(false);
        expect(context.isLastChild).toBe(false);
        expect(context.nextSibling?.id).toBe('node-3');
        expect(context.prevSibling?.id).toBe('node-1');
      });

      it('should return context for last root node', () => {
        const context = getNavigationContext(testNodes, 'node-3');
        expect(context.currentNode?.id).toBe('node-3');
        expect(context.parent).toBeNull();
        expect(context.depth).toBe(0);
        expect(context.isFirstChild).toBe(false);
        expect(context.isLastChild).toBe(true);
        expect(context.nextSibling).toBeNull();
        expect(context.prevSibling?.id).toBe('node-2');
      });
    });

    describe('child nodes', () => {
      it('should return context for first child', () => {
        const context = getNavigationContext(testNodes, 'node-1-1');
        expect(context.currentNode?.id).toBe('node-1-1');
        expect(context.parent?.id).toBe('node-1');
        expect(context.depth).toBe(1);
        expect(context.isFirstChild).toBe(true);
        expect(context.isLastChild).toBe(false);
        expect(context.nextSibling?.id).toBe('node-1-2');
        expect(context.prevSibling).toBeNull();
      });

      it('should return context for middle child', () => {
        const context = getNavigationContext(testNodes, 'node-1-2');
        expect(context.currentNode?.id).toBe('node-1-2');
        expect(context.parent?.id).toBe('node-1');
        expect(context.depth).toBe(1);
        expect(context.isFirstChild).toBe(false);
        expect(context.isLastChild).toBe(false);
        expect(context.nextSibling?.id).toBe('node-1-3');
        expect(context.prevSibling?.id).toBe('node-1-1');
      });

      it('should return context for last child', () => {
        const context = getNavigationContext(testNodes, 'node-1-3');
        expect(context.currentNode?.id).toBe('node-1-3');
        expect(context.parent?.id).toBe('node-1');
        expect(context.depth).toBe(1);
        expect(context.isFirstChild).toBe(false);
        expect(context.isLastChild).toBe(true);
        expect(context.nextSibling).toBeNull();
        expect(context.prevSibling?.id).toBe('node-1-2');
      });
    });

    describe('deeply nested nodes', () => {
      it('should return context for deeply nested node', () => {
        const context = getNavigationContext(testNodes, 'node-1-2-1');
        expect(context.currentNode?.id).toBe('node-1-2-1');
        expect(context.parent?.id).toBe('node-1-2');
        expect(context.depth).toBe(2);
        expect(context.isFirstChild).toBe(true);
        expect(context.isLastChild).toBe(true);
        expect(context.nextSibling).toBeNull();
        expect(context.prevSibling).toBeNull();
      });
    });

    describe('edge cases', () => {
      it('should return empty context for non-existent node', () => {
        const context = getNavigationContext(testNodes, 'non-existent');
        expect(context.currentNode).toBeNull();
        expect(context.parent).toBeNull();
        expect(context.depth).toBe(0);
        expect(context.isFirstChild).toBe(false);
        expect(context.isLastChild).toBe(false);
        expect(context.nextSibling).toBeNull();
        expect(context.prevSibling).toBeNull();
      });

      it('should return empty context for empty array', () => {
        const context = getNavigationContext([], 'node-1');
        expect(context.currentNode).toBeNull();
        expect(context.parent).toBeNull();
      });

      it('should handle single node with no siblings', () => {
        const nodes = [createNode('single')];
        const context = getNavigationContext(nodes, 'single');
        expect(context.currentNode?.id).toBe('single');
        expect(context.isFirstChild).toBe(true);
        expect(context.isLastChild).toBe(true);
        expect(context.nextSibling).toBeNull();
        expect(context.prevSibling).toBeNull();
      });

      it('should handle only child with parent', () => {
        const nodes = [createNode('parent', [createNode('only-child')])];
        const context = getNavigationContext(nodes, 'only-child');
        expect(context.currentNode?.id).toBe('only-child');
        expect(context.parent?.id).toBe('parent');
        expect(context.isFirstChild).toBe(true);
        expect(context.isLastChild).toBe(true);
        expect(context.nextSibling).toBeNull();
        expect(context.prevSibling).toBeNull();
      });
    });
  });

  describe('resetNavigationState', () => {
    it('should reset preferredColumn to null', () => {
      const state: NavigationState = {
        preferredColumn: 10,
        resetCounter: 0
      };

      resetNavigationState(state);

      expect(state.preferredColumn).toBeNull();
    });

    it('should increment resetCounter', () => {
      const state: NavigationState = {
        preferredColumn: 5,
        resetCounter: 0
      };

      resetNavigationState(state);

      expect(state.resetCounter).toBe(1);
    });

    it('should increment resetCounter multiple times', () => {
      const state: NavigationState = {
        preferredColumn: 5,
        resetCounter: 0
      };

      resetNavigationState(state);
      resetNavigationState(state);
      resetNavigationState(state);

      expect(state.resetCounter).toBe(3);
      expect(state.preferredColumn).toBeNull();
    });

    it('should work when preferredColumn is already null', () => {
      const state: NavigationState = {
        preferredColumn: null,
        resetCounter: 5
      };

      resetNavigationState(state);

      expect(state.preferredColumn).toBeNull();
      expect(state.resetCounter).toBe(6);
    });

    it('should mutate the original state object', () => {
      const state: NavigationState = {
        preferredColumn: 10,
        resetCounter: 0
      };
      const originalState = state;

      resetNavigationState(state);

      expect(state).toBe(originalState);
      expect(state.preferredColumn).toBeNull();
      expect(state.resetCounter).toBe(1);
    });
  });
});
