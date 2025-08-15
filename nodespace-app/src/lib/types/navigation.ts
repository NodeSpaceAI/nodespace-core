/**
 * Node Navigation Interface - Entry/Exit Methods
 * Based on GitHub Issue #28: Pluggable Node Navigation System
 */

export interface NodeNavigationMethods {
  /**
   * Can this node accept keyboard navigation?
   */
  canAcceptNavigation(): boolean;
  
  /**
   * Enter node from top (arrow down from previous node)
   * @param columnHint - Suggested column position for cursor
   * @returns true if navigation was handled
   */
  enterFromTop(columnHint?: number): boolean;
  
  /**
   * Enter node from bottom (arrow up from next node)  
   * @param columnHint - Suggested column position for cursor
   * @returns true if navigation was handled
   */
  enterFromBottom(columnHint?: number): boolean;
  
  /**
   * Exit node going up (cursor at first line, arrow up pressed)
   * @returns canExit: whether node allows exit, columnPosition: cursor column for next node
   */
  exitToTop(): { canExit: boolean; columnPosition: number };
  
  /**
   * Exit node going down (cursor at last line, arrow down pressed)
   * @returns canExit: whether node allows exit, columnPosition: cursor column for next node
   */
  exitToBottom(): { canExit: boolean; columnPosition: number };
  
  /**
   * Get current cursor column for cross-node consistency
   * @returns current column position
   */
  getCurrentColumn(): number;
}

export interface NavigationResult {
  handled: boolean;
  columnHint?: number;
}