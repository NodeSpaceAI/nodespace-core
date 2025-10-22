/**
 * Simple Split Splitting Strategy
 *
 * Used for node types that don't have special prefix handling:
 * - Text nodes
 * - Task nodes
 *
 * Behavior:
 * - Splits content at cursor position
 * - Preserves inline markdown formatting on both sides
 * - Closes formatting in first node, opens in second node
 */

import type { PatternTemplate, SplitResult, SplittingStrategyImpl } from '../../types';
import {
  analyzeMarkdownContext,
  getClosingMarkers,
  getOpeningMarkers,
  hasActiveFormatting
} from '../../markdown-context';

export class SimpleSplitStrategy implements SplittingStrategyImpl {
  split(content: string, position: number, _pattern: PatternTemplate): SplitResult {
    // Handle edge cases for boundary positions
    if (position <= 0) {
      return {
        beforeContent: '',
        afterContent: content,
        newNodeCursorPosition: 0
      };
    }

    if (position >= content.length) {
      return {
        beforeContent: content,
        afterContent: '',
        newNodeCursorPosition: 0
      };
    }

    // Split content at cursor position
    const beforeCursor = content.substring(0, position);
    const afterCursor = content.substring(position);

    // Analyze markdown context at split position
    const context = analyzeMarkdownContext(content, position);

    // If no active formatting, return simple split
    if (!hasActiveFormatting(context)) {
      return {
        beforeContent: beforeCursor,
        afterContent: afterCursor,
        newNodeCursorPosition: 0
      };
    }

    // Complete the formatting in the before content
    const closingMarkers = getClosingMarkers(context);
    const beforeContent = beforeCursor + closingMarkers;

    // Start new content with opening markers
    const openingMarkers = getOpeningMarkers(context);
    const newContent = openingMarkers + afterCursor;

    // Calculate cursor position in new node (after opening markers)
    const newNodeCursorPosition = openingMarkers.length;

    return {
      beforeContent,
      afterContent: newContent,
      newNodeCursorPosition
    };
  }
}
