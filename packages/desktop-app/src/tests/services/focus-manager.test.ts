/**
 * Comprehensive tests for FocusManager service
 * Target: 95%+ coverage
 *
 * This file covers all code paths not covered by focus-manager-cursor-positioning.test.ts
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { focusManager, getFocusManager } from '$lib/services/focus-manager.svelte';

describe('FocusManager - Comprehensive Coverage', () => {
  beforeEach(() => {
    // Clear state before each test
    focusManager.clearEditing();
  });

  describe('Pane ID tracking', () => {
    it('should track editing pane ID when focusing a node', () => {
      focusManager.focusNode('test-node', 'pane-1');

      expect(focusManager.editingNodeId).toBe('test-node');
      expect(focusManager.editingPaneId).toBe('pane-1');
    });

    it('should update pane ID when focusing in different pane', () => {
      focusManager.focusNode('test-node', 'pane-1');
      expect(focusManager.editingPaneId).toBe('pane-1');

      focusManager.focusNode('test-node', 'pane-2');
      expect(focusManager.editingPaneId).toBe('pane-2');
    });

    it('should track pane ID for focusNodeAtPosition', () => {
      focusManager.focusNodeAtPosition('test-node', 10, 'custom-pane');
      expect(focusManager.editingPaneId).toBe('custom-pane');
    });

    it('should track pane ID for focusNodeAtLine', () => {
      focusManager.focusNodeAtLine('test-node', 'sidebar-pane', 2);
      expect(focusManager.editingPaneId).toBe('sidebar-pane');
    });

    it('should track pane ID for focusNodeFromArrowNav', () => {
      focusManager.focusNodeFromArrowNav('test-node', 'down', 100, 'main-pane');
      expect(focusManager.editingPaneId).toBe('main-pane');
    });
  });

  describe('Node type conversion', () => {
    it('focusNodeFromTypeConversion should set node-type-conversion cursor position', () => {
      focusManager.focusNodeFromTypeConversion('converted-node', 25, 'default');

      expect(focusManager.editingNodeId).toBe('converted-node');
      expect(focusManager.editingPaneId).toBe('default');
      expect(focusManager.cursorPosition).toEqual({
        type: 'node-type-conversion',
        position: 25
      });
      expect(focusManager.nodeTypeConversionCursorPosition).toBe(25);
    });

    it('focusNodeFromTypeConversion should clear other cursor states', () => {
      // Set arrow navigation first
      focusManager.focusNodeFromArrowNav('node-1', 'up', 50, 'default');
      expect(focusManager.arrowNavDirection).toBe('up');

      // Convert node type - should clear arrow nav
      focusManager.focusNodeFromTypeConversion('node-2', 30, 'default');

      expect(focusManager.pendingCursorPosition).toBeNull();
      expect(focusManager.arrowNavDirection).toBeNull();
      expect(focusManager.arrowNavPixelOffset).toBeNull();
    });

    it('setEditingNodeFromTypeConversion (legacy) should set conversion cursor position', () => {
      focusManager.setEditingNodeFromTypeConversion('legacy-converted', 42, 'pane-1');

      expect(focusManager.editingNodeId).toBe('legacy-converted');
      expect(focusManager.editingPaneId).toBe('pane-1');
      expect(focusManager.cursorPosition).toEqual({
        type: 'node-type-conversion',
        position: 42
      });
      expect(focusManager.nodeTypeConversionCursorPosition).toBe(42);
      expect(focusManager.pendingCursorPosition).toBeNull();
    });

    it('clearNodeTypeConversionCursorPosition should clear legacy state', () => {
      focusManager.focusNodeFromTypeConversion('node', 20, 'default');
      expect(focusManager.nodeTypeConversionCursorPosition).toBe(20);

      focusManager.clearNodeTypeConversionCursorPosition();

      expect(focusManager.nodeTypeConversionCursorPosition).toBeNull();
      // Unified cursor position should remain (action will consume it)
      expect(focusManager.cursorPosition).toEqual({
        type: 'node-type-conversion',
        position: 20
      });
    });
  });

  describe('Inherited type node', () => {
    it('setEditingNodeFromInheritedType should set inherited-type cursor position', () => {
      focusManager.setEditingNodeFromInheritedType('inherited-node', 15, 'default');

      expect(focusManager.editingNodeId).toBe('inherited-node');
      expect(focusManager.editingPaneId).toBe('default');
      expect(focusManager.cursorPosition).toEqual({
        type: 'inherited-type',
        position: 15
      });
    });

    it('setEditingNodeFromInheritedType should clear all other cursor states', () => {
      // Set various states first
      focusManager.focusNodeFromArrowNav('node-1', 'down', 100, 'default');
      focusManager.setEditingNodeFromTypeConversion('node-2', 30, 'default');

      // Set inherited type - should clear everything
      focusManager.setEditingNodeFromInheritedType('inherited', 20, 'pane-1');

      expect(focusManager.pendingCursorPosition).toBeNull();
      expect(focusManager.arrowNavDirection).toBeNull();
      expect(focusManager.arrowNavPixelOffset).toBeNull();
      expect(focusManager.nodeTypeConversionCursorPosition).toBeNull();
      expect(focusManager.cursorPosition).toEqual({
        type: 'inherited-type',
        position: 20
      });
    });

    it('setEditingNodeFromInheritedType should track pane ID', () => {
      focusManager.setEditingNodeFromInheritedType('node', 10, 'custom-pane');
      expect(focusManager.editingPaneId).toBe('custom-pane');
    });
  });

  describe('Arrow navigation context', () => {
    it('arrowNavigationContext should return null when direction is null', () => {
      focusManager.focusNode('node', 'default');
      expect(focusManager.arrowNavigationContext).toBeNull();
    });

    it('arrowNavigationContext should return null when pixelOffset is null', () => {
      // Manually set up incomplete arrow nav state (edge case)
      focusManager.focusNode('node', 'default');
      expect(focusManager.arrowNavigationContext).toBeNull();
    });

    it('arrowNavigationContext should return context when both values are set', () => {
      focusManager.focusNodeFromArrowNav('node', 'up', 150, 'default');
      expect(focusManager.arrowNavigationContext).toEqual({
        direction: 'up',
        pixelOffset: 150
      });
    });

    it('clearArrowNavigationContext should clear legacy arrow nav state', () => {
      focusManager.focusNodeFromArrowNav('node', 'down', 200, 'default');
      expect(focusManager.arrowNavDirection).toBe('down');
      expect(focusManager.arrowNavPixelOffset).toBe(200);

      focusManager.clearArrowNavigationContext();

      expect(focusManager.arrowNavDirection).toBeNull();
      expect(focusManager.arrowNavPixelOffset).toBeNull();
      // Unified cursor position should remain (action will consume it)
      expect(focusManager.cursorPosition).toEqual({
        type: 'arrow-navigation',
        direction: 'down',
        pixelOffset: 200
      });
    });
  });

  describe('isNodeEditing', () => {
    it('should return true when the node is being edited', () => {
      focusManager.focusNode('test-node', 'default');
      expect(focusManager.isNodeEditing('test-node')).toBe(true);
    });

    it('should return false when a different node is being edited', () => {
      focusManager.focusNode('node-1', 'default');
      expect(focusManager.isNodeEditing('node-2')).toBe(false);
    });

    it('should return false when no node is being edited', () => {
      focusManager.clearEditing();
      expect(focusManager.isNodeEditing('any-node')).toBe(false);
    });

    it('should update when editing state changes', () => {
      focusManager.focusNode('node-1', 'default');
      expect(focusManager.isNodeEditing('node-1')).toBe(true);

      focusManager.focusNode('node-2', 'default');
      expect(focusManager.isNodeEditing('node-1')).toBe(false);
      expect(focusManager.isNodeEditing('node-2')).toBe(true);
    });
  });

  describe('setEditingNode (legacy) - null handling', () => {
    it('should clear editing state when nodeId is null', () => {
      focusManager.focusNode('node-1', 'default');
      expect(focusManager.editingNodeId).toBe('node-1');

      focusManager.setEditingNode(null, 'default');

      expect(focusManager.editingNodeId).toBeNull();
      expect(focusManager.cursorPosition).toBeNull();
      expect(focusManager.pendingCursorPosition).toBeNull();
    });

    it('should clear cursor position when nodeId is null even with cursor param', () => {
      focusManager.focusNodeAtPosition('node-1', 50, 'default');

      focusManager.setEditingNode(null, 'default', 100);

      expect(focusManager.editingNodeId).toBeNull();
      expect(focusManager.cursorPosition).toBeNull();
    });
  });

  describe('Legacy API - state clearing', () => {
    it('setEditingNodeFromArrowNavigation should clear nodeTypeConversionCursorPosition', () => {
      focusManager.focusNodeFromTypeConversion('node-1', 30, 'default');
      expect(focusManager.nodeTypeConversionCursorPosition).toBe(30);

      focusManager.setEditingNodeFromArrowNavigation('node-2', 'up', 100, 'default');

      expect(focusManager.nodeTypeConversionCursorPosition).toBeNull();
    });

    it('clearEditing should clear nodeTypeConversionCursorPosition', () => {
      focusManager.focusNodeFromTypeConversion('node', 40, 'default');
      expect(focusManager.nodeTypeConversionCursorPosition).toBe(40);

      focusManager.clearEditing();

      expect(focusManager.nodeTypeConversionCursorPosition).toBeNull();
    });
  });

  describe('getCurrentState', () => {
    it('should include arrowNavigationContext in state', () => {
      focusManager.focusNodeFromArrowNav('node', 'down', 75, 'default');

      const state = focusManager.getCurrentState();

      expect(state.nodeId).toBe('node');
      expect(state.cursorPosition).toEqual({
        type: 'arrow-navigation',
        direction: 'down',
        pixelOffset: 75
      });
      expect(state.arrowNavigationContext).toEqual({
        direction: 'down',
        pixelOffset: 75
      });
    });

    it('should return null arrowNavigationContext when not set', () => {
      focusManager.focusNode('node', 'default');

      const state = focusManager.getCurrentState();

      expect(state.arrowNavigationContext).toBeNull();
    });
  });

  describe('getFocusManager (legacy export)', () => {
    it('should return the same focusManager instance', () => {
      const manager = getFocusManager();
      expect(manager).toBe(focusManager);
    });

    it('should maintain state across both references', () => {
      const manager = getFocusManager();
      manager.focusNode('test-node', 'default');

      expect(focusManager.editingNodeId).toBe('test-node');
      expect(manager.editingNodeId).toBe('test-node');
    });
  });

  describe('Edge cases and state transitions', () => {
    it('should handle rapid state changes without conflicts', () => {
      focusManager.focusNode('node-1', 'default');
      focusManager.focusNodeAtPosition('node-2', 10, 'default');
      focusManager.focusNodeFromArrowNav('node-3', 'up', 50, 'default');
      focusManager.focusNodeFromTypeConversion('node-4', 20, 'default');
      focusManager.setEditingNodeFromInheritedType('node-5', 15, 'default');

      expect(focusManager.editingNodeId).toBe('node-5');
      expect(focusManager.cursorPosition).toEqual({
        type: 'inherited-type',
        position: 15
      });
    });

    it('should handle clearing state multiple times', () => {
      focusManager.focusNode('node', 'default');
      focusManager.clearEditing();
      focusManager.clearEditing();

      expect(focusManager.editingNodeId).toBeNull();
      expect(focusManager.cursorPosition).toBeNull();
    });

    it('should handle clearing cursor position when no cursor is set', () => {
      focusManager.clearCursorPosition();
      expect(focusManager.cursorPosition).toBeNull();
      expect(focusManager.pendingCursorPosition).toBeNull();
    });

    it('should maintain pane ID through cursor position changes', () => {
      focusManager.focusNode('node', 'pane-1');
      expect(focusManager.editingPaneId).toBe('pane-1');

      focusManager.focusNodeAtPosition('node', 20, 'pane-2');
      expect(focusManager.editingPaneId).toBe('pane-2');

      focusManager.clearCursorPosition();
      // Pane ID should remain after cursor clear (editing still active)
      expect(focusManager.editingPaneId).toBe('pane-2');
    });
  });

  describe('Legacy and new API interoperability', () => {
    it('should maintain consistency when mixing legacy and new APIs', () => {
      // Start with new API
      focusManager.focusNode('node-1', 'default');
      expect(focusManager.cursorPosition?.type).toBe('default');

      // Use legacy API
      focusManager.setEditingNode('node-2', 'default', 25);
      expect(focusManager.cursorPosition).toEqual({ type: 'absolute', position: 25 });
      expect(focusManager.pendingCursorPosition).toBe(25);

      // Back to new API
      focusManager.focusNodeFromArrowNav('node-3', 'down', 100, 'default');
      expect(focusManager.cursorPosition?.type).toBe('arrow-navigation');
      expect(focusManager.arrowNavDirection).toBe('down');
    });

    it('should handle legacy clear methods without breaking new state', () => {
      focusManager.focusNodeFromArrowNav('node', 'up', 150, 'default');

      focusManager.clearArrowNavigationContext();
      expect(focusManager.arrowNavDirection).toBeNull();
      // But cursorPosition should still exist for action to consume
      expect(focusManager.cursorPosition?.type).toBe('arrow-navigation');

      focusManager.clearCursorPosition();
      expect(focusManager.cursorPosition).toBeNull();
    });
  });

  describe('Multiple pane scenarios', () => {
    it('should switch editing between panes correctly', () => {
      focusManager.focusNode('node-1', 'left-pane');
      expect(focusManager.editingPaneId).toBe('left-pane');

      focusManager.focusNode('node-2', 'right-pane');
      expect(focusManager.editingPaneId).toBe('right-pane');
      expect(focusManager.editingNodeId).toBe('node-2');
    });

    it('should track pane through type conversion', () => {
      focusManager.focusNode('node', 'pane-1');
      focusManager.focusNodeFromTypeConversion('node', 10, 'pane-2');
      expect(focusManager.editingPaneId).toBe('pane-2');
    });

    it('should track pane through inherited type', () => {
      focusManager.setEditingNodeFromInheritedType('node', 5, 'special-pane');
      expect(focusManager.editingPaneId).toBe('special-pane');
    });
  });
});
