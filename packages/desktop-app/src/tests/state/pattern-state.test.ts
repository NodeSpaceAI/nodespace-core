/**
 * PatternState Unit Tests
 *
 * Tests for the PatternState class that manages pattern detection lifecycle.
 * Issue #664: Replace scattered nodeTypeSetViaPattern flag with explicit state machine.
 */

import { describe, it, expect } from 'vitest';
import {
  PatternState,
  createPatternState,
  type PatternMatch
} from '$lib/state/pattern-state.svelte';
import type { PatternTemplate } from '$lib/patterns/types';

// Mock pattern template for testing
const mockHeaderPattern: PatternTemplate = {
  regex: /^#\s/,
  nodeType: 'header',
  priority: 10,
  splittingStrategy: 'prefix-inheritance',
  cursorPlacement: 'after-prefix'
};

const mockQuotePattern: PatternTemplate = {
  regex: /^>\s/,
  nodeType: 'quote-block',
  priority: 10,
  splittingStrategy: 'prefix-inheritance',
  cursorPlacement: 'after-prefix'
};

const mockOrderedListPattern: PatternTemplate = {
  regex: /^1\.\s/,
  nodeType: 'ordered-list',
  priority: 10,
  splittingStrategy: 'prefix-inheritance',
  cursorPlacement: 'after-prefix'
};

// Helper to create a PatternMatch
function createPatternMatch(
  pattern: PatternTemplate,
  content: string
): PatternMatch | null {
  const match = pattern.regex.exec(content);
  if (!match) return null;
  return {
    pattern,
    match,
    nodeType: pattern.nodeType
  };
}

describe('PatternState', () => {
  describe('Constructor and Initial State', () => {
    it('should initialize with user source by default', () => {
      const state = new PatternState('user');
      expect(state.creationSource).toBe('user');
      expect(state.detectedPattern).toBeNull();
      expect(state.canRevert).toBe(false);
      expect(state.shouldDetectPatterns).toBe(true);
    });

    it('should initialize with pattern source and store pattern', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      expect(state.creationSource).toBe('pattern');
      expect(state.detectedPattern).toStrictEqual(patternMatch);
      expect(state.canRevert).toBe(true);
      expect(state.shouldDetectPatterns).toBe(false);
    });

    it('should initialize with inherited source', () => {
      const state = new PatternState('inherited');

      expect(state.creationSource).toBe('inherited');
      expect(state.detectedPattern).toBeNull();
      // Inherited nodes CAN revert (user clarification: all nodes detect pattern changes)
      expect(state.canRevert).toBe(true);
      expect(state.shouldDetectPatterns).toBe(false);
    });
  });

  describe('canRevert', () => {
    it('should return true for pattern source with detected pattern and inherited source', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;

      // Pattern source with pattern - can revert
      const patternState = new PatternState('pattern', patternMatch);
      expect(patternState.canRevert).toBe(true);

      // User source - cannot revert (no pattern detected yet)
      const userState = new PatternState('user');
      expect(userState.canRevert).toBe(false);

      // Inherited source - CAN revert (all nodes detect pattern changes)
      const inheritedState = new PatternState('inherited');
      expect(inheritedState.canRevert).toBe(true);

      // Pattern source without pattern - cannot revert
      const patternNoMatch = new PatternState('pattern');
      expect(patternNoMatch.canRevert).toBe(false);
    });
  });

  describe('shouldDetectPatterns', () => {
    it('should return true only for user source', () => {
      expect(new PatternState('user').shouldDetectPatterns).toBe(true);
      expect(new PatternState('pattern').shouldDetectPatterns).toBe(false);
      expect(new PatternState('inherited').shouldDetectPatterns).toBe(false);
    });
  });

  describe('shouldWatchForReversion', () => {
    it('should return true for pattern source with detected pattern and inherited source', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;

      const patternState = new PatternState('pattern', patternMatch);
      expect(patternState.shouldWatchForReversion).toBe(true);

      const userState = new PatternState('user');
      expect(userState.shouldWatchForReversion).toBe(false);

      // Inherited source also watches for reversion (all nodes detect pattern changes)
      const inheritedState = new PatternState('inherited');
      expect(inheritedState.shouldWatchForReversion).toBe(true);
    });
  });

  describe('recordPatternMatch', () => {
    it('should transition user to pattern source', () => {
      const state = new PatternState('user');
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;

      state.recordPatternMatch(patternMatch, '# Hello');

      expect(state.creationSource).toBe('pattern');
      expect(state.detectedPattern).toStrictEqual(patternMatch);
      expect(state.canRevert).toBe(true);
      expect(state.shouldDetectPatterns).toBe(false);
    });

    it('should update pattern info for subsequent matches', () => {
      const state = new PatternState('user');
      const headerMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const quoteMatch = createPatternMatch(mockQuotePattern, '> Quote')!;

      state.recordPatternMatch(headerMatch, '# Hello');
      expect(state.detectedPattern?.nodeType).toBe('header');

      state.recordPatternMatch(quoteMatch, '> Quote');
      expect(state.detectedPattern?.nodeType).toBe('quote-block');
    });
  });

  describe('patternStillMatches', () => {
    it('should return true when content still matches pattern', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      expect(state.patternStillMatches('# Hello')).toBe(true);
      expect(state.patternStillMatches('# Different text')).toBe(true);
      expect(state.patternStillMatches('# ')).toBe(true);
    });

    it('should return false when pattern is deleted', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      expect(state.patternStillMatches('Hello')).toBe(false);
      expect(state.patternStillMatches('')).toBe(false);
      expect(state.patternStillMatches('#Hello')).toBe(false); // Missing space
    });

    it('should return false when no pattern detected', () => {
      const state = new PatternState('user');
      expect(state.patternStillMatches('# Hello')).toBe(false);
    });
  });

  describe('attemptRevert', () => {
    it('should revert when pattern is deleted', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      // Pattern deleted
      const shouldRevert = state.attemptRevert('Hello');

      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
      expect(state.detectedPattern).toBeNull();
      expect(state.canRevert).toBe(false);
      expect(state.shouldDetectPatterns).toBe(true);
    });

    it('should not revert when pattern still exists', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      const shouldRevert = state.attemptRevert('# Still has pattern');

      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('pattern');
      expect(state.detectedPattern).toStrictEqual(patternMatch);
    });

    it('should not revert for user source', () => {
      const state = new PatternState('user');

      const shouldRevert = state.attemptRevert('any content');

      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('user');
    });

    it('should revert for inherited source (all nodes can revert)', () => {
      const state = new PatternState('inherited');

      // Inherited nodes CAN revert when syntax is deleted
      // Since no pattern is stored, it always reverts
      const shouldRevert = state.attemptRevert('any content');

      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });

  describe('resetToUser', () => {
    it('should clear pattern and set user source', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      state.resetToUser();

      expect(state.creationSource).toBe('user');
      expect(state.detectedPattern).toBeNull();
      expect(state.canRevert).toBe(false);
      expect(state.shouldDetectPatterns).toBe(true);
    });
  });

  describe('setPatternExists', () => {
    it('should upgrade user to pattern when pattern matches', () => {
      const state = new PatternState('user');
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;

      state.setPatternExists(patternMatch);

      expect(state.creationSource).toBe('pattern');
      expect(state.detectedPattern).toStrictEqual(patternMatch);
      expect(state.canRevert).toBe(true);
    });

    it('should store pattern for inherited source (for reversion capability)', () => {
      const state = new PatternState('inherited');
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;

      state.setPatternExists(patternMatch);

      // Source stays 'inherited' but pattern is stored for reversion detection
      expect(state.creationSource).toBe('inherited');
      expect(state.detectedPattern).toStrictEqual(patternMatch);
    });
  });

  describe('setCreationSource', () => {
    it('should update source and clear pattern for non-pattern sources', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      state.setCreationSource('inherited');

      expect(state.creationSource).toBe('inherited');
      expect(state.detectedPattern).toBeNull();
    });

    it('should preserve pattern when changing to pattern source', () => {
      const state = new PatternState('user');
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;

      state.recordPatternMatch(patternMatch, '# Hello');
      state.setCreationSource('pattern');

      expect(state.creationSource).toBe('pattern');
      expect(state.detectedPattern).toStrictEqual(patternMatch);
    });
  });

  describe('getDebugState', () => {
    it('should return complete debug information', () => {
      const patternMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      const state = new PatternState('pattern', patternMatch);

      const debug = state.getDebugState();

      expect(debug).toEqual({
        creationSource: 'pattern',
        hasPattern: true,
        patternNodeType: 'header',
        canRevert: true,
        shouldDetectPatterns: false
      });
    });
  });
});

describe('createPatternState Factory', () => {
  describe('User-created nodes', () => {
    it('should return user source for text nodes', () => {
      const state = createPatternState(false, false, 'text');
      expect(state.creationSource).toBe('user');
      expect(state.shouldDetectPatterns).toBe(true);
    });

    it('should return user source for non-inherited non-conversion nodes', () => {
      const state = createPatternState(false, false, 'header');
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Pattern-created nodes', () => {
    it('should return pattern source for type conversions', () => {
      const state = createPatternState(true, false, 'header');
      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(false); // No pattern recorded yet
    });

    it('should return user source for text type conversions', () => {
      const state = createPatternState(true, false, 'text');
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Inherited nodes', () => {
    it('should return inherited source for Enter on typed node', () => {
      const state = createPatternState(false, true, 'header');
      expect(state.creationSource).toBe('inherited');
      // Inherited nodes CAN revert (all nodes detect pattern changes)
      expect(state.canRevert).toBe(true);
    });

    it('should return user source for inherited text nodes', () => {
      // Enter on text node creates another text node - should detect patterns
      const state = createPatternState(false, true, 'text');
      expect(state.creationSource).toBe('user');
      expect(state.shouldDetectPatterns).toBe(true);
    });
  });

  describe('Edge cases', () => {
    it('should handle quote-block nodes correctly', () => {
      // Quote blocks inherit their type
      const inherited = createPatternState(false, true, 'quote-block');
      expect(inherited.creationSource).toBe('inherited');

      // Quote blocks created via pattern can revert
      const pattern = createPatternState(true, false, 'quote-block');
      expect(pattern.creationSource).toBe('pattern');
    });

    it('should handle ordered-list nodes correctly', () => {
      const inherited = createPatternState(false, true, 'ordered-list');
      expect(inherited.creationSource).toBe('inherited');

      const pattern = createPatternState(true, false, 'ordered-list');
      expect(pattern.creationSource).toBe('pattern');
    });
  });
});

describe('Pattern Lifecycle Scenarios', () => {
  describe('Header pattern workflow', () => {
    it('should handle: user types "# " → header → deletes "#" → text', () => {
      // 1. Start as user-created text node
      const state = new PatternState('user');
      expect(state.shouldDetectPatterns).toBe(true);

      // 2. User types "# " - pattern detected
      const headerMatch = createPatternMatch(mockHeaderPattern, '# Hello')!;
      state.recordPatternMatch(headerMatch, '# Hello');
      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(true);

      // 3. User continues typing
      expect(state.patternStillMatches('# Hello World')).toBe(true);

      // 4. User deletes "# " prefix
      const shouldRevert = state.attemptRevert('Hello World');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
      expect(state.shouldDetectPatterns).toBe(true);
    });
  });

  describe('Inherited header workflow', () => {
    it('should revert when Enter creates new header and syntax is deleted', () => {
      // User presses Enter on a header - new node inherits header type
      const state = new PatternState('inherited');
      // Inherited nodes CAN revert (all nodes detect pattern changes)
      expect(state.canRevert).toBe(true);

      // When user deletes the "# " prefix, node reverts to text
      const shouldRevert = state.attemptRevert('No prefix');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Page load workflow', () => {
    it('should enable reversion for loaded nodes matching their pattern', () => {
      // Node loaded from DB: header with "# Title" content
      const state = new PatternState('user');
      const headerMatch = createPatternMatch(mockHeaderPattern, '# Title')!;

      // Initialize sets pattern exists (content matches type's pattern)
      state.setPatternExists(headerMatch);

      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(true);

      // User can now delete "# " to revert to text
      const shouldRevert = state.attemptRevert('Title');
      expect(shouldRevert).toBe(true);
    });
  });

  describe('Quote block workflow', () => {
    it('should handle quote block creation and reversion', () => {
      const state = new PatternState('user');
      const quoteMatch = createPatternMatch(mockQuotePattern, '> Quote text')!;

      state.recordPatternMatch(quoteMatch, '> Quote text');
      expect(state.creationSource).toBe('pattern');

      // Delete "> " prefix
      const shouldRevert = state.attemptRevert('Quote text');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Ordered list workflow', () => {
    it('should handle ordered list creation and reversion', () => {
      const state = new PatternState('user');
      const listMatch = createPatternMatch(mockOrderedListPattern, '1. Item')!;

      state.recordPatternMatch(listMatch, '1. Item');
      expect(state.creationSource).toBe('pattern');

      // Delete "1. " prefix
      const shouldRevert = state.attemptRevert('Item');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });
});
