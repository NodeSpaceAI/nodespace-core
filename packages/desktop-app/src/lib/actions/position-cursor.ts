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
  | 'node-type-conversion'
  | 'inherited-type'; // Issue #664: For nodes created via Enter key that inherit parent type

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
 * Issue #664: For nodes created via Enter key that inherit parent type
 * These nodes have a type-locked pattern state (cannot revert to text)
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

  function applyPosition(data: CursorPosition | null, controller: TextareaControllerState | null): void {
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

    // Use requestAnimationFrame for smooth, non-blocking positioning
    // CRITICAL: Do NOT clear focusManager.cursorPosition from the action
    // The initialize() method in textarea-controller will check and clear it
    // This avoids a race condition where the RAF runs before initialize()
    requestAnimationFrame(() => {
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
          // Issue #664: Position cursor for inherited type nodes (Enter key on typed node)
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
