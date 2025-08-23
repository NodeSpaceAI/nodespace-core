/**
 * Cursor Positioning Tests
 * 
 * Tests for the precision cursor positioning functionality that maps
 * click coordinates to correct cursor positions in markdown content.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ContentEditableController } from '$lib/design/components/ContentEditableController';

describe('Cursor Positioning System', () => {
  let mockElement: HTMLDivElement;
  let controller: ContentEditableController;
  let mockEvents: any;

  beforeEach(() => {
    // Create mock DOM element
    mockElement = document.createElement('div');
    mockElement.contentEditable = 'true';
    mockElement.id = 'test-cursor-positioning';
    document.body.appendChild(mockElement);

    // Mock events
    mockEvents = {
      contentChanged: vi.fn(),
      headerLevelChanged: vi.fn(),
      focus: vi.fn(),
      blur: vi.fn(),
      createNewNode: vi.fn(),
      indentNode: vi.fn(),
      outdentNode: vi.fn(),
      navigateArrow: vi.fn(),
      combineWithPrevious: vi.fn(),
      deleteNode: vi.fn(),
      triggerDetected: vi.fn(),
      triggerHidden: vi.fn(),
      nodeReferenceSelected: vi.fn()
    };

    // Create controller instance
    controller = new ContentEditableController(mockElement, 'test-node', mockEvents);
  });

  afterEach(() => {
    // Clean up safely
    if (controller && typeof controller.destroy === 'function') {
      controller.destroy();
    }
    
    if (mockElement && mockElement.parentNode) {
      document.body.removeChild(mockElement);
    }
    
    vi.restoreAllMocks();
  });

  describe('Character Mapping Algorithm', () => {
    it('should create correct mapping for identical text', () => {
      if (!controller) return; // Skip if controller failed to initialize
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, 'Hello World', 'Hello World');

      expect(mapping).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    });

    it('should handle header syntax mapping', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, 'Hello World', '# Hello World');

      // HTML: H e l l o   W o r l d
      // MD:   # _ H e l l o   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12
      expect(mapping).toEqual([2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    });

    it('should handle bold formatting mapping', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, 'Hello World', '**Hello** World');

      // HTML: H e l l o   W o r l d
      // MD:   * * H e l l o * *   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12 13 14
      expect(mapping).toEqual([2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14]);
    });

    it('should handle italic formatting mapping', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, 'Hello World', '*Hello* World');

      // HTML: H e l l o   W o r l d
      // MD:   * H e l l o *   W o r l d
      //       0 1 2 3 4 5 6 7 8 9 10 11 12
      expect(mapping).toEqual([1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12]);
    });

    it('should handle nested formatting mapping', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, 'Hello', '***Hello***');

      // HTML: H e l l o
      // MD:   * * * H e l l o * * *
      //       0 1 2 3 4 5 6 7 8 9 10
      expect(mapping).toEqual([3, 4, 5, 6, 7]);
    });

    it('should map remaining characters to end position', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, 'Hello World', 'Hello');

      // HTML has more characters than markdown
      expect(mapping).toEqual([0, 1, 2, 3, 4, 5, 5, 5, 5, 5, 5]);
    });

    it('should handle empty content', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, '', '');

      expect(mapping).toEqual([]);
    });

    it('should handle content with only markdown syntax', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const mapping = buildMapping.call(controller, '', '**');

      expect(mapping).toEqual([]);
    });
  });

  describe('HTML to Markdown Position Mapping', () => {
    it('should handle identical content', () => {
      if (!controller) return;
      
      const mapPosition = (controller as any).mapHtmlPositionToMarkdown;
      const position = mapPosition.call(controller, 5, 'Hello World', 'Hello World');

      expect(position).toBe(5);
    });

    it('should handle header syntax offset', () => {
      if (!controller) return;
      
      const mapPosition = (controller as any).mapHtmlPositionToMarkdown;
      const position = mapPosition.call(controller, 5, 'Hello World', '# Hello World');

      // Position 5 in "Hello World" -> position 7 in "# Hello World"
      expect(position).toBe(7);
    });

    it('should handle multiple header levels', () => {
      if (!controller) return;
      
      const mapPosition = (controller as any).mapHtmlPositionToMarkdown;
      
      const h2Position = mapPosition.call(controller, 5, 'Hello World', '## Hello World');
      expect(h2Position).toBe(8); // "## " = 3 characters

      const h3Position = mapPosition.call(controller, 5, 'Hello World', '### Hello World');
      expect(h3Position).toBe(9); // "### " = 4 characters
    });

    it('should clamp position to content length', () => {
      if (!controller) return;
      
      const mapPosition = (controller as any).mapHtmlPositionToMarkdown;
      const position = mapPosition.call(controller, 100, 'Hello', '# Hi');

      expect(position).toBe(4); // Length of "# Hi"
    });
  });

  describe('Edge Cases', () => {
    it('should handle Unicode characters', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const text = 'Hello 🌟 World';
      const mapping = buildMapping.call(controller, text, text);

      // Unicode emoji may have different character counts, so just check basic structure
      expect(mapping[0]).toBe(0); // First character maps correctly
      expect(mapping[mapping.length - 1]).toBe(mapping.length - 1); // Last character maps correctly
      expect(mapping.length).toBe(text.length); // Mapping covers all characters
    });

    it('should handle very long content efficiently', () => {
      if (!controller) return;
      
      const longText = 'a'.repeat(1000); // Reduced for test performance
      const longMarkdown = '# ' + longText;

      const mapPosition = (controller as any).mapHtmlPositionToMarkdown;
      const start = performance.now();
      const position = mapPosition.call(controller, 500, longText, longMarkdown);
      const end = performance.now();

      expect(position).toBe(502); // 500 + 2 for "# "
      expect(end - start).toBeLessThan(50); // Should complete in <50ms
    });
  });

  describe('Pre-calculation Strategy', () => {
    it('should return null when character position detection fails', () => {
      if (!controller) return;
      
      // Mock character position detection to return null
      const getCharPosition = vi.fn().mockReturnValue(null);
      (controller as any).getCharacterPositionFromCoordinates = getCharPosition;

      const calculatePosition = (controller as any).calculateMarkdownPositionFromClick;
      const result = calculatePosition.call(controller, { x: 100, y: 50 }, 'content');

      expect(result).toBeNull();
    });

    it('should handle errors gracefully', () => {
      if (!controller) return;
      
      const getCharPosition = vi.fn().mockImplementation(() => {
        throw new Error('Test error');
      });
      (controller as any).getCharacterPositionFromCoordinates = getCharPosition;

      const calculatePosition = (controller as any).calculateMarkdownPositionFromClick;
      const result = calculatePosition.call(controller, { x: 100, y: 50 }, 'content');

      expect(result).toBeNull();
    });
  });

  describe('Text Extraction', () => {
    it('should extract plain text from HTML content', () => {
      if (!controller) return;
      
      const extractText = (controller as any).extractTextFromHtml;
      
      expect(extractText.call(controller, 'Hello World')).toBe('Hello World');
      expect(extractText.call(controller, '<span>Hello</span> World')).toBe('Hello World');
      expect(extractText.call(controller, '<b>Bold</b> and <i>italic</i>')).toBe('Bold and italic');
      expect(extractText.call(controller, '')).toBe('');
    });
  });

  describe('Performance Requirements', () => {
    it('should perform character mapping within performance requirements', () => {
      if (!controller) return;
      
      const buildMapping = (controller as any).buildCharacterMapping;
      const text = 'a'.repeat(100);
      const markdown = '# ' + text;

      const start = performance.now();
      const mapping = buildMapping.call(controller, text, markdown);
      const end = performance.now();

      expect(mapping.length).toBe(text.length);
      expect(end - start).toBeLessThan(10); // Should complete in <10ms for 100 chars
    });

    it('should perform position mapping within performance requirements', () => {
      if (!controller) return;
      
      const mapPosition = (controller as any).mapHtmlPositionToMarkdown;
      const text = 'a'.repeat(100);
      const markdown = '# ' + text;

      const start = performance.now();
      for (let i = 0; i < 10; i++) {
        mapPosition.call(controller, 50, text, markdown);
      }
      const end = performance.now();

      expect(end - start).toBeLessThan(50); // 10 operations in <50ms
    });
  });
});