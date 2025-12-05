import { describe, it, expect } from 'vitest';
import {
  mapViewPositionToEditPosition,
  mapEditPositionToViewPosition,
  stripAllMarkdown,
  stripInlineMarkdown
} from '$lib/utils/view-edit-mapper';

describe('stripAllMarkdown', () => {
  it('strips header prefix and inline formatting', () => {
    expect(stripAllMarkdown('### *And* again')).toBe('And again');
  });

  it('strips quote prefix and inline formatting', () => {
    expect(stripAllMarkdown('> **Quote** text')).toBe('Quote text');
  });

  it('strips ordered list prefix', () => {
    expect(stripAllMarkdown('1. List item')).toBe('List item');
  });

  it('strips unordered list prefix (dash)', () => {
    expect(stripAllMarkdown('- List item')).toBe('List item');
  });

  it('strips unordered list prefix (asterisk)', () => {
    expect(stripAllMarkdown('* List item')).toBe('List item');
  });

  it('handles multiple quote levels', () => {
    expect(stripAllMarkdown('>> Nested quote')).toBe('Nested quote');
  });

  it('handles plain text without syntax', () => {
    expect(stripAllMarkdown('Plain text')).toBe('Plain text');
  });

  it('handles complex formatting', () => {
    expect(stripAllMarkdown('### **Bold** and *italic* text')).toBe('Bold and italic text');
  });
});

describe('mapViewPositionToEditPosition', () => {
  it('returns same position when view === edit (no syntax)', () => {
    expect(mapViewPositionToEditPosition(5, 'Hello world', 'Hello world')).toBe(5);
  });

  it('maps position correctly with bold syntax', () => {
    // View: "Hello world" (click at 6 = 'w')
    // Edit: "**Hello** world" (should be at 10 = 'w')
    expect(mapViewPositionToEditPosition(6, 'Hello world', '**Hello** world')).toBe(10);
  });

  it('maps position correctly with italic syntax', () => {
    // View: "Hello world" (click at 0 = 'H')
    // Edit: "*Hello* world" (should be at 1 = 'H')
    expect(mapViewPositionToEditPosition(0, 'Hello world', '*Hello* world')).toBe(1);
  });

  it('maps position correctly with multiple formatting', () => {
    // View: "Hello world" (click at 6 = 'w')
    // Edit: "**Hello** _world_" (should be at 11 = 'w' after skipping _ opener)
    // Position 10 is the _ marker, position 11 is the actual 'w' character
    expect(mapViewPositionToEditPosition(6, 'Hello world', '**Hello** _world_')).toBe(11);
  });

  it('handles position at end of content', () => {
    const view = 'Hello';
    const edit = '**Hello**';
    // Position 5 (after 'o' in view) maps to position 7 (after 'o' in edit, before **)
    expect(mapViewPositionToEditPosition(5, view, edit)).toBe(7);
  });

  it('handles strikethrough syntax', () => {
    // View: "Hello world" (click at 6 = 'w')
    // Edit: "~~Hello~~ world" (should be at 10 = 'w')
    expect(mapViewPositionToEditPosition(6, 'Hello world', '~~Hello~~ world')).toBe(10);
  });

  it('handles code syntax', () => {
    // View: "Hello world" (click at 0 = 'H')
    // Edit: "`Hello` world" (should be at 1 = 'H')
    expect(mapViewPositionToEditPosition(0, 'Hello world', '`Hello` world')).toBe(1);
  });

  it('handles position in middle of bold text', () => {
    // View: "Hello world" (click at 2 = 'l')
    // Edit: "**Hello** world" (should be at 4 = 'l')
    expect(mapViewPositionToEditPosition(2, 'Hello world', '**Hello** world')).toBe(4);
  });

  it('handles single character italic at beginning', () => {
    // View: "a" (click at 0 = 'a')
    // Edit: "*a* word" (should be at 1 = 'a')
    expect(mapViewPositionToEditPosition(0, 'a', '*a*')).toBe(1);
  });

  it('handles mixed bold and italic', () => {
    // View: "Hello" (click at 0 = 'H')
    // Edit: "***Hello***" (should be at 3 = 'H' after ***, ***, **)
    expect(mapViewPositionToEditPosition(0, 'Hello', '***Hello***')).toBe(3);
  });

  // Prefix syntax tests
  it('maps position correctly with header prefix', () => {
    // View: "And again" (click at 0 = 'A')
    // Edit: "### And again" (should be at 4 = 'A' after "### ")
    expect(mapViewPositionToEditPosition(0, 'And again', '### And again')).toBe(4);
  });

  it('maps position correctly with header prefix and inline formatting', () => {
    // View: "And again" (click at 0 = 'A')
    // Edit: "### *And* again" (should be at 5 = 'A' after "### " and "*")
    expect(mapViewPositionToEditPosition(0, 'And again', '### *And* again')).toBe(5);
  });

  it('maps middle position with header prefix', () => {
    // View: "And again" (click at 4 = 'a' in "again")
    // Edit: "### And again" (should be at 8 = 'a')
    expect(mapViewPositionToEditPosition(4, 'And again', '### And again')).toBe(8);
  });

  it('maps position with quote prefix', () => {
    // View: "Quote text" (click at 0 = 'Q')
    // Edit: "> Quote text" (should be at 2 = 'Q')
    expect(mapViewPositionToEditPosition(0, 'Quote text', '> Quote text')).toBe(2);
  });

  it('maps position with ordered list prefix', () => {
    // View: "List item" (click at 0 = 'L')
    // Edit: "1. List item" (should be at 3 = 'L')
    expect(mapViewPositionToEditPosition(0, 'List item', '1. List item')).toBe(3);
  });

  it('maps position with unordered list prefix (dash)', () => {
    // View: "List item" (click at 0 = 'L')
    // Edit: "- List item" (should be at 2 = 'L')
    expect(mapViewPositionToEditPosition(0, 'List item', '- List item')).toBe(2);
  });

  it('maps position with unordered list prefix (asterisk)', () => {
    // View: "List item" (click at 0 = 'L')
    // Edit: "* List item" (should be at 2 = 'L')
    expect(mapViewPositionToEditPosition(0, 'List item', '* List item')).toBe(2);
  });

  it('maps position with multiple quote levels', () => {
    // View: "Quote" (click at 0 = 'Q')
    // Edit: ">> Quote" (should be at 3 = 'Q')
    expect(mapViewPositionToEditPosition(0, 'Quote', '>> Quote')).toBe(3);
  });

  it('maps position with nested markers at end', () => {
    // View: "Hello" (click at 5 = after 'o')
    // Edit: "**Hello**" (should be at 7 = after 'o', before closing **)
    expect(mapViewPositionToEditPosition(5, 'Hello', '**Hello**')).toBe(7);
  });

  it('skips opening markers after reaching target position', () => {
    // View: "Hello world test" (click at 6 = 'w')
    // Edit: "Hello *world* test" (should be at 7 = 'w' after skipping opening *)
    expect(mapViewPositionToEditPosition(6, 'Hello world test', 'Hello *world* test')).toBe(7);
  });

  it('handles closing marker detection - does not skip closing markers', () => {
    // View: "Hello" (click at 5 = after 'o')
    // Edit: "*Hello*" (should be at 6 = after 'o', before closing *)
    // The closing marker * is followed by end of string, so it's NOT an opening marker
    expect(mapViewPositionToEditPosition(5, 'Hello', '*Hello*')).toBe(6);
  });

  it('handles consecutive markers (bold + italic)', () => {
    // View: "Hello" (click at 0 = 'H')
    // Edit: "***Hello***" (should be at 3 = 'H' after ***)
    expect(mapViewPositionToEditPosition(0, 'Hello', '***Hello***')).toBe(3);
  });

  it('handles marker followed by another marker (nested formatting)', () => {
    // View: "Hello" (click at 5 = after 'o')
    // Edit: "***Hello***" (should be at 8 = after 'o', before closing ***)
    expect(mapViewPositionToEditPosition(5, 'Hello', '***Hello***')).toBe(8);
  });

  it('handles underscore bold syntax', () => {
    // View: "Hello world" (click at 0 = 'H')
    // Edit: "__Hello__ world" (should be at 2 = 'H')
    expect(mapViewPositionToEditPosition(0, 'Hello world', '__Hello__ world')).toBe(2);
  });

  it('handles underscore italic syntax', () => {
    // View: "Hello world" (click at 6 = 'w')
    // Edit: "Hello _world_" (should be at 7 = 'w')
    expect(mapViewPositionToEditPosition(6, 'Hello world', 'Hello _world_')).toBe(7);
  });

  it('handles six-level header prefix', () => {
    // View: "Header" (click at 0 = 'H')
    // Edit: "###### Header" (should be at 7 = 'H')
    expect(mapViewPositionToEditPosition(0, 'Header', '###### Header')).toBe(7);
  });

  it('handles multiple markers in sequence while walking', () => {
    // View: "Hello world" (click at 6 = 'w')
    // Edit: "**Hello** **world**" (should be at 12 = 'w' after skipping opening **)
    expect(mapViewPositionToEditPosition(6, 'Hello world', '**Hello** **world**')).toBe(12);
  });

  it('handles position beyond view content length', () => {
    // View: "Hello" (click at 10 = beyond content)
    // Edit: "**Hello**" (should be at last position)
    // This tests the while loop boundary condition
    const result = mapViewPositionToEditPosition(10, 'Hello', '**Hello**');
    // Should stop at end of edit content after processing
    expect(result).toBeLessThanOrEqual(9);
  });

  it('handles empty view/edit content edge case', () => {
    expect(mapViewPositionToEditPosition(0, '', '')).toBe(0);
  });

  it('handles position in middle of formatted text with prefix', () => {
    // View: "Hello world" (click at 2 = 'l')
    // Edit: "### **Hello** world" (should be at 8 = 'l')
    // Position breakdown: "### " (4) + "**" (2) + "He" (2) = 8
    expect(mapViewPositionToEditPosition(2, 'Hello world', '### **Hello** world')).toBe(8);
  });
});

describe('mapEditPositionToViewPosition', () => {
  it('maps edit position to view with header prefix', () => {
    // Edit: "### And again" (cursor at 4 = 'A')
    // View: "And again" (corresponds to 0 = 'A')
    expect(mapEditPositionToViewPosition(4, '### And again')).toBe(0);
  });

  it('maps edit position to view with header prefix and inline formatting', () => {
    // Edit: "### *And* again" (cursor at 5 = 'A')
    // View: "And again" (corresponds to 0 = 'A')
    expect(mapEditPositionToViewPosition(5, '### *And* again')).toBe(0);
  });

  it('returns 0 for positions within prefix', () => {
    // Edit: "### And again" (cursor at 2 = '#')
    // View position should be 0 (within prefix)
    expect(mapEditPositionToViewPosition(2, '### And again')).toBe(0);
  });

  it('maps middle position with prefix', () => {
    // Edit: "### And again" (cursor at 8 = 'a' in "again")
    // View: "And again" (corresponds to 4 = 'a')
    expect(mapEditPositionToViewPosition(8, '### And again')).toBe(4);
  });

  it('maps position with inline formatting', () => {
    // Edit: "**Hello** world" (cursor at 10 = 'w')
    // View: "Hello world" (corresponds to 6 = 'w')
    expect(mapEditPositionToViewPosition(10, '**Hello** world')).toBe(6);
  });

  it('returns 0 for position 0', () => {
    expect(mapEditPositionToViewPosition(0, '**Hello** world')).toBe(0);
  });

  it('maps position with quote prefix', () => {
    // Edit: "> Quote text" (cursor at 2 = 'Q')
    // View: "Quote text" (corresponds to 0 = 'Q')
    expect(mapEditPositionToViewPosition(2, '> Quote text')).toBe(0);
  });

  it('maps position with ordered list prefix', () => {
    // Edit: "1. List item" (cursor at 3 = 'L')
    // View: "List item" (corresponds to 0 = 'L')
    expect(mapEditPositionToViewPosition(3, '1. List item')).toBe(0);
  });

  it('maps position with multiple nested markers', () => {
    // Edit: "***Hello*** world" (cursor at 12 = 'w')
    // View: "Hello world" (corresponds to 6 = 'w')
    expect(mapEditPositionToViewPosition(12, '***Hello*** world')).toBe(6);
  });
});

describe('stripInlineMarkdown', () => {
  it('strips bold (asterisk) syntax', () => {
    expect(stripInlineMarkdown('**Hello** world')).toBe('Hello world');
  });

  it('strips bold (underscore) syntax', () => {
    expect(stripInlineMarkdown('__Hello__ world')).toBe('Hello world');
  });

  it('strips strikethrough syntax', () => {
    expect(stripInlineMarkdown('~~Hello~~ world')).toBe('Hello world');
  });

  it('strips italic (asterisk) syntax', () => {
    expect(stripInlineMarkdown('*Hello* world')).toBe('Hello world');
  });

  it('strips italic (underscore) syntax', () => {
    expect(stripInlineMarkdown('_Hello_ world')).toBe('Hello world');
  });

  it('strips code syntax', () => {
    expect(stripInlineMarkdown('`Hello` world')).toBe('Hello world');
  });

  it('preserves header prefix while stripping inline formatting', () => {
    expect(stripInlineMarkdown('### *And* again')).toBe('### And again');
  });

  it('preserves quote prefix while stripping inline formatting', () => {
    expect(stripInlineMarkdown('> **Quote** text')).toBe('> Quote text');
  });

  it('preserves ordered list prefix while stripping inline formatting', () => {
    expect(stripInlineMarkdown('1. **List** item')).toBe('1. List item');
  });

  it('preserves unordered list prefix while stripping inline formatting', () => {
    expect(stripInlineMarkdown('- *List* item')).toBe('- List item');
  });

  it('handles multiple inline formatting types', () => {
    expect(stripInlineMarkdown('**Bold** and *italic* and ~~strike~~ and `code`')).toBe(
      'Bold and italic and strike and code'
    );
  });

  it('handles plain text without formatting', () => {
    expect(stripInlineMarkdown('Plain text')).toBe('Plain text');
  });
});
