/**
 * Type-Safe DateNode Wrapper
 *
 * Provides type narrowing for date nodes while maintaining
 * the universal Node storage model.
 *
 * Date nodes have deterministic IDs in YYYY-MM-DD format and serve
 * as root nodes for daily journals.
 *
 * @example
 * ```typescript
 * import { Node } from '$lib/types/node';
 * import { DateNode, isDateNode, getDate } from '$lib/types/date-node';
 *
 * // Type guard
 * if (isDateNode(node)) {
 *   const date = getDate(node);
 *   console.log(`Date: ${date}`);
 * }
 * ```
 */

import type { Node } from './node';

/**
 * DateNode interface extending base Node
 *
 * Represents a date node that serves as a root for daily content.
 * The ID is always in YYYY-MM-DD format.
 */
export interface DateNode extends Node {
  nodeType: 'date';
}

/**
 * Type guard to check if a node is a date node
 *
 * @param node - Node to check
 * @returns True if node is a date node
 *
 * @example
 * ```typescript
 * if (isDateNode(node)) {
 *   // TypeScript knows node is DateNode here
 *   const date = getDate(node);
 * }
 * ```
 */
export function isDateNode(node: Node): node is DateNode {
  return node.nodeType === 'date';
}

/**
 * Get the date from a date node's ID
 *
 * Date node IDs are in YYYY-MM-DD format, which is also the date.
 *
 * @param node - Date node
 * @returns Date string in YYYY-MM-DD format
 *
 * @example
 * ```typescript
 * getDate(dateNode); // "2025-01-15"
 * ```
 */
export function getDate(node: DateNode): string {
  return node.id;
}

/**
 * Parse the date from a date node's ID into a Date object
 *
 * @param node - Date node
 * @returns JavaScript Date object
 */
export function getDateObject(node: DateNode): Date {
  return new Date(node.id + 'T00:00:00');
}

/**
 * Check if a date node ID is valid (YYYY-MM-DD format)
 *
 * @param id - String to check
 * @returns True if valid date ID format
 */
export function isValidDateId(id: string): boolean {
  const dateRegex = /^\d{4}-\d{2}-\d{2}$/;
  if (!dateRegex.test(id)) return false;

  // Also validate it's a real date
  const date = new Date(id + 'T00:00:00');
  return !isNaN(date.getTime()) && id === date.toISOString().split('T')[0];
}

/**
 * Generate a deterministic date node ID from a Date object
 *
 * @param date - JavaScript Date object
 * @returns Date ID in YYYY-MM-DD format
 */
export function generateDateId(date: Date): string {
  return date.toISOString().split('T')[0];
}

/**
 * Helper namespace for date node operations
 */
export const DateNodeHelpers = {
  isDateNode,
  getDate,
  getDateObject,
  isValidDateId,
  generateDateId,

  /**
   * Create a new date node for today
   *
   * @returns New date node for today
   */
  createTodayNode(): DateNode {
    const today = new Date();
    const id = generateDateId(today);
    return {
      id,
      nodeType: 'date',
      content: '',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  },

  /**
   * Create a date node for a specific date
   *
   * @param date - JavaScript Date object or YYYY-MM-DD string
   * @returns New date node
   */
  createDateNode(date: Date | string): DateNode {
    const id = typeof date === 'string' ? date : generateDateId(date);
    return {
      id,
      nodeType: 'date',
      content: '',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {}
    };
  },

  /**
   * Check if a date node represents today
   */
  isToday(node: DateNode): boolean {
    return node.id === generateDateId(new Date());
  },

  /**
   * Check if a date node represents a past date
   */
  isPast(node: DateNode): boolean {
    const today = generateDateId(new Date());
    return node.id < today;
  },

  /**
   * Check if a date node represents a future date
   */
  isFuture(node: DateNode): boolean {
    const today = generateDateId(new Date());
    return node.id > today;
  },

  /**
   * Get display-friendly date string
   *
   * @param node - Date node
   * @param options - Intl.DateTimeFormat options
   * @returns Formatted date string
   */
  getDisplayDate(
    node: DateNode,
    options: Intl.DateTimeFormatOptions = { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' }
  ): string {
    const date = getDateObject(node);
    return date.toLocaleDateString(undefined, options);
  }
};
