/**
 * Pattern Registry
 *
 * Singleton registry for all markdown patterns in the system.
 * Plugins register their patterns here to enable:
 * - Pattern detection (for node type conversion)
 * - Pattern-based content splitting
 * - WYSIWYG rendering (future)
 */

import type { PatternTemplate, PatternDetectionResult } from './types';

export class PatternRegistry {
  private static instance: PatternRegistry;
  private patterns = new Map<string, PatternTemplate>();
  private patternsByPriority: PatternTemplate[] = [];

  /**
   * Get the singleton instance
   */
  static getInstance(): PatternRegistry {
    if (!PatternRegistry.instance) {
      PatternRegistry.instance = new PatternRegistry();
    }
    return PatternRegistry.instance;
  }

  /**
   * Register a pattern template
   * Patterns are sorted by priority for efficient detection
   */
  register(pattern: PatternTemplate): void {
    // Validate pattern
    if (!pattern.nodeType || !pattern.regex) {
      throw new Error('Pattern must have nodeType and regex');
    }

    // Store by node type for quick lookup
    this.patterns.set(pattern.nodeType, pattern);

    // Rebuild priority-sorted list
    this.rebuildPriorityList();
  }

  /**
   * Get a pattern by node type
   */
  getPattern(nodeType: string): PatternTemplate | undefined {
    return this.patterns.get(nodeType);
  }

  /**
   * Detect if content matches any registered pattern
   * Returns the highest priority matching pattern
   */
  detectPattern(content: string): PatternDetectionResult {
    // Check patterns in priority order (highest first)
    for (const pattern of this.patternsByPriority) {
      // Reset regex lastIndex for global patterns
      if (pattern.regex.global) {
        pattern.regex.lastIndex = 0;
      }

      const match = pattern.regex.exec(content);
      if (match) {
        return {
          pattern,
          match,
          found: true
        };
      }
    }

    return {
      pattern: null,
      match: null,
      found: false
    };
  }

  /**
   * Detect patterns in content for a specific node type
   */
  detectPatternForNodeType(content: string, nodeType: string): PatternDetectionResult {
    const pattern = this.patterns.get(nodeType);
    if (!pattern) {
      return {
        pattern: null,
        match: null,
        found: false
      };
    }

    // Reset regex lastIndex for global patterns
    if (pattern.regex.global) {
      pattern.regex.lastIndex = 0;
    }

    const match = pattern.regex.exec(content);
    return {
      pattern: match ? pattern : null,
      match,
      found: !!match
    };
  }

  /**
   * Check if content starts with a pattern
   */
  startsWithPattern(content: string): PatternDetectionResult {
    // Content must match at start (position 0)
    const result = this.detectPattern(content);
    if (result.found && result.match && result.match.index === 0) {
      return result;
    }
    return {
      pattern: null,
      match: null,
      found: false
    };
  }

  /**
   * Get all registered patterns
   */
  getAllPatterns(): PatternTemplate[] {
    return Array.from(this.patterns.values());
  }

  /**
   * Check if a pattern is registered for a node type
   */
  hasPattern(nodeType: string): boolean {
    return this.patterns.has(nodeType);
  }

  /**
   * Clear all patterns (for testing only)
   *
   * @internal This method should only be used in test environments.
   * Calling this in production will break pattern detection functionality.
   *
   * @throws {Error} If called in production environment
   */
  clear(): void {
    // Guard against accidental production usage
    if (import.meta.env.PROD) {
      throw new Error(
        'PatternRegistry.clear() is for testing only and cannot be called in production. ' +
          'This would break pattern detection functionality.'
      );
    }

    this.patterns.clear();
    this.patternsByPriority = [];
  }

  /**
   * Get registry statistics
   */
  getStats(): {
    patternCount: number;
    registeredNodeTypes: string[];
  } {
    return {
      patternCount: this.patterns.size,
      registeredNodeTypes: Array.from(this.patterns.keys())
    };
  }

  /**
   * Rebuild the priority-sorted list
   * Called whenever patterns are registered
   */
  private rebuildPriorityList(): void {
    this.patternsByPriority = Array.from(this.patterns.values()).sort(
      (a, b) => (b.priority ?? 0) - (a.priority ?? 0)
    );
  }
}
