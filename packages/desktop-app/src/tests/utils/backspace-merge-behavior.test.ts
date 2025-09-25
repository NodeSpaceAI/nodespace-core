/**
 * Backspace Merge Behavior Tests
 *
 * Tests backspace functionality with both formatted and unformatted content
 * to ensure merge behavior works correctly in all scenarios.
 */

import { describe, it, expect } from 'vitest';
import { JSDOM } from 'jsdom';
import { ContentEditableController } from '../../lib/design/components/contentEditableController';

// Setup DOM environment
const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
globalThis.document = dom.window.document;
globalThis.window = dom.window as unknown as Window & typeof globalThis;
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.Text = dom.window.Text;
globalThis.Range = dom.window.Range;
globalThis.Selection = dom.window.Selection;
globalThis.Event = dom.window.Event;
globalThis.FocusEvent = dom.window.FocusEvent;
globalThis.KeyboardEvent = dom.window.KeyboardEvent;
globalThis.InputEvent = dom.window.InputEvent;
globalThis.NodeFilter = dom.window.NodeFilter;

interface EventCallRecord {
  deleteNode?: Array<{ nodeId: string }>;
  combineWithPrevious?: Array<{ nodeId: string; currentContent: string }>;
}

describe('Backspace Merge Behavior', () => {
  function createController(initialContent: string = '') {
    const div = document.createElement('div');
    div.contentEditable = 'true';
    div.textContent = initialContent;
    document.body.appendChild(div);

    const eventCalls: EventCallRecord = {};

    const events = {
      contentChanged: () => {},
      headerLevelChanged: () => {},
      focus: () => {},
      blur: () => {},
      createNewNode: () => {},
      indentNode: () => {},
      outdentNode: () => {},
      navigateArrow: () => {},
      combineWithPrevious: (data: { nodeId: string; currentContent: string }) => {
        eventCalls.combineWithPrevious = eventCalls.combineWithPrevious || [];
        eventCalls.combineWithPrevious.push(data);
      },
      deleteNode: (data: { nodeId: string }) => {
        eventCalls.deleteNode = eventCalls.deleteNode || [];
        eventCalls.deleteNode.push(data);
      },
      triggerDetected: () => {},
      triggerHidden: () => {},
      nodeReferenceSelected: () => {},
      slashCommandDetected: () => {},
      slashCommandHidden: () => {},
      slashCommandSelected: () => {},
      nodeTypeConversionDetected: () => {},
      directSlashCommand: () => {}
    };

    const controller = new ContentEditableController(div, 'test-node', 'text', events);
    controller.initialize(initialContent, true); // Start in editing mode

    return { controller, eventCalls, div };
  }

  function setCursorAtBeginning(element: HTMLElement) {
    const range = document.createRange();
    const selection = window.getSelection();
    if (!selection) return;

    // Use TreeWalker to find the first text position
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);

    const firstTextNode = walker.nextNode();
    if (firstTextNode) {
      range.setStart(firstTextNode, 0);
      range.collapse(true);
      selection.removeAllRanges();
      selection.addRange(range);
    }
  }

  function simulateBackspaceKey(element: HTMLElement) {
    const event = new KeyboardEvent('keydown', {
      key: 'Backspace',
      code: 'Backspace',
      bubbles: true,
      cancelable: true
    });
    element.dispatchEvent(event);
  }

  describe('Unformatted Content', () => {
    it('should trigger combineWithPrevious when backspacing at beginning of non-empty node', () => {
      const { eventCalls, div } = createController('Regular text content');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: 'Regular text content'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });

    it('should trigger deleteNode when backspacing at beginning of empty node', () => {
      const { eventCalls, div } = createController('');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.deleteNode).toHaveLength(1);
      expect(eventCalls.deleteNode![0]).toMatchObject({
        nodeId: 'test-node'
      });
      expect(eventCalls.combineWithPrevious).toBeUndefined();
    });
  });

  describe('Formatted Content', () => {
    it('should trigger combineWithPrevious when backspacing at beginning of bold content', () => {
      const { eventCalls, div } = createController('**bold text**');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: '**bold text**'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });

    it('should trigger combineWithPrevious when backspacing at beginning of italic content', () => {
      const { eventCalls, div } = createController('*italic text*');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: '*italic text*'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });

    it('should trigger combineWithPrevious when backspacing at beginning of strikethrough content', () => {
      const { eventCalls, div } = createController('~~strikethrough text~~');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: '~~strikethrough text~~'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });

    it('should trigger combineWithPrevious when backspacing at beginning of code content', () => {
      const { eventCalls, div } = createController('`code text`');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: '`code text`'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });

    it('should trigger combineWithPrevious when backspacing at beginning of header content', () => {
      const { eventCalls, div } = createController('# Header text');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: '# Header text'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });
  });

  describe('Mixed Content', () => {
    it('should trigger combineWithPrevious when backspacing at beginning of mixed formatted content', () => {
      const { eventCalls, div } = createController('**bold** and *italic* text');
      setCursorAtBeginning(div);

      simulateBackspaceKey(div);

      expect(eventCalls.combineWithPrevious).toHaveLength(1);
      expect(eventCalls.combineWithPrevious![0]).toMatchObject({
        nodeId: 'test-node',
        currentContent: '**bold** and *italic* text'
      });
      expect(eventCalls.deleteNode).toBeUndefined();
    });
  });

  describe('Edge Cases', () => {
    it('should not trigger combine/delete when backspacing in middle of content', () => {
      const { eventCalls, div } = createController('**bold text**');

      // Set cursor in middle of content (position 5 = "l" in "bold")
      const range = document.createRange();
      const selection = window.getSelection();
      if (selection) {
        const walker = document.createTreeWalker(div, NodeFilter.SHOW_TEXT, null);
        let currentPosition = 0;
        let node = walker.nextNode();

        while (node && currentPosition + node.textContent!.length < 5) {
          currentPosition += node.textContent!.length;
          node = walker.nextNode();
        }

        if (node) {
          const offset = 5 - currentPosition;
          range.setStart(node, offset);
          range.collapse(true);
          selection.removeAllRanges();
          selection.addRange(range);
        }
      }

      simulateBackspaceKey(div);

      // Should not trigger merge operations when not at beginning
      expect(eventCalls.combineWithPrevious).toBeUndefined();
      expect(eventCalls.deleteNode).toBeUndefined();
    });
  });
});
