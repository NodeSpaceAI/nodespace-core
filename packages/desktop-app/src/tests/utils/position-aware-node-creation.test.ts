/**
 * Position-Aware Node Creation Tests
 *
 * Tests the new behavior where pressing Enter at the beginning or within header syntax
 * creates a new node ABOVE instead of splitting content, preserving original node identity.
 */

import { describe, it, expect } from 'vitest';
import { JSDOM } from 'jsdom';
import { ContentEditableController } from '../../lib/design/components/contentEditableController';

// Setup DOM environment
const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
globalThis.document = dom.window.document;
globalThis.window = dom.window as any;
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
  createNewNode?: Array<{
    afterNodeId: string;
    nodeType: string;
    currentContent?: string;
    newContent?: string;
    cursorAtBeginning?: boolean;
    insertAtBeginning?: boolean;
    focusOriginalNode?: boolean;
  }>;
}

describe('Position-Aware Node Creation', () => {
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
      createNewNode: (data: any) => {
        eventCalls.createNewNode = eventCalls.createNewNode || [];
        eventCalls.createNewNode.push(data);
      },
      indentNode: () => {},
      outdentNode: () => {},
      navigateArrow: () => {},
      combineWithPrevious: () => {},
      deleteNode: () => {}
    };

    const controller = new ContentEditableController(div, events);
    controller.initialize('test-node', true);

    return { controller, eventCalls, div };
  }

  function setCursorPosition(element: HTMLElement, position: number) {
    const range = document.createRange();
    const selection = window.getSelection();
    if (!selection) return;

    const textNode = element.firstChild;
    if (textNode && textNode.nodeType === Node.TEXT_NODE) {
      range.setStart(textNode, Math.min(position, textNode.textContent?.length || 0));
      range.collapse(true);
      selection.removeAllRanges();
      selection.addRange(range);
    }
  }

  function simulateEnterKey(element: HTMLElement) {
    const event = new KeyboardEvent('keydown', {
      key: 'Enter',
      code: 'Enter',
      bubbles: true,
      cancelable: true
    });
    element.dispatchEvent(event);
  }

  describe('Header Node Creation Above', () => {
    it('should create new node ABOVE when cursor is at beginning |# Header', () => {
      const { controller, eventCalls, div } = createController('# Header text');
      setCursorPosition(div, 0); // |# Header text

      simulateEnterKey(div);

      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode![0]).toMatchObject({
        afterNodeId: 'test-node',
        nodeType: 'text',
        currentContent: '', // New node above is empty
        newContent: content, // Original node (bottom) keeps its content
        cursorAtBeginning: true,
        insertAtBeginning: true, // Key: Creates node ABOVE
        focusOriginalNode: true // Focus goes to original node (bottom)
      });

      // Original node content should remain unchanged
      expect(div.textContent).toBe('# Header text');
    });

    it('should create new node ABOVE when cursor is within header syntax #| Header', () => {
      const { controller, eventCalls, div } = createController('# Header text');
      setCursorPosition(div, 1); // #| Header text

      simulateEnterKey(div);

      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode![0]).toMatchObject({
        afterNodeId: 'test-node',
        insertAtBeginning: true // Creates node ABOVE
      });

      // Original node content should remain unchanged
      expect(div.textContent).toBe('# Header text');
    });

    it('should handle various header levels correctly', () => {
      for (let level = 1; level <= 6; level++) {
        const headerPrefix = '#'.repeat(level) + ' ';
        const content = headerPrefix + 'Header text';

        const { controller, eventCalls, div } = createController(content);

        // Test cursor at beginning
        setCursorPosition(div, 0);
        simulateEnterKey(div);

        expect(eventCalls.createNewNode).toHaveLength(1);
        expect(eventCalls.createNewNode![0].insertAtBeginning).toBe(true);
        expect(div.textContent).toBe(content); // Original unchanged
      }
    });
  });

  describe('Normal Splitting Behavior', () => {
    it('should create new node ABOVE when cursor is right after header syntax # |Header', () => {
      const { controller, eventCalls, div } = createController('# Header text');
      setCursorPosition(div, 2); // # |Header text

      simulateEnterKey(div);

      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode![0]).toMatchObject({
        insertAtBeginning: true, // Still creates ABOVE - cursor at end of syntax area
        cursorAtBeginning: true
      });

      // Original node content should remain unchanged
      expect(div.textContent).toBe('# Header text');
    });

    it('should use normal splitting when cursor is within the actual content', () => {
      const { controller, eventCalls, div } = createController('# Header text');
      setCursorPosition(div, 5); // # Hea|der text

      simulateEnterKey(div);

      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode![0]).toMatchObject({
        insertAtBeginning: undefined, // Normal behavior - splits content
        cursorAtBeginning: false
      });
    });

    it('should use normal splitting for non-header content', () => {
      const { controller, eventCalls, div } = createController('Regular text content');
      setCursorPosition(div, 7); // Regular| text content

      simulateEnterKey(div);

      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode![0]).toMatchObject({
        insertAtBeginning: undefined, // Normal behavior
        cursorAtBeginning: false
      });
    });
  });

  describe('Inline Formatting Node Creation Above', () => {
    const inlineFormattingCases = [
      { name: 'Bold **text**', content: '**bold text**', positions: [0, 1, 2] },
      { name: 'Bold __text__', content: '__bold text__', positions: [0, 1, 2] },
      { name: 'Italic *text*', content: '*italic text*', positions: [0, 1] },
      { name: 'Italic _text_', content: '_italic text_', positions: [0, 1] },
      { name: 'Strikethrough ~~text~~', content: '~~strikethrough text~~', positions: [0, 1, 2] },
      { name: 'Code `text`', content: '`code text`', positions: [0, 1] },
    ];

    inlineFormattingCases.forEach(({ name, content, positions }) => {
      positions.forEach(pos => {
        it(`should create new node ABOVE when cursor is within ${name} opening syntax at position ${pos}`, () => {
          const { controller, eventCalls, div } = createController(content);
          setCursorPosition(div, pos);

          simulateEnterKey(div);

          expect(eventCalls.createNewNode).toHaveLength(1);
          expect(eventCalls.createNewNode![0]).toMatchObject({
            afterNodeId: 'test-node',
            nodeType: 'text',
            currentContent: '', // New node above is empty
            newContent: '',
            cursorAtBeginning: true,
            insertAtBeginning: true // Key: Creates node ABOVE
          });

          // Original node content should remain unchanged
          expect(div.textContent).toBe(content);
        });
      });

      it(`should use normal splitting when cursor is past ${name} opening syntax`, () => {
        const { controller, eventCalls, div } = createController(content);
        // Set cursor past the opening syntax (in the actual content)
        const pastSyntaxPosition = positions[positions.length - 1] + 2;
        setCursorPosition(div, pastSyntaxPosition);

        simulateEnterKey(div);

        expect(eventCalls.createNewNode).toHaveLength(1);
        expect(eventCalls.createNewNode![0]).toMatchObject({
          insertAtBeginning: undefined, // Normal behavior - splits content
          cursorAtBeginning: false
        });
      });
    });
  });

  describe('shouldCreateNodeAbove Logic', () => {
    it('should return true for position 0 (beginning)', () => {
      const { controller } = createController('# Header text');
      // Access private method via type assertion for testing
      const shouldCreateAbove = (controller as any).shouldCreateNodeAbove('# Header text', 0);
      expect(shouldCreateAbove).toBe(true);
    });

    it('should return true for cursor within header syntax', () => {
      const { controller } = createController('## Header text');
      const shouldCreateAbove = (controller as any).shouldCreateNodeAbove('## Header text', 1);
      expect(shouldCreateAbove).toBe(true);
    });

    it('should return true for cursor right after header syntax', () => {
      const { controller } = createController('# Header text');
      const shouldCreateAbove = (controller as any).shouldCreateNodeAbove('# Header text', 2);
      expect(shouldCreateAbove).toBe(true); // Position 2 is '# |'
    });

    it('should return false for cursor in actual content', () => {
      const { controller } = createController('# Header text');
      const shouldCreateAbove = (controller as any).shouldCreateNodeAbove('# Header text', 3);
      expect(shouldCreateAbove).toBe(false); // Position 3 is '# H|'
    });

    it('should return true for inline formatting at beginning', () => {
      const { controller } = createController('**bold text**');

      // Test positions within opening syntax
      expect((controller as any).shouldCreateNodeAbove('**bold text**', 0)).toBe(true); // |**
      expect((controller as any).shouldCreateNodeAbove('**bold text**', 1)).toBe(true); // *|*
      expect((controller as any).shouldCreateNodeAbove('**bold text**', 2)).toBe(true); // **|

      // Test position past opening syntax
      expect((controller as any).shouldCreateNodeAbove('**bold text**', 3)).toBe(false); // **b|old
    });

    it('should handle various inline formatting patterns', () => {
      const { controller } = createController();

      // Bold patterns
      expect((controller as any).shouldCreateNodeAbove('__bold__', 1)).toBe(true);
      expect((controller as any).shouldCreateNodeAbove('__bold__', 2)).toBe(true);

      // Italic patterns
      expect((controller as any).shouldCreateNodeAbove('*italic*', 1)).toBe(true);
      expect((controller as any).shouldCreateNodeAbove('_italic_', 1)).toBe(true);

      // Strikethrough
      expect((controller as any).shouldCreateNodeAbove('~~strike~~', 1)).toBe(true);
      expect((controller as any).shouldCreateNodeAbove('~~strike~~', 2)).toBe(true);

      // Code
      expect((controller as any).shouldCreateNodeAbove('`code`', 1)).toBe(true);

      // Edge cases: Should prioritize first pattern that matches
      expect((controller as any).shouldCreateNodeAbove('***bold italic***', 1)).toBe(true); // Matches ** first
      expect((controller as any).shouldCreateNodeAbove('___bold italic___', 1)).toBe(true); // Matches __ first
    });

    it('should return false for non-formatted content', () => {
      const { controller } = createController('Regular text');
      const shouldCreateAbove = (controller as any).shouldCreateNodeAbove('Regular text', 0);
      expect(shouldCreateAbove).toBe(true); // Position 0 is always true

      const shouldCreateAboveMiddle = (controller as any).shouldCreateNodeAbove('Regular text', 5);
      expect(shouldCreateAboveMiddle).toBe(false);
    });
  });
});