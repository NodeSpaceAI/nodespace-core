/**
 * ContentEditableController Unit Tests
 * Tests the controller pattern implementation for dual-representation editor
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ContentEditableController, type ContentEditableEvents } from '$lib/design/components/ContentEditableController.js';

describe('ContentEditableController', () => {
  let element: HTMLDivElement;
  let controller: ContentEditableController;
  let mockEvents: ContentEditableEvents;
  let eventCalls: Record<string, any[]>;

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
      }
    };

    controller = new ContentEditableController(element, 'test-node', mockEvents);
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
      expect(html).toContain('<span class="markdown-underline">underline</span>');
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
      
      // Simulate input
      element.textContent = 'updated test';
      element.dispatchEvent(new Event('input'));
      
      expect(eventCalls.contentChanged).toHaveLength(1);
      expect(eventCalls.contentChanged[0]).toBe('updated test');
    });

    it('should handle Enter key for new node creation', () => {
      controller.initialize('test', true);
      
      // Simulate Enter key
      const enterEvent = new KeyboardEvent('keydown', { key: 'Enter' });
      element.dispatchEvent(enterEvent);
      
      expect(eventCalls.createNewNode).toHaveLength(1);
      expect(eventCalls.createNewNode[0].afterNodeId).toBe('test-node');
    });

    it('should handle Tab key for indentation', () => {
      controller.initialize('test', true);
      
      // Simulate Tab key
      const tabEvent = new KeyboardEvent('keydown', { key: 'Tab' });
      element.dispatchEvent(tabEvent);
      
      expect(eventCalls.indentNode).toHaveLength(1);
      expect(eventCalls.indentNode[0].nodeId).toBe('test-node');
    });

    it('should handle Shift+Tab for outdentation', () => {
      controller.initialize('test', true);
      
      // Simulate Shift+Tab
      const shiftTabEvent = new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true });
      element.dispatchEvent(shiftTabEvent);
      
      expect(eventCalls.outdentNode).toHaveLength(1);
      expect(eventCalls.outdentNode[0].nodeId).toBe('test-node');
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
      
      // Test in editing mode (raw)
      controller.initialize('**old**', true);
      controller.updateContent('**new**');
      expect(element.textContent).toBe('**new**');
    });
  });

  describe('Cleanup', () => {
    it('should properly cleanup event listeners', () => {
      const addEventListenerSpy = vi.spyOn(element, 'addEventListener');
      const removeEventListenerSpy = vi.spyOn(element, 'removeEventListener');
      
      const testController = new ContentEditableController(element, 'test', mockEvents);
      
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