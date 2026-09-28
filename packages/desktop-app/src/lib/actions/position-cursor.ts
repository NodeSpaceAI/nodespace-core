/**
 * positionCursor - Svelte 5 Action for Reactive Cursor Positioning
 *
 * Encapsulates all DOM manipulation for cursor positioning in a declarative action.
 * Replaces imperative $effect blocks with reactive data-driven positioning.
 *
 * Architecture:
 * - Action accepts cursor position data from FocusManager via $derived
 * - Defers via Svelte's tick() so this runs after the reactive DOM update that
 *   mounted the target textarea (see "Why tick(), not requestAnimationFrame" below)
 * - Handles different cursor position types: default, line-column, arrow-navigation, link
 * - Integrates with TextareaController for actual cursor manipulation
 *
 * Why tick(), not requestAnimationFrame:
 * This action fires the moment its textarea is inserted into the DOM, which can be
 * BEFORE TextareaController.initialize() (a sibling $effect on the same state change)
 * has run and set `element.value`. Positioning before that would clamp against an
 * empty string and lose the requested offset, so applying the position needs to wait
 * until that effect has definitely run. requestAnimationFrame achieved that by luck
 * (it always fires well after the current reactive flush), but it also waits for the
 * next paint (~16ms) -- long enough for a fast keystroke right after a click-to-edit or
 * Enter-creates-sibling transition to be typed into an element that isn't focused yet
 * and be silently dropped (repro'd via Playwright with a 15ms per-keystroke delay).
 * tick() waits for exactly the same pending reactive flush (it resolves via
 * `Promise.resolve()` + `flushSync()`) but settles on the next microtask instead of
 * the next frame -- always before the browser dispatches the next keydown, even under
 * fast/automated typing.
 *
 * Usage (in Svelte component):
 * ```typescript
 * // Derive cursor data from FocusManager
 * const cursorData = $derived(
 *   isEditing ? focusManager.cursorPosition : null
 * );
 *
 * // Apply action to textarea element
 * // <textarea use:positionCursor={{ data: cursorData, controller }} />
 * ```
 */

import { tick } from 'svelte';
import type { TextareaControllerState } from '$lib/design/components/textarea-controller';

/**
 * Cursor position data types
 */
export type CursorPositionType =
  | 'default'
  | 'absolute'
  | 'arrow-navigation'
  | 'line-column'
  | 'node-type-conversion'
  | 'inherited-type'; // For nodes created via Enter key that inherit parent type

export interface CursorPositionDefault {
  type: 'default';
  skipSyntax?: boolean;
}

export interface CursorPositionAbsolute {
  type: 'absolute';
  position: number;
}

export interface CursorPositionArrowNavigation {
  type: 'arrow-navigation';
  direction: 'up' | 'down';
  pixelOffset: number;
}

export interface CursorPositionLineColumn {
  type: 'line-column';
  line: number;
  skipSyntax?: boolean;
}

export interface CursorPositionNodeTypeConversion {
  type: 'node-type-conversion';
  position: number;
}

/**
 * For nodes created via Enter key that inherit parent type
 * These nodes get an 'inherited' pattern state (reverts to text only if the plugin's canRevert allows)
 */
export interface CursorPositionInheritedType {
  type: 'inherited-type';
  position: number;
}

export type CursorPosition =
  | CursorPositionDefault
  | CursorPositionAbsolute
  | CursorPositionArrowNavigation
  | CursorPositionLineColumn
  | CursorPositionNodeTypeConversion
  | CursorPositionInheritedType;

export interface PositionCursorParams {
  data: CursorPosition | null;
  controller: TextareaControllerState | null;
}

/**
 * Svelte action for reactive cursor positioning
 *
 * @param element - The textarea element to position cursor in
 * @param params - Cursor position data and controller reference
 * @returns Action lifecycle object with update method
 */
export function positionCursor(
  element: HTMLTextAreaElement,
  params: PositionCursorParams
): { update: (params: PositionCursorParams) => void } {
  let lastProcessedData: CursorPosition | null = null;

  async function applyPosition(
    data: CursorPosition | null,
    controller: TextareaControllerState | null
  ): Promise<void> {
    // Skip if no data or no controller
    if (!data || !controller) {
      return;
    }

    // Skip if this is the same data we just processed (prevent duplicate positioning)
    // Use JSON comparison since $derived may return same object reference but we need
    // to handle re-renders that pass the same position data
    if (lastProcessedData !== null && JSON.stringify(lastProcessedData) === JSON.stringify(data)) {
      return;
    }

    lastProcessedData = data;

    // Wait for the pending reactive flush (which includes TextareaController.initialize()
    // setting element.value) to land before touching the element ourselves.
    // CRITICAL: Do NOT clear focusManager.cursorPosition from the action
    // The initialize() method in textarea-controller will check and clear it
    // This avoids a race condition where this callback runs before initialize()
    await tick();

    switch (data.type) {
        case 'default':
          // Position at beginning of first line, optionally skipping syntax
          // CRITICAL: Focus first, then set position
          controller.focus();
          controller.positionCursorAtLineBeginning(0, data.skipSyntax ?? true);
          break;

        case 'absolute':
          // Position at specific character offset
          // CRITICAL: Focus first, then set position
          controller.focus();
          controller.setCursorPosition(data.position);
          break;

        case 'arrow-navigation':
          // Position from arrow navigation with pixel-accurate horizontal alignment
          // enterFromArrowNavigation handles focus internally
          controller.enterFromArrowNavigation(data.direction, data.pixelOffset);
          break;

        case 'line-column':
          // Position at beginning of specific line, optionally skipping syntax
          // CRITICAL: Focus first, then set position
          controller.focus();
          controller.positionCursorAtLineBeginning(data.line, data.skipSyntax ?? true);
          break;

        case 'node-type-conversion':
          // Position cursor after node type conversion (similar to arrow navigation)
          // Focus first, then set position with retry logic for component switches
          controller.focus();
          controller.setCursorPosition(data.position);

          // Verify and retry if needed (component switches may reset cursor)
          setTimeout(() => {
            const textarea = document.activeElement as HTMLTextAreaElement;
            if (
              controller &&
              textarea &&
              textarea.tagName === 'TEXTAREA' &&
              textarea.selectionStart !== data.position
            ) {
              controller.setCursorPosition(data.position);
            }
          }, 10);
          break;

        case 'inherited-type':
          // Position cursor for inherited type nodes (Enter key on typed node)
          // Same positioning as node-type-conversion, but controller will use 'inherited' source
          controller.focus();
          controller.setCursorPosition(data.position);

          // Verify and retry if needed (component switches may reset cursor)
          setTimeout(() => {
            const textarea = document.activeElement as HTMLTextAreaElement;
            if (
              controller &&
              textarea &&
              textarea.tagName === 'TEXTAREA' &&
              textarea.selectionStart !== data.position
            ) {
              controller.setCursorPosition(data.position);
            }
          }, 10);
          break;
      }
  }

  // Initial position on mount. Fire-and-forget: applyPosition awaits tick() internally
  // and the action's lifecycle contract (mount/update/destroy) is synchronous.
  void applyPosition(params.data, params.controller);

  return {
    update(newParams: PositionCursorParams) {
      // Reset lastProcessedData if data becomes null (allows re-application of same position)
      if (newParams.data === null) {
        lastProcessedData = null;
      }
      void applyPosition(newParams.data, newParams.controller);
    }
  };
}
