/**
 * positionCursor - Svelte 5 Action for Reactive Cursor Positioning
 *
 * Encapsulates all DOM manipulation for cursor positioning in a declarative action.
 * Replaces imperative $effect blocks with reactive data-driven positioning.
 *
 * Architecture:
 * - Action accepts cursor position data from FocusManager via $derived
 * - Uses requestAnimationFrame for smooth, non-blocking positioning
 * - Handles different cursor position types: default, line-column, arrow-navigation, link
 * - Integrates with TextareaController for actual cursor manipulation
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

import type { TextareaControllerState } from '$lib/design/components/textarea-controller';

/**
 * Cursor position data types
 */
export type CursorPositionType =
  | 'default'
  | 'absolute'
  | 'arrow-navigation'
  | 'line-column'
  | 'node-type-conversion';

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

export type CursorPosition =
  | CursorPositionDefault
  | CursorPositionAbsolute
  | CursorPositionArrowNavigation
  | CursorPositionLineColumn
  | CursorPositionNodeTypeConversion;

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

  function applyPosition(data: CursorPosition | null, controller: TextareaControllerState | null): void {
    // Skip if no data or no controller
    if (!data || !controller) {
      return;
    }

    // Skip if this is the same data we just processed (prevent duplicate positioning)
    if (lastProcessedData === data) {
      return;
    }

    lastProcessedData = data;

    // Use requestAnimationFrame for smooth, non-blocking positioning
    requestAnimationFrame(() => {
      switch (data.type) {
        case 'default':
          // Position at beginning of first line, optionally skipping syntax
          controller.positionCursorAtLineBeginning(0, data.skipSyntax ?? true);
          break;

        case 'absolute':
          // Position at specific character offset
          controller.setCursorPosition(data.position);
          break;

        case 'arrow-navigation':
          // Position from arrow navigation with pixel-accurate horizontal alignment
          controller.enterFromArrowNavigation(data.direction, data.pixelOffset);
          break;

        case 'line-column':
          // Position at beginning of specific line, optionally skipping syntax
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
      }
    });
  }

  // Initial position on mount
  applyPosition(params.data, params.controller);

  return {
    update(newParams: PositionCursorParams) {
      // Reset lastProcessedData if data becomes null (allows re-application of same position)
      if (newParams.data === null) {
        lastProcessedData = null;
      }
      applyPosition(newParams.data, newParams.controller);
    }
  };
}
