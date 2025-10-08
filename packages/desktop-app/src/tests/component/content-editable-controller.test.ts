/**
 * ContentEditableController Unit Tests
 * Tests the controller pattern implementation for dual-representation editor
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { JSDOM } from 'jsdom';
import {
  ContentEditableController,
  type ContentEditableEvents
} from '$lib/design/components/content-editable-controller.js';

// Setup DOM environment for this test file
const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>');
globalThis.document = dom.window.document;
globalThis.window = dom.window as unknown as Window & typeof globalThis;
globalThis.HTMLElement = dom.window.HTMLElement;
globalThis.Element = dom.window.Element;
globalThis.Node = dom.window.Node;
globalThis.Event = dom.window.Event;
globalThis.FocusEvent = dom.window.FocusEvent;
globalThis.KeyboardEvent = dom.window.KeyboardEvent;
globalThis.InputEvent = dom.window.InputEvent;
globalThis.NodeFilter = dom.window.NodeFilter;

// Type for tracking event calls in tests
interface EventCallRecord {
  contentChanged?: string[];
  headerLevelChanged?: number[];
  focus?: boolean[];
  blur?: boolean[];
  createNewNode?: Array<{
    afterNodeId: string;
    nodeType: string;
    currentContent?: string;
    newContent?: string;
    originalContent?: string;
    cursorAtBeginning?: boolean;
    insertAtBeginning?: boolean;
    focusOriginalNode?: boolean;
  }>;
  indentNode?: Array<{ nodeId: string }>;
  outdentNode?: Array<{ nodeId: string }>;
  navigateArrow?: Array<{ nodeId: string; direction: 'up' | 'down'; pixelOffset: number }>;
  combineWithPrevious?: Array<{ nodeId: string; currentContent: string }>;
  deleteNode?: Array<{ nodeId: string }>;
  triggerDetected?: Array<{
    triggerContext: unknown;
    cursorPosition: { x: number; y: number };
  }>;
  triggerHidden?: boolean[];
  nodeReferenceSelected?: Array<{ nodeId: string; nodeTitle: string }>;
  slashCommandDetected?: Array<unknown>;
  slashCommandHidden?: boolean[];
  slashCommandSelected?: Array<unknown>;
  nodeTypeConversionDetected?: Array<{
    nodeId: string;
    newNodeType: string;
    cleanedContent: string;
  }>;
  directSlashCommand?: Array<{
    command: string;
    nodeType: string;
    cursorPosition?: number;
  }>;
}

describe('ContentEditableController', () => {
  let element: HTMLDivElement;
  let controller: ContentEditableController;
  let mockEvents: ContentEditableEvents;
  let eventCalls: EventCallRecord;

  beforeEach(() => {
    // Create mock element
    element = document.createElement('div');
    element.contentEditable = 'true';
    document.body.appendChild(element);
    eventCalls = {};

    // Mock event handlers
    mockEvents = {
      contentChanged: (content: string) => {
        eventCalls.contentChanged = eventCalls.contentChanged || [];
        eventCalls.contentChanged.push(content);
      },
      headerLevelChanged: (level: number) => {
        eventCalls.headerLevelChanged = eventCalls.headerLevelChanged || [];
        eventCalls.headerLevelChanged.push(level);
      },
      focus: () => {
        eventCalls.focus = eventCalls.focus || [];
        eventCalls.focus.push(true);
      },
      blur: () => {
        eventCalls.blur = eventCalls.blur || [];
        eventCalls.blur.push(true);
      },
      createNewNode: (data) => {
        eventCalls.createNewNode = eventCalls.createNewNode || [];
        eventCalls.createNewNode.push(data);
      },
      indentNode: (data) => {
        eventCalls.indentNode = eventCalls.indentNode || [];
        eventCalls.indentNode.push(data);
      },
      outdentNode: (data) => {
        eventCalls.outdentNode = eventCalls.outdentNode || [];
        eventCalls.outdentNode.push(data);
      },
      navigateArrow: (data) => {
        eventCalls.navigateArrow = eventCalls.navigateArrow || [];
        eventCalls.navigateArrow.push(data);
      },
      combineWithPrevious: (data) => {
        eventCalls.combineWithPrevious = eventCalls.combineWithPrevious || [];
        eventCalls.combineWithPrevious.push(data);
      },
      deleteNode: (data) => {
        eventCalls.deleteNode = eventCalls.deleteNode || [];
        eventCalls.deleteNode.push(data);
      },
      triggerDetected: (data) => {
        eventCalls.triggerDetected = eventCalls.triggerDetected || [];
        eventCalls.triggerDetected.push(data);
      },
      triggerHidden: () => {
        eventCalls.triggerHidden = eventCalls.triggerHidden || [];
        eventCalls.triggerHidden.push(true);
      },
      nodeReferenceSelected: (data) => {
        eventCalls.nodeReferenceSelected = eventCalls.nodeReferenceSelected || [];
        eventCalls.nodeReferenceSelected.push(data);
      },
      slashCommandDetected: (data) => {
        eventCalls.slashCommandDetected = eventCalls.slashCommandDetected || [];
        eventCalls.slashCommandDetected.push(data);
      },
      slashCommandHidden: () => {
        eventCalls.slashCommandHidden = eventCalls.slashCommandHidden || [];
        eventCalls.slashCommandHidden.push(true);
      },
      slashCommandSelected: (data) => {
        eventCalls.slashCommandSelected = eventCalls.slashCommandSelected || [];
        eventCalls.slashCommandSelected.push(data);
      },
      nodeTypeConversionDetected: (data) => {
        eventCalls.nodeTypeConversionDetected = eventCalls.nodeTypeConversionDetected || [];
        eventCalls.nodeTypeConversionDetected.push(data);
      },
      directSlashCommand: (data) => {
        eventCalls.directSlashCommand = eventCalls.directSlashCommand || [];
        eventCalls.directSlashCommand.push(data);
      }
    };

    controller = new ContentEditableController(element, 'test-node', 'text', mockEvents);
  });

  afterEach(() => {
    if (controller) {
      controller.destroy();
    }
    if (element && element.parentNode) {
      element.parentNode.removeChild(element);
    }
  });

  describe('Initialization', () => {
    it('should initialize with plain text content when autoFocus is false', () => {
      controller.initialize('# Test Header', false);

      // Should show formatted content (no # symbols for headers)
      expect(element.textContent).toBe('Test Header');
    });

    it('should initialize with raw markdown when autoFocus is true', () => {
      controller.initialize('# Test Header', true);

      // Should show raw markdown
      expect(element.textContent).toBe('# Test Header');
    });

    it('should initialize with inline formatting when not a header', () => {
      controller.initialize('This is **bold** text', false);

      // Should show HTML formatting
      expect(element.innerHTML).toContain('<span class="markdown-bold">bold</span>');
    });
  });

  describe('Dual-representation switching', () => {
    it('should switch to raw markdown on focus', () => {
      controller.initialize('This is **bold** text', false);

      // Initial state should have formatted HTML
      expect(element.innerHTML).toContain('<span class="markdown-bold">bold</span>');

      // Simulate focus
      element.dispatchEvent(new FocusEvent('focus'));

      // Should switch to raw markdown
      expect(element.textContent).toBe('This is **bold** text');
      expect(eventCalls.focus).toHaveLength(1);
    });

    it('should switch to formatted content on blur', () => {
      controller.initialize('This is **bold** text', true);

      // Initial state should have raw markdown
      expect(element.textContent).toBe('This is **bold** text');

      // Simulate blur
      element.dispatchEvent(new FocusEvent('blur'));

      // Should switch to formatted HTML
      expect(element.innerHTML).toContain('<span class="markdown-bold">bold</span>');
      expect(eventCalls.blur).toHaveLength(1);
    });
  });

  describe('Content conversion', () => {
    it('should correctly convert markdown to HTML', () => {
      controller.initialize('**bold** *italic* __underline__', false);

      const html = element.innerHTML;
      expect(html).toContain('<span class="markdown-bold">bold</span>');
      expect(html).toContain('<span class="markdown-italic">italic</span>');
      expect(html).toContain('<span class="markdown-bold">underline</span>'); // __ produces bold in this implementation
    });

    it('should correctly handle header syntax stripping', () => {
      controller.initialize('### Header Level 3', false);

      // Should show clean text without # symbols
      expect(element.textContent).toBe('Header Level 3');
    });

    it('should maintain markdown content accurately', () => {
      const originalContent = '# Header\n\nThis is **bold** and *italic* text.';
      controller.initialize(originalContent, true);

      const retrievedContent = controller.getMarkdownContent();
      expect(retrievedContent).toBe(originalContent);
    });
  });

  describe('Event handling', () => {
    it('should dispatch content changed events', () => {
      controller.initialize('test', true);

      // Simulate input with proper event target
      element.textContent = 'updated test';
      const inputEvent = new Event('input', { bubbles: true });
      Object.defineProperty(inputEvent, 'target', {
        value: element,
        writable: false
      });
      element.dispatchEvent(inputEvent);

      expect(eventCalls.contentChanged).toHaveLength(1);
      expect(eventCalls.contentChanged?.[0]).toBe('updated test');
    });

    it('should handle Enter key for new node creation', () => {
      controller.initialize('test', true);

      // Simulate Enter key
      const enterEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      element.dispatchEvent(enterEvent);

      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode?.[0]?.afterNodeId).toBe('test-node');
    });

    it('should handle Tab key for indentation', () => {
      controller.initialize('test', true);

      // Simulate Tab key
      const tabEvent = new KeyboardEvent('keydown', { key: 'Tab' });
      element.dispatchEvent(tabEvent);

      expect(eventCalls.indentNode).toHaveLength(1);
      expect(eventCalls.indentNode?.[0]?.nodeId).toBe('test-node');
    });

    it('should handle Shift+Tab for outdentation', () => {
      controller.initialize('test', true);

      // Simulate Shift+Tab
      const shiftTabEvent = new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true });
      element.dispatchEvent(shiftTabEvent);

      expect(eventCalls.outdentNode).toHaveLength(1);
      expect(eventCalls.outdentNode?.[0]?.nodeId).toBe('test-node');
    });
  });

  describe('Content updates', () => {
    it('should update content without triggering events', () => {
      controller.initialize('initial', false);

      // Reset event calls
      eventCalls = {};

      // Update content programmatically
      controller.updateContent('updated content');

      // Should not trigger content changed events
      expect(eventCalls.contentChanged).toBeUndefined();

      // But content should be updated
      expect(element.textContent).toBe('updated content');
    });

    it('should handle external content updates in both modes', () => {
      // Test in display mode (formatted)
      controller.initialize('**old**', false);
      controller.updateContent('**new**');
      expect(element.innerHTML).toContain('<span class="markdown-bold">new</span>');

      // Test in editing mode (live formatting with syntax preserved)
      // Create fresh element and controller for editing mode test
      const editingElement = document.createElement('div');
      editingElement.contentEditable = 'true';
      document.body.appendChild(editingElement);

      const editingController = new ContentEditableController(
        editingElement,
        'editing-test-node',
        'text',
        mockEvents
      );
      editingController.initialize('**old**', true);

      // Verify initial state has live formatting
      expect(editingElement.innerHTML).toContain(
        '<span class="markdown-syntax">**<span class="markdown-bold">old</span>**</span>'
      );

      editingController.updateContent('**new**');
      // In editing mode, should show live formatting while preserving syntax
      expect(editingElement.innerHTML).toContain(
        '<span class="markdown-syntax">**<span class="markdown-bold">new</span>**</span>'
      );

      // Cleanup
      editingController.destroy();
    });
  });

  describe('Formatting operations', () => {
    beforeEach(() => {
      // Create a fresh element for formatting tests
      element.textContent = '';
      controller.initialize('', true);
    });

    it('should correctly apply nested formatting (italic to bold)', () => {
      // Set up the content with bold text
      controller.updateContent('__bold__');
      element.textContent = '__bold__';

      // Create a selection that includes the bold markers
      const range = document.createRange();
      const textNode = element.firstChild as Text;
      range.setStart(textNode, 0);
      range.setEnd(textNode, 8); // selects "__bold__"

      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Cmd+I (italic) keyboard shortcut
      const italicEvent = new KeyboardEvent('keydown', {
        key: 'i',
        metaKey: true,
        bubbles: true
      });
      element.dispatchEvent(italicEvent);

      // Should result in "*__bold__*" (italic around bold)
      expect(element.textContent).toBe('*__bold__*');
    });

    it('should correctly apply nested formatting (bold to italic)', () => {
      // Set up the content with italic text
      controller.updateContent('_italic_');
      element.textContent = '_italic_';

      // Create a selection that includes the italic markers
      const range = document.createRange();
      const textNode = element.firstChild as Text;
      range.setStart(textNode, 0);
      range.setEnd(textNode, 8); // selects "_italic_"

      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Cmd+B (bold) keyboard shortcut
      const boldEvent = new KeyboardEvent('keydown', {
        key: 'b',
        metaKey: true,
        bubbles: true
      });
      element.dispatchEvent(boldEvent);

      // Should result in "**_italic_**" (bold around italic)
      expect(element.textContent).toBe('**_italic_**');
    });

    it('should correctly remove formatting when exact same marker is applied', () => {
      // Set up the content with asterisk bold text (so Cmd+B will match exactly)
      controller.updateContent('**bold**');
      element.textContent = '**bold**';

      // Create a selection that includes the bold markers
      const range = document.createRange();
      const textNode = element.firstChild as Text;
      range.setStart(textNode, 0);
      range.setEnd(textNode, 8); // selects "**bold**"

      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Cmd+B (bold) keyboard shortcut - should toggle off since ** matches **
      const boldEvent = new KeyboardEvent('keydown', {
        key: 'b',
        metaKey: true,
        bubbles: true
      });
      element.dispatchEvent(boldEvent);

      // Should result in "bold" (formatting removed)
      expect(element.textContent).toBe('bold');
    });
  });

  describe('Bug fixes for multi-line behavior', () => {
    beforeEach(() => {
      // Create controller with multiline enabled for these tests
      controller.destroy();
      controller = new ContentEditableController(element, 'test-node', 'text', mockEvents, {
        allowMultiline: true
      });
      controller.initialize('', true);
    });

    it('should not merge nodes when backspacing at start of first line (Bug 1 fix)', () => {
      // Set up multi-line content with DIV structure
      element.innerHTML = '<div>First line</div><div>Second line</div>';

      // Position cursor at start of second div
      const secondDiv = element.children[1] as HTMLElement;
      const range = document.createRange();
      range.setStart(secondDiv, 0);
      range.collapse(true);
      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate backspace at start of second line
      const backspaceEvent = new KeyboardEvent('keydown', { key: 'Backspace' });
      element.dispatchEvent(backspaceEvent);

      // Should not trigger combineWithPrevious since we're at start of first line of this node
      expect(eventCalls.combineWithPrevious).toBeUndefined();
    });

    it('should preserve line breaks through blur/focus cycles using innerText (Bug 2 fix)', () => {
      // Set up content with BR tags (simulating browser behavior)
      element.innerHTML = 'First line<br>Second line';

      // Simulate blur to trigger content conversion
      element.dispatchEvent(new FocusEvent('blur'));

      // Content should preserve line breaks
      const content = controller.getMarkdownContent();
      expect(content).toBe('First line\nSecond line');
    });
  });

  describe('TaskNode multiline prevention (Bug 3 fix)', () => {
    beforeEach(() => {
      // Create controller configured as task node (single-line only)
      controller.destroy();
      controller = new ContentEditableController(element, 'task-node', 'task', mockEvents, {
        allowMultiline: false
      });
      controller.initialize('Task content', true);
    });

    it('should not create new lines on Shift+Enter for task nodes', () => {
      element.textContent = 'Task content';

      // Position cursor in middle
      const textNode = element.firstChild as Text;
      const range = document.createRange();
      range.setStart(textNode, 4);
      range.collapse(true);
      const selection = window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter on task node
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Should not create DIV structure for single-line nodes
      expect(element.children.length).toBe(0);
      expect(element.textContent).toBe('Task content');
    });
  });

  describe('Arrow key navigation with leading line breaks (Bug fixes)', () => {
    let navigateArrowSpy: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      // Destroy previous controller to avoid multiple event listeners
      if (controller) {
        controller.destroy();
      }

      // Reset event calls and create spy
      eventCalls = {};
      navigateArrowSpy = vi.fn((data) => {
        eventCalls.navigateArrow = eventCalls.navigateArrow || [];
        eventCalls.navigateArrow.push(data);
      });

      // Replace the navigateArrow handler with our spy
      mockEvents.navigateArrow = navigateArrowSpy;

      // Create a multiline contenteditable with allowMultiline config
      element.innerHTML = '<div><br></div><div><br></div><div>Text content</div>';
      controller = new ContentEditableController(element, 'test-node', 'text', mockEvents, {
        allowMultiline: true
      });
    });

    it('should stay within node when arrow up from empty leading lines', () => {
      // Set cursor in the second empty line (not the first)
      const selection = window.getSelection()!;
      const range = document.createRange();
      const secondEmptyDiv = element.children[1] as Element;
      range.setStart(secondEmptyDiv, 0);
      range.setEnd(secondEmptyDiv, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate ArrowUp key
      const arrowUpEvent = new KeyboardEvent('keydown', {
        key: 'ArrowUp',
        bubbles: true
      });
      element.dispatchEvent(arrowUpEvent);

      // Should NOT call navigateArrow (stay within node)
      expect(navigateArrowSpy).not.toHaveBeenCalled();
    });

    it('should navigate to previous node only from beginning of first line', () => {
      // Set cursor at the very beginning of the first empty line
      const selection = window.getSelection()!;
      const range = document.createRange();
      const firstEmptyDiv = element.children[0] as Element;
      range.setStart(firstEmptyDiv, 0);
      range.setEnd(firstEmptyDiv, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate ArrowUp key
      const arrowUpEvent = new KeyboardEvent('keydown', {
        key: 'ArrowUp',
        bubbles: true
      });
      element.dispatchEvent(arrowUpEvent);

      // Should call navigateArrow (go to previous node)
      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'up',
        pixelOffset: expect.any(Number)
      });
    });

    it('should navigate to next node from last line (cohesive writing canvas)', () => {
      // Set cursor in the middle of the last line (text content line)
      const selection = window.getSelection()!;
      const range = document.createRange();
      const textDiv = element.children[2] as Element; // Last line
      const textNode = textDiv.firstChild!;
      range.setStart(textNode, 5); // Middle of "Text content"
      range.setEnd(textNode, 5);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate ArrowDown key
      const arrowDownEvent = new KeyboardEvent('keydown', {
        key: 'ArrowDown',
        bubbles: true
      });
      element.dispatchEvent(arrowDownEvent);

      // Should call navigateArrow - navigates from anywhere on last line for seamless experience
      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'down',
        pixelOffset: expect.any(Number)
      });
    });

    it('should stay within node when arrow down from middle line', () => {
      // Set cursor in the second line (middle line, not first or last)
      const selection = window.getSelection()!;
      const range = document.createRange();
      const secondDiv = element.children[1] as Element; // Second line (index 1)
      range.setStart(secondDiv, 0);
      range.setEnd(secondDiv, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate ArrowDown key
      const arrowDownEvent = new KeyboardEvent('keydown', {
        key: 'ArrowDown',
        bubbles: true
      });
      element.dispatchEvent(arrowDownEvent);

      // Should NOT call navigateArrow - only navigates from first/last line
      expect(navigateArrowSpy).not.toHaveBeenCalled();
    });

    it('should handle mixed empty and content lines correctly', () => {
      // Create structure: empty, content, empty
      element.innerHTML = '<div><br></div><div>Middle text</div><div><br></div>';

      // Test arrow up from middle content line (should stay within)
      const selection = window.getSelection()!;
      const range = document.createRange();
      const middleDiv = element.children[1] as Element;
      const textNode = middleDiv.firstChild!;
      range.setStart(textNode, 0);
      range.setEnd(textNode, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      // Clear previous calls
      vi.clearAllMocks();

      // Simulate ArrowUp key
      const arrowUpEvent = new KeyboardEvent('keydown', {
        key: 'ArrowUp',
        bubbles: true
      });
      element.dispatchEvent(arrowUpEvent);

      // Should NOT call navigateArrow (move to first empty line within node)
      expect(navigateArrowSpy).not.toHaveBeenCalled();
    });

    it('should correctly identify line boundaries with complex content', () => {
      // Create structure with formatted content
      element.innerHTML =
        '<div><br></div><div><strong>Bold</strong> and normal</div><div>Last line</div>';

      // Test arrow up from beginning of bold content (should stay within)
      const selection = window.getSelection()!;
      const range = document.createRange();
      const secondDiv = element.children[1] as Element;
      const boldElement = secondDiv.querySelector('strong')!;
      const textNode = boldElement.firstChild!;
      range.setStart(textNode, 0);
      range.setEnd(textNode, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      // Clear previous calls
      vi.clearAllMocks();

      // Simulate ArrowUp key
      const arrowUpEvent = new KeyboardEvent('keydown', {
        key: 'ArrowUp',
        bubbles: true
      });
      element.dispatchEvent(arrowUpEvent);

      // Should NOT call navigateArrow (should navigate to first empty line within node)
      expect(navigateArrowSpy).not.toHaveBeenCalled();
    });
  });

  describe('Shift+Enter Navigation (Regression Tests)', () => {
    let navigateArrowSpy: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      // Destroy previous controller to avoid multiple event listeners
      if (controller) {
        controller.destroy();
      }

      // Reset event calls and create spy
      eventCalls = {};
      navigateArrowSpy = vi.fn((data) => {
        eventCalls.navigateArrow = eventCalls.navigateArrow || [];
        eventCalls.navigateArrow.push(data);
      });

      // Replace the navigateArrow handler with our spy
      mockEvents.navigateArrow = navigateArrowSpy;

      // Create a multiline contenteditable with allowMultiline config
      controller = new ContentEditableController(element, 'test-node', 'text', mockEvents, {
        allowMultiline: true
      });
    });

    it('should navigate within multiline content after Shift+Enter, not jump to next node', () => {
      // Simulate "Some text" with Shift+Enter creating "Some \ntext" (text before DIV + DIV)
      // This is the browser structure immediately after Shift+Enter
      const textNode = document.createTextNode('Some ');
      const divElement = document.createElement('div');
      divElement.textContent = 'text';
      element.appendChild(textNode);
      element.appendChild(divElement);

      // Position cursor at beginning of text before DIV (line 0): "|Some "
      const selection = window.getSelection()!;
      const range = document.createRange();
      range.setStart(textNode, 0);
      range.setEnd(textNode, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      vi.clearAllMocks();

      // Arrow down should move to the DIV (line 1), NOT navigate to next node
      const arrowDownEvent = new KeyboardEvent('keydown', {
        key: 'ArrowDown',
        bubbles: true
      });
      element.dispatchEvent(arrowDownEvent);

      // Should NOT call navigateArrow because we're on line 0 (not last line)
      expect(navigateArrowSpy).not.toHaveBeenCalled();
    });

    it('should navigate to next node when on last line of multiline content', () => {
      // Simulate "Some text" with Shift+Enter creating "Some \ntext"
      const textNode = document.createTextNode('Some ');
      const divElement = document.createElement('div');
      divElement.textContent = 'text';
      element.appendChild(textNode);
      element.appendChild(divElement);

      // Position cursor in the DIV (line 1 - last line): "Some \n|text"
      const selection = window.getSelection()!;
      const range = document.createRange();
      const divTextNode = divElement.firstChild!;
      range.setStart(divTextNode, 0);
      range.setEnd(divTextNode, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      vi.clearAllMocks();

      // Arrow down from last line should navigate to next node
      const arrowDownEvent = new KeyboardEvent('keydown', {
        key: 'ArrowDown',
        bubbles: true
      });
      element.dispatchEvent(arrowDownEvent);

      // Should call navigateArrow because we're on the last line
      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'down',
        pixelOffset: 0
      });
    });

    it('should navigate to previous node when on first line of multiline content', () => {
      // Simulate "Some text" with Shift+Enter creating "Some \ntext"
      const textNode = document.createTextNode('Some ');
      const divElement = document.createElement('div');
      divElement.textContent = 'text';
      element.appendChild(textNode);
      element.appendChild(divElement);

      // Position cursor at beginning of text before DIV (line 0): "|Some "
      const selection = window.getSelection()!;
      const range = document.createRange();
      range.setStart(textNode, 0);
      range.setEnd(textNode, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      vi.clearAllMocks();

      // Arrow up from first line should navigate to previous node
      const arrowUpEvent = new KeyboardEvent('keydown', {
        key: 'ArrowUp',
        bubbles: true
      });
      element.dispatchEvent(arrowUpEvent);

      // Should call navigateArrow because we're on the first line
      expect(navigateArrowSpy).toHaveBeenCalledWith({
        nodeId: 'test-node',
        direction: 'up',
        pixelOffset: 0
      });
    });

    it('should not navigate to previous node when on second line (DIV) of multiline content', () => {
      // Simulate "Some text" with Shift+Enter creating "Some \ntext"
      const textNode = document.createTextNode('Some ');
      const divElement = document.createElement('div');
      divElement.textContent = 'text';
      element.appendChild(textNode);
      element.appendChild(divElement);

      // Position cursor in the DIV (line 1): "Some \n|text"
      const selection = window.getSelection()!;
      const range = document.createRange();
      const divTextNode = divElement.firstChild!;
      range.setStart(divTextNode, 0);
      range.setEnd(divTextNode, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      vi.clearAllMocks();

      // Arrow up should move within node (to line 0), NOT navigate to previous node
      const arrowUpEvent = new KeyboardEvent('keydown', {
        key: 'ArrowUp',
        bubbles: true
      });
      element.dispatchEvent(arrowUpEvent);

      // Should NOT call navigateArrow because we're not on first line
      expect(navigateArrowSpy).not.toHaveBeenCalled();
    });
  });

  describe('Shift+Enter with inline formatting (Integration Tests)', () => {
    beforeEach(() => {
      // Destroy previous controller
      if (controller) {
        controller.destroy();
      }

      // Reset event calls
      eventCalls = {};

      // Create a multiline contenteditable for these tests
      controller = new ContentEditableController(element, 'test-node', 'text', mockEvents, {
        allowMultiline: true
      });
    });

    it('should split bold text and preserve formatting on both lines', () => {
      // Initialize with bold text
      controller.initialize('**bold text**', true);

      // Position cursor after "bol" - **bol|d text**
      const selection = window.getSelection()!;
      const range = document.createRange();

      // Find the text node containing "bold text"
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let boldTextNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        if (currentNode.textContent?.includes('bol')) {
          boldTextNode = currentNode;
          break;
        }
      }

      expect(boldTextNode).not.toBeNull();

      // Position after "bol" (3 characters into the text node)
      range.setStart(boldTextNode!, 3);
      range.setEnd(boldTextNode!, 3);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify DOM structure has two DIVs
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);

      // Verify content of both lines
      expect(divs[0].textContent).toBe('**bol**');
      expect(divs[1].textContent).toBe('**d text**');

      // Verify contentChanged event was fired
      expect(eventCalls.contentChanged).toHaveLength(1);
      expect(eventCalls.contentChanged?.[0]).toBe('**bol**\n**d text**');
    });

    it('should split italic text and preserve formatting', () => {
      controller.initialize('*italic text*', true);

      // Position cursor after "ital" - *ital|ic text*
      const selection = window.getSelection()!;
      const range = document.createRange();

      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let textNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        if (currentNode.textContent?.includes('ital')) {
          textNode = currentNode;
          break;
        }
      }

      range.setStart(textNode!, 4);
      range.setEnd(textNode!, 4);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify two lines with preserved formatting
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('*ital*');
      expect(divs[1].textContent).toBe('*ic text*');
    });

    it('should split code text and preserve formatting', () => {
      controller.initialize('`code snippet`', true);

      // Position cursor after "code" - `code| snippet`
      const selection = window.getSelection()!;
      const range = document.createRange();

      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let textNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        if (currentNode.textContent?.includes('code')) {
          textNode = currentNode;
          break;
        }
      }

      range.setStart(textNode!, 4);
      range.setEnd(textNode!, 4);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify preserved code formatting
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('`code`');
      expect(divs[1].textContent).toBe('` snippet`');
    });

    it('should split strikethrough text and preserve formatting', () => {
      controller.initialize('~~crossed out~~', true);

      // Position cursor after "crossed" - ~~crossed| out~~
      const selection = window.getSelection()!;
      const range = document.createRange();

      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let textNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        if (currentNode.textContent?.includes('crossed')) {
          textNode = currentNode;
          break;
        }
      }

      range.setStart(textNode!, 7);
      range.setEnd(textNode!, 7);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify preserved strikethrough formatting
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('~~crossed~~');
      expect(divs[1].textContent).toBe('~~ out~~');
    });

    it('should split mixed plain and formatted text correctly', () => {
      controller.initialize('This is **bold** text', true);

      // Position cursor in the middle of "bold" - This is **bo|ld** text
      const selection = window.getSelection()!;
      const range = document.createRange();

      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let textNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        if (currentNode.textContent?.includes('bo')) {
          textNode = currentNode;
          break;
        }
      }

      range.setStart(textNode!, 2);
      range.setEnd(textNode!, 2);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify split preserves bold formatting
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('This is **bo**');
      expect(divs[1].textContent).toBe('**ld** text');
    });

    it('should handle Shift+Enter at the beginning of formatted text', () => {
      controller.initialize('**bold**', true);

      // Position cursor at beginning - |**bold**
      const selection = window.getSelection()!;
      const range = document.createRange();
      range.setStart(element, 0);
      range.setEnd(element, 0);
      selection.removeAllRanges();
      selection.addRange(range);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify empty first line and preserved formatting on second line
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('');
      expect(divs[1].textContent).toBe('**bold**');
    });

    it('should handle Shift+Enter at the end of formatted text', () => {
      controller.initialize('**bold**', true);

      // Position cursor at end - **bold**|
      const selection = window.getSelection()!;
      const range = document.createRange();

      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let lastNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        lastNode = currentNode;
      }

      if (lastNode) {
        range.setStart(lastNode, lastNode.textContent?.length || 0);
        range.setEnd(lastNode, lastNode.textContent?.length || 0);
        selection.removeAllRanges();
        selection.addRange(range);
      }

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Verify preserved formatting on first line and empty second line
      const divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('**bold**');
      expect(divs[1].textContent).toBe('');
    });

    it('should handle multiple Shift+Enter presses', () => {
      controller.initialize('**text**', true);

      // First Shift+Enter in the middle
      const selection = window.getSelection()!;
      const range = document.createRange();

      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT, null);
      let textNode: Node | null = null;
      let currentNode;
      while ((currentNode = walker.nextNode())) {
        if (currentNode.textContent?.includes('te')) {
          textNode = currentNode;
          break;
        }
      }

      range.setStart(textNode!, 2);
      range.setEnd(textNode!, 2);
      selection.removeAllRanges();
      selection.addRange(range);

      const shiftEnterEvent1 = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent1);

      // Verify first split
      let divs = element.querySelectorAll('div');
      expect(divs.length).toBe(2);
      expect(divs[0].textContent).toBe('**te**');
      expect(divs[1].textContent).toBe('**xt**');

      // Second Shift+Enter in the second line
      const walker2 = document.createTreeWalker(divs[1], NodeFilter.SHOW_TEXT, null);
      let textNode2: Node | null = null;
      let currentNode2;
      while ((currentNode2 = walker2.nextNode())) {
        if (currentNode2.textContent?.includes('xt')) {
          textNode2 = currentNode2;
          break;
        }
      }

      range.setStart(textNode2!, 1);
      range.setEnd(textNode2!, 1);
      selection.removeAllRanges();
      selection.addRange(range);

      const shiftEnterEvent2 = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent2);

      // Verify second split resulted in three lines
      divs = element.querySelectorAll('div');
      expect(divs.length).toBe(3);
    });
  });

  describe('Cleanup', () => {
    it('should properly cleanup event listeners', () => {
      const addEventListenerSpy = vi.spyOn(element, 'addEventListener');
      const removeEventListenerSpy = vi.spyOn(element, 'removeEventListener');

      const testController = new ContentEditableController(element, 'test', 'text', mockEvents);

      // Should have added event listeners
      expect(addEventListenerSpy).toHaveBeenCalledWith('focus', expect.any(Function));
      expect(addEventListenerSpy).toHaveBeenCalledWith('blur', expect.any(Function));
      expect(addEventListenerSpy).toHaveBeenCalledWith('input', expect.any(Function));
      expect(addEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));

      testController.destroy();

      // Should have removed event listeners
      expect(removeEventListenerSpy).toHaveBeenCalledWith('focus', expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith('blur', expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith('input', expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });
  });
});
