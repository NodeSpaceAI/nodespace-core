/**
 * Quote Block Reversion Tests
 * 
 * Tests that quote-block nodes can revert to text when the "> " pattern is deleted.
 * Issue: When user backspaces from "> Hello" to ">Hello", the node should revert to text.
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  TextareaController,
  type TextareaControllerEvents
} from '../../lib/design/components/textarea-controller';

describe('Quote Block Reversion', () => {
  let element: HTMLTextAreaElement;
  let controller: TextareaController;
  let conversionEvents: Array<{ nodeId: string; newNodeType: string; cleanedContent: string }>;
  let mockEvents: TextareaControllerEvents;

  beforeEach(() => {
    element = document.createElement('textarea');
    document.body.appendChild(element);
    conversionEvents = [];

    mockEvents = {
      contentChanged: () => {},
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

  describe('Forward Conversion: Text → Quote Block', () => {
    it('should convert to quote-block when typing "> "', async () => {
      controller = new TextareaController(element, 'test-node', 'text', 'default', mockEvents);
      controller.initialize('', true);

      element.value = '> ';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('quote-block');
      expect(conversionEvents[0].cleanedContent).toBe('> '); // Content preserved
    });

    it('should convert to quote-block with content', async () => {
      controller = new TextareaController(element, 'test-node', 'text', 'default', mockEvents);
      controller.initialize('', true);

      element.value = '> Hello';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('quote-block');
      expect(conversionEvents[0].cleanedContent).toBe('> Hello'); // Content preserved!
    });
  });

  describe('Reverse Conversion: Quote Block → Text', () => {
    it('should revert to text when removing space from "> Hello" to ">Hello"', async () => {
      // Start with quote-block node
      controller = new TextareaController(element, 'test-node', 'quote-block', 'default', mockEvents);
      controller.initialize('> Hello', true);

      // Remove space: ">Hello"
      element.value = '>Hello';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      // Should emit reverse conversion
      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('text');
      expect(conversionEvents[0].cleanedContent).toBe('>Hello');
    });

    it('should revert to text when removing > entirely', async () => {
      controller = new TextareaController(element, 'test-node', 'quote-block', 'default', mockEvents);
      controller.initialize('> Hello', true);

      // Remove > entirely
      element.value = 'Hello';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents).toHaveLength(1);
      expect(conversionEvents[0].newNodeType).toBe('text');
      expect(conversionEvents[0].cleanedContent).toBe('Hello');
    });
  });

  describe('Bidirectional Conversion', () => {
    it('should handle text → quote → text → quote sequence', async () => {
      controller = new TextareaController(element, 'test-node', 'text', 'default', mockEvents);
      controller.initialize('', true);

      // 1. Text → Quote: Type "> "
      element.value = '> Hello';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('quote-block');
      conversionEvents = [];

      // 2. Quote → Text: Remove space ">Hello"
      element.value = '>Hello';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('text');
      conversionEvents = [];

      // 3. Text → Quote: Add space back "> Hello"
      element.value = '> Hello';
      element.dispatchEvent(new Event('input', { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(conversionEvents[0].newNodeType).toBe('quote-block');
    });
  });
});
