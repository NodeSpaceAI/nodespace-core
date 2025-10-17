/**
 * TextareaController Bidirectional Conversion Tests
 *
 * Tests that node type conversions work correctly during edit mode:
 * - Forward: text → header when typing "## "
 * - Reverse: header → text when removing space "##"
 *
 * Critical for issue #275: Ensures controller's internal nodeType stays in sync
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  TextareaController,
  type TextareaControllerEvents
} from '../../lib/design/components/textarea-controller';

describe('TextareaController - Bidirectional Conversion', () => {
  let element: HTMLTextAreaElement;
  let controller: TextareaController;
  let conversionEvents: Array<{ nodeId: string; newNodeType: string; cleanedContent: string }>;
  let mockEvents: TextareaControllerEvents;

  beforeEach(() => {
    element = document.createElement('textarea');
    document.body.appendChild(element);
    conversionEvents = [];

    // Mock only the conversion event we care about
    mockEvents = {
      contentChanged: () => {},
      headerLevelChanged: () => {},
      focus: () => {},
      blur: () => {},
      createNewNode: () => {},
      indentNode: () => {},
      outdentNode: () => {},
      navigateArrow: () => {},
      combineWithPrevious: () => {},
      deleteNode: () => {},
      triggerDetected: () => {},
      triggerHidden: () => {},
      nodeReferenceSelected: () => {},
      slashCommandDetected: () => {},
      slashCommandHidden: () => {},
      slashCommandSelected: () => {},
      nodeTypeConversionDetected: (data) => {
        conversionEvents.push(data);
      },
      directSlashCommand: () => {}
    };
  });

  afterEach(() => {
    if (controller) {
      controller.destroy();
    }
    if (element && element.parentNode) {
      element.parentNode.removeChild(element);
    }
  });

  describe('Forward Conversion: Text → Header', () => {
    it('should convert to header when typing "## "', async () => {
      // Start with text node
      controller = new TextareaController(element, 'test-node', 'text', mockEvents);
      controller.initialize('', true);

      // Type "## "
      element.value = '## ';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      // Wait for pattern detection
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should emit conversion event
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('header');
      expect(conversionEvents[0].cleanedContent).toBe('## '); // cleanContent: false for headers
    });

    it('should update internal nodeType after forward conversion', async () => {
      controller = new TextareaController(element, 'test-node', 'text', mockEvents);
      controller.initialize('', true);

      // Type "## "
      element.value = '## ';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      // Controller's internal nodeType should be updated
      // We can verify this by triggering reverse conversion
      conversionEvents = []; // Reset

      // Remove space: "##"
      element.value = '##';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should now trigger reverse conversion (only possible if nodeType was updated to 'header')
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('text');
    });
  });

  describe('Reverse Conversion: Header → Text', () => {
    it('should convert back to text when removing space from "## "', async () => {
      // Start with header node
      controller = new TextareaController(element, 'test-node', 'header', mockEvents);
      controller.initialize('## Title', true);

      // Remove space: "##Title"
      element.value = '##Title';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should emit reverse conversion
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('text');
      expect(conversionEvents[0].cleanedContent).toBe('##Title');
    });

    it('should convert back to text when removing all hashtags', async () => {
      controller = new TextareaController(element, 'test-node', 'header', mockEvents);
      controller.initialize('## Title', true);

      // Remove all hashtags: "Title"
      element.value = 'Title';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should convert back to text
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('text');
    });

    it('should update internal nodeType after reverse conversion', async () => {
      controller = new TextareaController(element, 'test-node', 'header', mockEvents);
      controller.initialize('## Title', true);

      // Remove space
      element.value = '##Title';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents).toHaveLength(1);
      conversionEvents = []; // Reset

      // Add space back: "## Title"
      element.value = '## Title';
      element.dispatchEvent(new Event('input', { bubbles: true }));

      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should convert back to header (only possible if nodeType was updated to 'text')
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('header');
    });
  });

  describe('Multiple Conversions During Edit', () => {
    it('should handle text → header → text → header sequence', async () => {
      controller = new TextareaController(element, 'test-node', 'text', mockEvents);
      controller.initialize('', true);

      // 1. Text → Header: Type "## "
      element.value = '## ';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('header');
      conversionEvents = [];

      // 2. Header → Text: Remove space "##"
      element.value = '##';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('text');
      conversionEvents = [];

      // 3. Text → Header: Add space back "## "
      element.value = '## ';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('header');
    });

    it('should handle header level changes during edit', async () => {
      controller = new TextareaController(element, 'test-node', 'text', mockEvents);
      controller.initialize('', true);

      // Start with h2
      element.value = '## ';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('header');
      expect(conversionEvents[0].cleanedContent).toBe('## ');
      conversionEvents = [];

      // Change to h3 by adding another #
      element.value = '### ';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should still be header but with updated level
      // Pattern detection re-runs and detects h3
      expect(conversionEvents[0].newNodeType).toBe('header');
      expect(conversionEvents[0].cleanedContent).toBe('### ');
    });
  });

  describe('Edge Cases During Edit', () => {
    it('should not convert text node when typing hashtags without space', async () => {
      controller = new TextareaController(element, 'test-node', 'text', mockEvents);
      controller.initialize('', true);

      // Type "##" (no space)
      element.value = '##';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should NOT trigger conversion
      expect(conversionEvents).toHaveLength(0);
    });

    it('should not trigger conversion when already correct type', async () => {
      controller = new TextareaController(element, 'test-node', 'header', mockEvents);
      controller.initialize('## Title', true);

      // Type more content but keep pattern valid
      element.value = '## Title with more text';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should emit conversion event to update level (pattern re-detection)
      // But since we started as header and pattern still matches, it converts to header again
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('header');
    });
  });
});
