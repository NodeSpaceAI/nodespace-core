/**
 * Pattern Splitter
 *
 * Unified interface for splitting content based on registered patterns.
 * Uses the pattern registry to detect which pattern applies and delegates
 * to the appropriate splitting strategy.
 *
 * This is the main entry point for content splitting functionality.
 */

import { PatternRegistry } from '../registry';
import { PrefixInheritanceStrategy } from './strategies/prefix-inheritance';
import { SimpleSplitStrategy } from './strategies/simple-split';
import type { SplitResult, SplittingStrategyImpl, PatternTemplate } from '../types';

// Default fallback pattern for when no pattern is detected
const DEFAULT_PATTERN: PatternTemplate = {
  regex: /^/,
  nodeType: 'unknown',
  priority: 0,
  splittingStrategy: 'simple-split',
  cursorPlacement: 'start'
};

export class PatternSplitter {
  private registry: PatternRegistry;
  private strategies: Map<string, SplittingStrategyImpl>;

  constructor(registry?: PatternRegistry) {
    this.registry = registry || PatternRegistry.getInstance();
    this.strategies = new Map([
      ['prefix-inheritance', new PrefixInheritanceStrategy()],
      ['simple-split', new SimpleSplitStrategy()]
    ]);
  }

  /**
   * Split content at a cursor position based on detected pattern
   * Falls back to simple split if no pattern is detected
   */
  split(content: string, cursorPosition: number, nodeType?: string): SplitResult {
    // Detect pattern (either for specific node type or any pattern)
    const detectionResult = nodeType
      ? this.registry.detectPatternForNodeType(content, nodeType)
      : this.registry.detectPattern(content);

    if (!detectionResult.found || !detectionResult.pattern) {
      // No pattern detected - use simple split as fallback
      return this.splitWithStrategy('simple-split', content, cursorPosition, null);
    }

    const pattern = detectionResult.pattern;
    const strategyType = pattern.splittingStrategy || 'simple-split';

    return this.splitWithStrategy(strategyType, content, cursorPosition, pattern);
  }

  /**
   * Split content using a specific strategy
   */
  private splitWithStrategy(
    strategyType: string,
    content: string,
    cursorPosition: number,
    pattern: PatternTemplate | null
  ): SplitResult {
    const strategy = this.strategies.get(strategyType);
    const resolvedPattern = pattern || DEFAULT_PATTERN;

    if (!strategy) {
      // Fallback to simple split if strategy not found
      const fallback = this.strategies.get('simple-split');
      if (!fallback) {
        throw new Error(`No strategy found: ${strategyType}`);
      }
      return fallback.split(content, cursorPosition, resolvedPattern);
    }

    return strategy.split(content, cursorPosition, resolvedPattern);
  }

  /**
   * Register a custom splitting strategy
   */
  registerStrategy(name: string, strategy: SplittingStrategyImpl): void {
    this.strategies.set(name, strategy);
  }

  /**
   * Get a registered strategy
   */
  getStrategy(name: string): SplittingStrategyImpl | undefined {
    return this.strategies.get(name);
  }
}

// Export singleton instance for convenience
export const patternSplitter = new PatternSplitter();
