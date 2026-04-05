/**
 * Unit tests for chat link navigation override logic
 *
 * Tests the decision logic from app-shell.svelte handleLinkClick:
 * - Standard tabs: Click=in-place, Cmd+Click=new tab, Cmd+Shift+Click=other pane
 * - Chat tabs: Click=new tab (preserve chat), Cmd+Click=other pane
 */

import { describe, it, expect } from 'vitest';

/**
 * Replicates the navigation decision logic from app-shell.svelte
 */
function computeNavigation(isFromChat: boolean, modifierPressed: boolean, shiftPressed: boolean) {
  const openInOtherPane = isFromChat ? modifierPressed : (modifierPressed && shiftPressed);
  const openInNewTab = isFromChat ? !modifierPressed : (modifierPressed && !shiftPressed);
  return { openInOtherPane, openInNewTab };
}

describe('Chat Link Navigation Override', () => {
  describe('standard tab (non-chat)', () => {
    it('regular click navigates in-place', () => {
      const result = computeNavigation(false, false, false);
      expect(result.openInNewTab).toBe(false);
      expect(result.openInOtherPane).toBe(false);
    });

    it('Cmd+Click opens in new tab', () => {
      const result = computeNavigation(false, true, false);
      expect(result.openInNewTab).toBe(true);
      expect(result.openInOtherPane).toBe(false);
    });

    it('Cmd+Shift+Click opens in other pane', () => {
      const result = computeNavigation(false, true, true);
      expect(result.openInOtherPane).toBe(true);
      expect(result.openInNewTab).toBe(false);
    });
  });

  describe('chat tab', () => {
    it('regular click opens in new tab (preserves conversation)', () => {
      const result = computeNavigation(true, false, false);
      expect(result.openInNewTab).toBe(true);
      expect(result.openInOtherPane).toBe(false);
    });

    it('Cmd+Click opens in other pane', () => {
      const result = computeNavigation(true, true, false);
      expect(result.openInOtherPane).toBe(true);
      expect(result.openInNewTab).toBe(false);
    });

    it('Cmd+Shift+Click also opens in other pane', () => {
      const result = computeNavigation(true, true, true);
      expect(result.openInOtherPane).toBe(true);
      expect(result.openInNewTab).toBe(false);
    });
  });
});
