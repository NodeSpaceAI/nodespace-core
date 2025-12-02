import { describe, it, expect } from 'vitest';
import {
  mapViewPositionToEditPosition,
  mapEditPositionToViewPosition,
  stripAllMarkdown
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
});
