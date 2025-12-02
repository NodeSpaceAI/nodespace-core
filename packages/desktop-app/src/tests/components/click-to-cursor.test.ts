import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createMockElementForView } from '$lib/design/components/cursor-positioning';
import { mapViewPositionToEditPosition } from '$lib/utils/view-edit-mapper';

/**
 * Extract text from HTML element while preserving line breaks from <br> tags
 * This is the same logic used in base-node.svelte
 */
function extractTextWithLineBreaks(element: HTMLElement): string {
  let text = '';
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      text += node.textContent || '';
    } else if (node.nodeName === 'BR') {
      text += '\n';
    } else if (node.childNodes) {
      node.childNodes.forEach(walk);
    }
  };
  walk(element);
  return text;
}

describe('Click-to-Cursor Positioning', () => {
  beforeEach(() => {
    // Reset any global state if needed
    vi.clearAllMocks();
  });

  describe('extractTextWithLineBreaks', () => {
    it('extracts plain text without br tags', () => {
      const div = document.createElement('div');
      div.textContent = 'Hello world';
      expect(extractTextWithLineBreaks(div)).toBe('Hello world');
    });

    it('preserves newlines from br tags', () => {
      const div = document.createElement('div');
      div.innerHTML = 'Line 1<br>Line 2<br>Line 3';
      expect(extractTextWithLineBreaks(div)).toBe('Line 1\nLine 2\nLine 3');
    });

    it('handles mixed text nodes and br tags', () => {
      const div = document.createElement('div');
      div.innerHTML = 'First<br><strong>Second</strong><br>Third';
      expect(extractTextWithLineBreaks(div)).toBe('First\nSecond\nThird');
    });

    it('handles nested elements with br tags', () => {
      const div = document.createElement('div');
      div.innerHTML = '<p>Para 1<br>Para 2</p>';
      expect(extractTextWithLineBreaks(div)).toBe('Para 1\nPara 2');
    });

    it('handles empty content', () => {
      const div = document.createElement('div');
      expect(extractTextWithLineBreaks(div)).toBe('');
    });
  });

  it('creates mock element and finds character position - plain text', () => {
    // Setup a mock view element
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.lineHeight = '1.5';
    viewDiv.style.padding = '0px';
    document.body.appendChild(viewDiv);

    const content = 'Hello world';
    const mockElement = createMockElementForView(viewDiv, content);

    // Verify mock element was created with character spans
    const spans = mockElement.querySelectorAll('[data-position]');
    expect(spans.length).toBeGreaterThan(0);
    expect(spans[0].textContent).toBe('H');
    expect(spans[spans.length - 1].textContent).toBe('d');

    // Clean up
    mockElement.remove();
    viewDiv.remove();
  });

  it('maps plain text positions correctly', () => {
    const view = 'Hello world';
    const edit = 'Hello world';

    // No mapping needed when content is same
    expect(mapViewPositionToEditPosition(0, view, edit)).toBe(0);
    expect(mapViewPositionToEditPosition(5, view, edit)).toBe(5);
    expect(mapViewPositionToEditPosition(11, view, edit)).toBe(11);
  });

  it('maps positions correctly with bold syntax', () => {
    const view = 'Hello world';
    const edit = '**Hello** world';

    // Position 0 -> skip ** -> position 2
    expect(mapViewPositionToEditPosition(0, view, edit)).toBe(2);
    // Position 6 -> skip ** -> position 10
    expect(mapViewPositionToEditPosition(6, view, edit)).toBe(10);
  });

  it('handles emoji and complex Unicode in mock element', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.padding = '0px';
    document.body.appendChild(viewDiv);

    const content = '😀 Hello';
    const mockElement = createMockElementForView(viewDiv, content);

    // Should handle emoji correctly
    const spans = mockElement.querySelectorAll('[data-position]');
    expect(spans.length).toBe(content.length);

    mockElement.remove();
    viewDiv.remove();
  });

  it('handles multi-line content in mock element', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.padding = '0px';
    document.body.appendChild(viewDiv);

    const content = 'Line 1\nLine 2\nLine 3';
    const mockElement = createMockElementForView(viewDiv, content);

    // Should create spans for each character including newlines
    const spans = mockElement.querySelectorAll('[data-position]');
    const brs = mockElement.querySelectorAll('br');
    expect(spans.length).toBe(content.length);
    expect(brs.length).toBe(2); // Two newlines

    mockElement.remove();
    viewDiv.remove();
  });

  it('mock element positioning matches view element', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.position = 'absolute';
    viewDiv.style.top = '100px';
    viewDiv.style.left = '200px';
    viewDiv.style.width = '500px';
    document.body.appendChild(viewDiv);

    const content = 'Test content';
    const mockElement = createMockElementForView(viewDiv, content);

    const viewRect = viewDiv.getBoundingClientRect();
    const mockRect = mockElement.getBoundingClientRect();

    // Mock element should be positioned at same location as view element
    expect(mockRect.left).toBe(viewRect.left);
    expect(mockRect.top).toBe(viewRect.top);
    expect(mockRect.width).toBe(viewRect.width);

    mockElement.remove();
    viewDiv.remove();
  });

  it('extracts text with line breaks from view element with br tags', () => {
    const viewDiv = document.createElement('div');
    viewDiv.innerHTML = 'Line 1<br>Line 2<br>Line 3';
    document.body.appendChild(viewDiv);

    const extractedText = extractTextWithLineBreaks(viewDiv);
    expect(extractedText).toBe('Line 1\nLine 2\nLine 3');

    // Verify this differs from textContent (which doesn't preserve br as \n)
    expect(viewDiv.textContent).not.toContain('\n');
    expect(extractedText).toContain('\n');

    viewDiv.remove();
  });

  it('handles empty content gracefully', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.padding = '0px';
    document.body.appendChild(viewDiv);

    const content = '';
    const mockElement = createMockElementForView(viewDiv, content);

    // Should not crash with empty content
    const spans = mockElement.querySelectorAll('[data-position]');
    expect(spans.length).toBe(0);

    mockElement.remove();
    viewDiv.remove();
  });

  it('handles special characters in content', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.padding = '0px';
    document.body.appendChild(viewDiv);

    const content = 'Hello @mention #hashtag';
    const mockElement = createMockElementForView(viewDiv, content);

    // Should handle special characters
    const spans = mockElement.querySelectorAll('[data-position]');
    expect(spans.length).toBe(content.length);

    mockElement.remove();
    viewDiv.remove();
  });

  it('mock element copies view styles', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'Arial';
    viewDiv.style.fontSize = '20px';
    viewDiv.style.fontWeight = 'bold';
    viewDiv.style.lineHeight = '2';
    viewDiv.style.padding = '10px';
    document.body.appendChild(viewDiv);

    const content = 'Test';
    const mockElement = createMockElementForView(viewDiv, content);

    // Verify base styles are always present
    expect(mockElement.style.position).toBe('absolute');
    expect(mockElement.style.visibility).toBe('hidden');
    expect(mockElement.style.whiteSpace).toBe('pre-wrap');

    // In environments with getComputedStyle support (real browsers),
    // verify computed styles are copied
    if (typeof window !== 'undefined' && typeof window.getComputedStyle === 'function') {
      expect(mockElement.style.fontFamily).toContain('Arial');
      expect(mockElement.style.fontSize).toContain('20');
      expect(mockElement.style.fontWeight).toBe('bold');
      expect(mockElement.style.lineHeight).toContain('2');
      expect(mockElement.style.padding).toContain('10');
    }

    mockElement.remove();
    viewDiv.remove();
  });

  it('handles position mapping with multiple formatting', () => {
    const view = 'Hello world';
    const edit = '**Hello** _world_';

    // Position 0 -> skip ** -> position 2
    expect(mapViewPositionToEditPosition(0, view, edit)).toBe(2);
    // Position 6 -> skip **, then space, then skip _ opener -> position 11 (the 'w' char)
    // Position 10 is the _ marker, position 11 is actual 'w' character
    expect(mapViewPositionToEditPosition(6, view, edit)).toBe(11);
  });

  it('full integration: extract text from view with br tags and create mock element', () => {
    const viewDiv = document.createElement('div');
    viewDiv.style.fontFamily = 'monospace';
    viewDiv.style.fontSize = '16px';
    viewDiv.style.padding = '0px';
    // Simulate markdown-rendered content with <br> tags
    viewDiv.innerHTML = 'Multi-line:<br>Line with <strong>bold</strong>';
    document.body.appendChild(viewDiv);

    // Extract with preserved line breaks
    const viewText = extractTextWithLineBreaks(viewDiv);
    expect(viewText).toBe('Multi-line:\nLine with bold');

    // Create mock element with extracted text
    const mockElement = createMockElementForView(viewDiv, viewText);

    // Verify mock has correct structure with newlines
    const spans = mockElement.querySelectorAll('[data-position]');
    const brs = mockElement.querySelectorAll('br');
    expect(spans.length).toBe(viewText.length);
    expect(brs.length).toBe(1); // One newline

    // Verify mock positioning matches view
    const viewRect = viewDiv.getBoundingClientRect();
    const mockRect = mockElement.getBoundingClientRect();
    expect(mockRect.left).toBe(viewRect.left);
    expect(mockRect.top).toBe(viewRect.top);

    mockElement.remove();
    viewDiv.remove();
  });
});
