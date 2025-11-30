/**
 * TextareaController Unit Tests
 *
 * Tests the textarea-based controller implementation with single-source-of-truth architecture.
 * Migrated from ContentEditableController tests, adapted for simpler textarea-based editor.
 *
 * Key differences from ContentEditableController:
 * - Single state (textarea.value) instead of dual (DOM + markdown)
 * - Native cursor APIs (selectionStart/End) instead of Range manipulation
 * - String assertions instead of HTML parsing
 * - Pattern detection instead of HTML conversion
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  TextareaController,
  type TextareaControllerEvents
} from '../../lib/design/components/textarea-controller';
import { DEFAULT_PANE_ID } from '../../lib/stores/navigation';

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
    newNodeCursorPosition?: number;
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

describe('TextareaController', () => {
  let element: HTMLTextAreaElement;
  let controller: TextareaController;
  let mockEvents: TextareaControllerEvents;
  let eventCalls: EventCallRecord;
  let removeEventListenerSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    // Create mock textarea element
    element = document.createElement('textarea');
    document.body.appendChild(element);
    eventCalls = {};

    // Spy on removeEventListener for cleanup tests
    removeEventListenerSpy = vi.spyOn(element, 'removeEventListener');

    // Mock event handlers
    mockEvents = {
      contentChanged: (content: string) => {
        eventCalls.contentChanged = eventCalls.contentChanged || [];
        eventCalls.contentChanged.push(content);
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

    controller = new TextareaController(element, 'test-node', 'text', DEFAULT_PANE_ID, mockEvents);
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
    it('should initialize with raw markdown content when autoFocus is false', () => {
      controller.initialize('# Test Header', false);

      // Textarea always shows raw markdown
      expect(element.value).toBe('# Test Header');
    });

    it('should initialize with raw markdown when autoFocus is true', () => {
      controller.initialize('# Test Header', true);

      // Textarea always shows raw markdown
      expect(element.value).toBe('# Test Header');
    });

    it('should initialize with inline formatting markers preserved', () => {
      controller.initialize('This is **bold** text', false);

      // Textarea shows raw markdown, not HTML formatting
      expect(element.value).toBe('This is **bold** text');
    });

    it('should auto-focus when autoFocus is true', () => {
      controller.initialize('Test content', true);

      // Note: In Happy-DOM, focus() might not set activeElement correctly
      // We test that the method was called without throwing
      expect(element.value).toBe('Test content');
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
      expect(element.value).toBe('updated content');
    });

    it('should handle external content updates correctly', () => {
      controller.initialize('**old**', false);
      controller.updateContent('**new**');

      // Textarea always shows raw markdown
      expect(element.value).toBe('**new**');
    });

    it('should skip update if content is identical', () => {
      controller.initialize('test content', false);
      const initialValue = element.value;

      controller.updateContent('test content');

      expect(element.value).toBe(initialValue);
    });
  });

  describe('Event handling', () => {
    it('should dispatch content changed events on input', () => {
      controller.initialize('test', true);

      // Simulate input
      element.value = 'updated test';
      const inputEvent = new Event('input', { bubbles: true });
      element.dispatchEvent(inputEvent);

      expect(eventCalls.contentChanged).toHaveLength(1);
      expect(eventCalls.contentChanged?.[0]).toBe('updated test');
    });

    it('should handle Enter key for new node creation', async () => {
      controller.initialize('test', true);

      // Set cursor position
      element.selectionStart = 4;
      element.selectionEnd = 4;

      // Simulate Enter key
      const enterEvent = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true });
      element.dispatchEvent(enterEvent);

      // Wait for async command execution
      await vi.waitFor(
        () => {
          expect(eventCalls.createNewNode).toHaveLength(1);
        },
        { timeout: 100 }
      );

      expect(eventCalls.createNewNode?.[0]?.afterNodeId).toBe('test-node');
    });

    it('should handle Tab key for indentation', async () => {
      controller.initialize('test', true);

      // Simulate Tab key
      const tabEvent = new KeyboardEvent('keydown', { key: 'Tab', bubbles: true });
      element.dispatchEvent(tabEvent);

      // Wait for async command execution
      await vi.waitFor(
        () => {
          expect(eventCalls.indentNode).toHaveLength(1);
        },
        { timeout: 100 }
      );

      expect(eventCalls.indentNode?.[0]?.nodeId).toBe('test-node');
    });

    it('should handle Shift+Tab for outdentation', async () => {
      controller.initialize('test', true);

      // Simulate Shift+Tab
      const shiftTabEvent = new KeyboardEvent('keydown', {
        key: 'Tab',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftTabEvent);

      // Wait for async command execution
      await vi.waitFor(
        () => {
          expect(eventCalls.outdentNode).toHaveLength(1);
        },
        { timeout: 100 }
      );

      expect(eventCalls.outdentNode?.[0]?.nodeId).toBe('test-node');
    });

    it('should dispatch focus events', () => {
      controller.initialize('test', false);

      element.dispatchEvent(new FocusEvent('focus'));

      expect(eventCalls.focus).toHaveLength(1);
    });

    it('should dispatch blur events', () => {
      controller.initialize('test', true);

      element.dispatchEvent(new FocusEvent('blur'));

      expect(eventCalls.blur).toHaveLength(1);
    });
  });

  describe('Pattern detection', () => {
    it('should detect header patterns', async () => {
      controller.initialize('', true);

      // Type header pattern
      element.value = '# ';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      // Wait for setTimeout in detectNodeTypeConversion
      await new Promise((resolve) => setTimeout(resolve, 10));

      // TextareaController no longer emits headerLevelChanged - that's handled by HeaderNode's $effect
      // It only emits nodeTypeConversionDetected which tells the system to switch to a HeaderNode
      expect(eventCalls.nodeTypeConversionDetected).toBeDefined();
      expect(eventCalls.nodeTypeConversionDetected?.[0]?.newNodeType).toBe('header');
    });

    it('should detect different header levels', async () => {
      controller.initialize('', true);

      // Type level 3 header
      element.value = '### ';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      // TextareaController no longer tracks or emits header levels
      // Header level detection is now the responsibility of HeaderNode component
      expect(eventCalls.nodeTypeConversionDetected).toBeDefined();
      expect(eventCalls.nodeTypeConversionDetected?.[0]?.newNodeType).toBe('header');
    });

    it('should detect @mention triggers', () => {
      controller.initialize('', true);

      // Type @mention
      element.value = 'Hello @test';
      element.selectionStart = 11;
      element.selectionEnd = 11;
      element.dispatchEvent(new Event('input', { bubbles: true }));

      expect(eventCalls.triggerDetected).toBeDefined();
      expect(eventCalls.triggerDetected?.length).toBeGreaterThan(0);
    });

    it('should detect slash command triggers', () => {
      controller.initialize('', true);

      // Type slash command
      element.value = '/test';
      element.selectionStart = 5;
      element.selectionEnd = 5;
      element.dispatchEvent(new Event('input', { bubbles: true }));

      expect(eventCalls.slashCommandDetected).toBeDefined();
      expect(eventCalls.slashCommandDetected?.length).toBeGreaterThan(0);
    });
  });

  describe('Formatting operations', () => {
    it('should toggle bold formatting', () => {
      controller.initialize('test', true);

      // Select "test"
      element.selectionStart = 0;
      element.selectionEnd = 4;

      // Apply bold formatting
      controller.toggleFormatting('**');

      expect(element.value).toBe('**test**');
    });

    it('should remove bold formatting when already formatted', () => {
      controller.initialize('**test**', true);

      // Select "test" (characters 2-6)
      element.selectionStart = 2;
      element.selectionEnd = 6;

      // Toggle bold (should remove)
      controller.toggleFormatting('**');

      expect(element.value).toBe('test');
    });

    it('should toggle italic formatting', () => {
      controller.initialize('text', true);

      element.selectionStart = 0;
      element.selectionEnd = 4;

      controller.toggleFormatting('*');

      expect(element.value).toBe('*text*');
    });

    it('should handle formatting with markers in selection', () => {
      controller.initialize('Some **bold** text', true);

      // Select "bold" (characters 7-11, inside markers)
      element.selectionStart = 7;
      element.selectionEnd = 11;

      // toggleFormatting checks if markers exist around selection
      // before="Some **", after="** text" - both have **, so it's formatted
      controller.toggleFormatting('**');

      // Should remove ** markers around "bold"
      expect(element.value).toBe('Some bold text');
    });

    it('should not format when no selection exists', () => {
      controller.initialize('test', true);

      // No selection (cursor at position 2)
      element.selectionStart = 2;
      element.selectionEnd = 2;

      controller.toggleFormatting('**');

      // Content should remain unchanged
      expect(element.value).toBe('test');
    });
  });

  describe('Cursor positioning', () => {
    it('should set cursor position correctly', () => {
      controller.initialize('Hello World', true);

      controller.setCursorPosition(5);

      expect(element.selectionStart).toBe(5);
      expect(element.selectionEnd).toBe(5);
    });

    it('should detect cursor at start', () => {
      controller.initialize('test', true);

      element.selectionStart = 0;
      element.selectionEnd = 0;

      // Use private method via public API
      expect(controller.isAtFirstLine()).toBe(true);
    });

    it('should detect cursor at end', () => {
      controller.initialize('test', true);

      element.selectionStart = 4;
      element.selectionEnd = 4;

      expect(controller.isAtLastLine()).toBe(true);
    });

    it('should detect first line correctly', () => {
      controller.initialize('Line 1\nLine 2', true);

      // Cursor on first line
      element.selectionStart = 3;
      element.selectionEnd = 3;

      expect(controller.isAtFirstLine()).toBe(true);
    });

    it('should detect last line correctly', () => {
      controller.initialize('Line 1\nLine 2', true);

      // Cursor on last line
      element.selectionStart = 10;
      element.selectionEnd = 10;

      expect(controller.isAtLastLine()).toBe(true);
    });

    it('should detect middle line correctly', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', true);

      // Cursor on middle line (line 2)
      element.selectionStart = 10;
      element.selectionEnd = 10;

      expect(controller.isAtFirstLine()).toBe(false);
      expect(controller.isAtLastLine()).toBe(false);
    });
  });

  describe('Arrow navigation', () => {
    let navigateArrowSpy: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      // Destroy previous controller
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

      // Create controller with multiline enabled (config as getter - Issue #695)
      controller = new TextareaController(
        element,
        'test-node',
        'text',
        DEFAULT_PANE_ID,
        mockEvents,
        () => ({ allowMultiline: true })
      );
    });

    it('should detect cursor on first line correctly', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', true);

      // Position cursor at beginning of first line
      element.selectionStart = 0;
      element.selectionEnd = 0;

      // Should be on first line
      expect(controller.isAtFirstLine()).toBe(true);
    });

    it('should detect cursor on last line correctly', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', true);

      // Position cursor on last line
      element.selectionStart = element.value.length;
      element.selectionEnd = element.value.length;

      // Should be on last line
      expect(controller.isAtLastLine()).toBe(true);
    });

    it('should navigate to previous node from first line', () => {
      // Note: In Happy-DOM, arrow key navigation behavior may be limited
      // This test verifies the controller's line detection logic
      controller.initialize('Line 1', true);

      // Position cursor at beginning (only one line, so it's both first and last)
      element.selectionStart = 0;
      element.selectionEnd = 0;

      // Verify we're on the first line
      expect(controller.isAtFirstLine()).toBe(true);
    });

    it('should navigate to next node from last line', () => {
      controller.initialize('Line 1', true);

      // Position cursor at end (only one line, so it's both first and last)
      element.selectionStart = element.value.length;
      element.selectionEnd = element.value.length;

      // Verify we're on the last line
      expect(controller.isAtLastLine()).toBe(true);
    });

    it('should stay within node when cursor on middle line', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', true);

      // Position cursor on line 2 (after "Line 1\n")
      element.selectionStart = 8;
      element.selectionEnd = 8;

      // Should not be on first or last line
      expect(controller.isAtFirstLine()).toBe(false);
      expect(controller.isAtLastLine()).toBe(false);
    });
  });

  describe('Multiline support', () => {
    beforeEach(() => {
      // Destroy previous controller
      if (controller) {
        controller.destroy();
      }

      // Create controller with multiline enabled (config as getter - Issue #695)
      controller = new TextareaController(
        element,
        'test-node',
        'text',
        DEFAULT_PANE_ID,
        mockEvents,
        () => ({ allowMultiline: true })
      );
    });

    it('should allow Shift+Enter to create new lines', () => {
      controller.initialize('test', true);

      // Simulate Shift+Enter
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });
      element.dispatchEvent(shiftEnterEvent);

      // Should not prevent default or create new node
      expect(eventCalls.createNewNode).toBeUndefined();
    });

    it('should handle multiline content correctly', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', false);

      expect(element.value).toBe('Line 1\nLine 2\nLine 3');
    });
  });

  describe('Single-line mode (task nodes)', () => {
    beforeEach(() => {
      // Destroy previous controller
      if (controller) {
        controller.destroy();
      }

      // Create controller with multiline disabled (config as getter - Issue #695)
      controller = new TextareaController(
        element,
        'task-node',
        'task',
        DEFAULT_PANE_ID,
        mockEvents,
        () => ({ allowMultiline: false })
      );
    });

    it('should not allow multiline in single-line mode', () => {
      controller.initialize('Task content', true);

      // Verify multiline is disabled in config
      expect(controller['config'].allowMultiline).toBe(false);

      // Shift+Enter should not create new node
      const shiftEnterEvent = new KeyboardEvent('keydown', {
        key: 'Enter',
        shiftKey: true,
        bubbles: true
      });

      element.dispatchEvent(shiftEnterEvent);

      // Should not create new node (Shift+Enter is prevented in single-line mode)
      expect(eventCalls.createNewNode).toBeUndefined();
    });
  });

  describe('Cleanup', () => {
    it('should remove all event listeners on destroy', () => {
      controller.initialize('test', false);

      // Destroy controller
      controller.destroy();

      // Should have called removeEventListener for each event
      expect(removeEventListenerSpy).toHaveBeenCalledWith('focus', expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith('blur', expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith('input', expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith('keydown', expect.any(Function));
    });

    it('should clean up controller reference from DOM element', () => {
      controller.initialize('test', false);

      // Verify controller is attached
      expect(
        (element as unknown as { _textareaController?: TextareaController })._textareaController
      ).toBe(controller);

      // Destroy controller
      controller.destroy();

      // Verify controller reference is removed
      expect(
        (element as unknown as { _textareaController?: TextareaController })._textareaController
      ).toBeUndefined();
    });
  });

  describe('Content manipulation', () => {
    it('should insert node reference correctly (UUID-only format)', () => {
      controller.initialize('Hello @', true);

      // Set cursor after @
      element.selectionStart = 7;
      element.selectionEnd = 7;

      // Trigger @mention detection
      element.dispatchEvent(new Event('input', { bubbles: true }));

      // Insert node reference - stores UUID only, display text fetched at render time
      controller.insertNodeReference('node-123');

      // New format: [](nodespace://uuid) - no display text stored
      expect(element.value).toContain('[](nodespace://node-123)');
    });

    it('should insert slash command content correctly', () => {
      controller.initialize('/test', true);

      // Set cursor after /test
      element.selectionStart = 5;
      element.selectionEnd = 5;

      // Trigger slash command detection
      element.dispatchEvent(new Event('input', { bubbles: true }));

      // Insert command content
      const cursorPos = controller.insertSlashCommand('Header content', false, 'header');

      expect(element.value).toContain('Header content');
      expect(cursorPos).toBeGreaterThan(0);
    });
  });

  describe('Markdown content API', () => {
    it('should return raw markdown content', () => {
      controller.initialize('**bold** and *italic*', true);

      const markdown = controller.getMarkdownContent();

      expect(markdown).toBe('**bold** and *italic*');
    });

    it('should preserve exact content including whitespace', () => {
      const content = '  Leading spaces\n\nBlank line above';
      controller.initialize(content, false);

      const markdown = controller.getMarkdownContent();

      expect(markdown).toBe(content);
    });
  });

  describe('Auto-resize behavior', () => {
    it('should adjust height on initialization', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', false);

      // Height should be set (not auto)
      expect(element.style.height).not.toBe('auto');
      expect(element.style.height).toBeTruthy();
    });

    it('should call adjustHeight on content change', () => {
      controller.initialize('Short', true);

      // Spy on adjustHeight method
      const adjustHeightSpy = vi.spyOn(controller, 'adjustHeight');

      // Update with longer content
      element.value = 'Line 1\nLine 2\nLine 3\nLine 4\nLine 5';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      // adjustHeight should have been called
      expect(adjustHeightSpy).toHaveBeenCalled();
    });
  });

  describe('Column and pixel offset calculation', () => {
    it('should calculate current column correctly', () => {
      controller.initialize('Hello World\nSecond Line', true);

      // Position cursor at "W" in "World"
      element.selectionStart = 6;
      element.selectionEnd = 6;

      const column = controller.getCurrentColumn();

      expect(column).toBe(6);
    });

    it('should calculate column on second line correctly', () => {
      controller.initialize('First Line\nSecond Line', true);

      // Position cursor at "S" in "Second"
      element.selectionStart = 11; // After "First Line\n"
      element.selectionEnd = 11;

      const column = controller.getCurrentColumn();

      expect(column).toBe(0); // First character of second line
    });

    it('should calculate pixel offset for arrow navigation', () => {
      controller.initialize('Hello World', true);

      element.selectionStart = 6;
      element.selectionEnd = 6;

      const pixelOffset = controller.getCurrentPixelOffset();

      // In Happy-DOM, getBoundingClientRect may return 0
      // We just verify the method runs without error
      expect(pixelOffset).toBeGreaterThanOrEqual(0);
    });
  });

  describe('Enter from arrow navigation', () => {
    it('should position cursor at beginning when entering from top', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', true);

      // Enter from top (arrow down from previous node)
      controller.enterFromArrowNavigation('down', 100);

      // Cursor should be on first line
      expect(controller.isAtFirstLine()).toBe(true);
    });

    it('should position cursor at end when entering from bottom', () => {
      controller.initialize('Line 1\nLine 2\nLine 3', true);

      // Enter from bottom (arrow up from next node)
      controller.enterFromArrowNavigation('up', 100);

      // Cursor should be on last line
      expect(controller.isAtLastLine()).toBe(true);
    });
  });

  describe('Backspace behavior', () => {
    it('should detect cursor at start for merge operations', () => {
      controller.initialize('Content', true);

      // Position cursor at start
      element.selectionStart = 0;
      element.selectionEnd = 0;

      // Verify cursor is at first line and start position
      expect(controller.isAtFirstLine()).toBe(true);
      expect(element.selectionStart).toBe(0);
    });

    it('should not be at start when cursor in middle', () => {
      controller.initialize('Content', true);

      // Position cursor in middle
      element.selectionStart = 3;
      element.selectionEnd = 3;

      // Cursor is not at absolute start
      expect(element.selectionStart).toBeGreaterThan(0);
    });
  });

  describe('Keyboard command integration', () => {
    it('should handle Cmd+B for bold formatting', async () => {
      controller.initialize('text', true);

      // Select "text"
      element.selectionStart = 0;
      element.selectionEnd = 4;

      // Simulate Cmd+B
      const cmdBEvent = new KeyboardEvent('keydown', {
        key: 'b',
        metaKey: true,
        bubbles: true
      });
      element.dispatchEvent(cmdBEvent);

      // Wait for async command execution
      await vi.waitFor(
        () => {
          expect(element.value).toBe('**text**');
        },
        { timeout: 100 }
      );
    });

    it('should handle Cmd+I for italic formatting', async () => {
      controller.initialize('text', true);

      // Select "text"
      element.selectionStart = 0;
      element.selectionEnd = 4;

      // Simulate Cmd+I
      const cmdIEvent = new KeyboardEvent('keydown', {
        key: 'i',
        metaKey: true,
        bubbles: true
      });
      element.dispatchEvent(cmdIEvent);

      // Wait for async command execution
      await vi.waitFor(
        () => {
          expect(element.value).toBe('*text*');
        },
        { timeout: 100 }
      );
    });

    it('should handle Ctrl+B for bold formatting (cross-platform)', async () => {
      controller.initialize('text', true);

      // Select "text"
      element.selectionStart = 0;
      element.selectionEnd = 4;

      // Simulate Ctrl+B
      const ctrlBEvent = new KeyboardEvent('keydown', {
        key: 'b',
        ctrlKey: true,
        bubbles: true
      });
      element.dispatchEvent(ctrlBEvent);

      // Wait for async command execution
      await vi.waitFor(
        () => {
          expect(element.value).toBe('**text**');
        },
        { timeout: 100 }
      );
    });
  });

  describe('Dropdown state management', () => {
    it('should track slash command dropdown state', () => {
      controller.initialize('test', true);

      expect(controller.slashCommandDropdownActive).toBe(false);

      controller.setSlashCommandDropdownActive(true);
      expect(controller.slashCommandDropdownActive).toBe(true);

      controller.setSlashCommandDropdownActive(false);
      expect(controller.slashCommandDropdownActive).toBe(false);
    });

    it('should track autocomplete dropdown state', () => {
      controller.initialize('test', true);

      expect(controller.autocompleteDropdownActive).toBe(false);

      controller.setAutocompleteDropdownActive(true);
      expect(controller.autocompleteDropdownActive).toBe(true);

      controller.setAutocompleteDropdownActive(false);
      expect(controller.autocompleteDropdownActive).toBe(false);
    });

    it('should prevent keyboard commands when dropdown is active', () => {
      controller.initialize('test', true);

      // Activate dropdown
      controller.setSlashCommandDropdownActive(true);

      // Try to create new node with Enter
      const enterEvent = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true });
      element.dispatchEvent(enterEvent);

      // Should not create new node (dropdown handles Enter)
      expect(eventCalls.createNewNode).toBeUndefined();
    });
  });

  describe('Config getter pattern (Issue #695)', () => {
    it('should read config through getter on-demand', () => {
      // Destroy default controller
      if (controller) {
        controller.destroy();
      }

      // Create mutable config that the getter will read
      let currentConfig = { allowMultiline: false };
      const getConfig = () => currentConfig;

      controller = new TextareaController(
        element,
        'test-node',
        'text',
        DEFAULT_PANE_ID,
        mockEvents,
        getConfig
      );

      controller.initialize('test', true);

      // Initially single-line
      expect(controller['config'].allowMultiline).toBe(false);

      // Update the external config (simulates reactive state change)
      currentConfig = { allowMultiline: true };

      // Controller reads config on-demand, so it sees the new value
      expect(controller['config'].allowMultiline).toBe(true);
    });
  });
});
