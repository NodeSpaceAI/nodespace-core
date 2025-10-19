import { describe, it, expect } from 'vitest';
import { mapViewPositionToEditPosition } from '$lib/utils/view-edit-mapper';

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
    // Edit: "**Hello** _world_" (should be at 10 = 'w' after skipping leading **)
    expect(mapViewPositionToEditPosition(6, 'Hello world', '**Hello** _world_')).toBe(10);
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
});
