/**
 * PatternState Unit Tests
 *
 * Tests for the PatternState class that manages pattern detection lifecycle.
 * Issue #664: Replace scattered nodeTypeSetViaPattern flag with explicit state machine.
 * Issue #667: Plugin-owned pattern behavior (canRevert, revert regex).
 */

import { describe, it, expect } from 'vitest';
import { PatternState, createPatternState } from '$lib/state/pattern-state.svelte';
import type { PluginDefinition } from '$lib/plugins/types';

// Mock plugins for testing - these simulate real plugin pattern configurations
const mockHeaderPlugin: PluginDefinition = {
  id: 'header',
  name: 'Header Node',
  description: 'Header with level 1-6',
  version: '1.0.0',
  config: {
    slashCommands: [],
    canHaveChildren: true,
    canBeChild: true
  },
  pattern: {
    detect: /^#\s/,
    canRevert: true,
    revert: /^#$/, // "# " → "#" triggers reversion
    onEnter: 'inherit',
    splittingStrategy: 'prefix-inheritance',
    cursorPlacement: 'after-prefix'
  }
};

const mockQuotePlugin: PluginDefinition = {
  id: 'quote-block',
  name: 'Quote Block Node',
  description: 'Block quote',
  version: '1.0.0',
  config: {
    slashCommands: [],
    canHaveChildren: true,
    canBeChild: true
  },
  pattern: {
    detect: /^>\s/,
    canRevert: true,
    revert: /^>$/, // "> " → ">" triggers reversion
    onEnter: 'inherit',
    splittingStrategy: 'prefix-inheritance',
    cursorPlacement: 'after-prefix'
  }
};

const mockOrderedListPlugin: PluginDefinition = {
  id: 'ordered-list',
  name: 'Ordered List Node',
  description: 'Numbered list',
  version: '1.0.0',
  config: {
    slashCommands: [],
    canHaveChildren: false,
    canBeChild: true
  },
  pattern: {
    detect: /^1\.\s/,
    canRevert: true,
    revert: /^1\.$/, // "1. " → "1." triggers reversion
    onEnter: 'inherit',
    splittingStrategy: 'prefix-inheritance',
    cursorPlacement: 'after-prefix'
  }
};

const mockTaskPlugin: PluginDefinition = {
  id: 'task',
  name: 'Task Node',
  description: 'Task with checkbox',
  version: '1.0.0',
  config: {
    slashCommands: [],
    canHaveChildren: true,
    canBeChild: true
  },
  pattern: {
    detect: /^[-*+]?\s*\[\s*[xX\s]\s*\]\s/,
    canRevert: false, // Task nodes strip their prefix from content (cleanContent pattern), so reversion is not applicable
    onEnter: 'inherit',
    splittingStrategy: 'simple-split',
    cursorPlacement: 'start'
  }
};

describe('PatternState', () => {
  describe('Constructor and Initial State', () => {
    it('should initialize with user source by default', () => {
      const state = new PatternState('user');
      expect(state.creationSource).toBe('user');
      expect(state.plugin).toBeNull();
      expect(state.canRevert).toBe(false);
      expect(state.shouldDetectPatterns).toBe(true);
    });

    it('should initialize with pattern source and store plugin', () => {
      const state = new PatternState('pattern', mockHeaderPlugin);

      expect(state.creationSource).toBe('pattern');
      expect(state.plugin?.id).toBe('header');
      expect(state.canRevert).toBe(true); // Header has canRevert: true
      expect(state.shouldDetectPatterns).toBe(false);
    });

    it('should initialize with inherited source and plugin', () => {
      const state = new PatternState('inherited', mockHeaderPlugin);

      expect(state.creationSource).toBe('inherited');
      expect(state.plugin?.id).toBe('header');
      // Inherited header nodes CAN revert (plugin has canRevert: true)
      expect(state.canRevert).toBe(true);
      expect(state.shouldDetectPatterns).toBe(false);
    });

    it('should not allow reversion for task nodes (canRevert: false)', () => {
      const state = new PatternState('pattern', mockTaskPlugin);

      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(false); // Task plugin has canRevert: false
    });

    it('should not allow reversion for inherited task nodes', () => {
      const state = new PatternState('inherited', mockTaskPlugin);

      expect(state.creationSource).toBe('inherited');
      expect(state.canRevert).toBe(false); // Task plugin has canRevert: false
    });
  });

  describe('canRevert', () => {
    it('should respect plugin canRevert setting', () => {
      // Header plugin has canRevert: true
      const headerPatternState = new PatternState('pattern', mockHeaderPlugin);
      expect(headerPatternState.canRevert).toBe(true);

      const headerInheritedState = new PatternState('inherited', mockHeaderPlugin);
      expect(headerInheritedState.canRevert).toBe(true);

      // Task plugin has canRevert: false
      const taskPatternState = new PatternState('pattern', mockTaskPlugin);
      expect(taskPatternState.canRevert).toBe(false);

      const taskInheritedState = new PatternState('inherited', mockTaskPlugin);
      expect(taskInheritedState.canRevert).toBe(false);

      // User source - cannot revert (no pattern detected yet)
      const userState = new PatternState('user');
      expect(userState.canRevert).toBe(false);

      // No plugin set - cannot revert
      const noPluginState = new PatternState('pattern');
      expect(noPluginState.canRevert).toBe(false);
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
    it('should match canRevert behavior', () => {
      const headerState = new PatternState('pattern', mockHeaderPlugin);
      expect(headerState.shouldWatchForReversion).toBe(true);

      const taskState = new PatternState('pattern', mockTaskPlugin);
      expect(taskState.shouldWatchForReversion).toBe(false);

      const userState = new PatternState('user');
      expect(userState.shouldWatchForReversion).toBe(false);

      const inheritedHeaderState = new PatternState('inherited', mockHeaderPlugin);
      expect(inheritedHeaderState.shouldWatchForReversion).toBe(true);

      const inheritedTaskState = new PatternState('inherited', mockTaskPlugin);
      expect(inheritedTaskState.shouldWatchForReversion).toBe(false);
    });
  });

  describe('recordPluginPatternMatch', () => {
    it('should transition user to pattern source', () => {
      const state = new PatternState('user');

      state.recordPluginPatternMatch(mockHeaderPlugin);

      expect(state.creationSource).toBe('pattern');
      expect(state.plugin?.id).toBe('header');
      expect(state.canRevert).toBe(true);
      expect(state.shouldDetectPatterns).toBe(false);
    });

    it('should update plugin info for subsequent matches', () => {
      const state = new PatternState('user');

      state.recordPluginPatternMatch(mockHeaderPlugin);
      expect(state.plugin?.id).toBe('header');

      state.recordPluginPatternMatch(mockQuotePlugin);
      expect(state.plugin?.id).toBe('quote-block');
    });
  });

  describe('attemptRevert', () => {
    it('should revert when plugin revert pattern matches', () => {
      const state = new PatternState('pattern', mockHeaderPlugin);

      // "# " → "#" triggers reversion (revert pattern: /^#$/)
      const shouldRevert = state.attemptRevert('#');

      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
      expect(state.plugin).toBeNull();
      expect(state.canRevert).toBe(false);
      expect(state.shouldDetectPatterns).toBe(true);
    });

    it('should not revert when revert pattern does not match', () => {
      const state = new PatternState('pattern', mockHeaderPlugin);

      // Still has "# " prefix - should not revert
      const shouldRevert = state.attemptRevert('# Hello');

      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('pattern');
      expect(state.plugin?.id).toBe('header');
    });

    it('should not revert for user source', () => {
      const state = new PatternState('user');

      const shouldRevert = state.attemptRevert('any content');

      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('user');
    });

    it('should not revert for task nodes (canRevert: false)', () => {
      const state = new PatternState('pattern', mockTaskPlugin);

      // Task nodes cannot revert regardless of content
      const shouldRevert = state.attemptRevert('');

      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('pattern');
    });

    it('should revert for inherited header nodes (canRevert: true)', () => {
      const state = new PatternState('inherited', mockHeaderPlugin);

      // "#" matches revert pattern /^#$/
      const shouldRevert = state.attemptRevert('#');

      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });

    it('should not revert for inherited task nodes (canRevert: false)', () => {
      const state = new PatternState('inherited', mockTaskPlugin);

      const shouldRevert = state.attemptRevert('any content');

      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('inherited');
    });
  });

  describe('resetToUser', () => {
    it('should clear plugin and set user source', () => {
      const state = new PatternState('pattern', mockHeaderPlugin);

      state.resetToUser();

      expect(state.creationSource).toBe('user');
      expect(state.plugin).toBeNull();
      expect(state.canRevert).toBe(false);
      expect(state.shouldDetectPatterns).toBe(true);
    });
  });

  describe('setPluginPatternExists', () => {
    it('should upgrade user to pattern when plugin matches', () => {
      const state = new PatternState('user');

      state.setPluginPatternExists(mockHeaderPlugin);

      expect(state.creationSource).toBe('pattern');
      expect(state.plugin?.id).toBe('header');
      expect(state.canRevert).toBe(true);
    });

    it('should store plugin for inherited source (for reversion capability)', () => {
      const state = new PatternState('inherited');

      state.setPluginPatternExists(mockHeaderPlugin);

      // Source stays 'inherited' but plugin is stored for reversion detection
      expect(state.creationSource).toBe('inherited');
      expect(state.plugin?.id).toBe('header');
      expect(state.canRevert).toBe(true);
    });
  });

  describe('setCreationSource', () => {
    it('should update source', () => {
      const state = new PatternState('pattern', mockHeaderPlugin);

      state.setCreationSource('inherited');

      expect(state.creationSource).toBe('inherited');
      // Plugin is preserved
      expect(state.plugin?.id).toBe('header');
    });
  });

  describe('getDebugState', () => {
    it('should return complete debug information', () => {
      const state = new PatternState('pattern', mockHeaderPlugin);

      const debug = state.getDebugState();

      expect(debug).toEqual({
        creationSource: 'pattern',
        hasPlugin: true,
        pluginId: 'header',
        canRevert: true,
        shouldDetectPatterns: false
      });
    });

    it('should show null pluginId when no plugin', () => {
      const state = new PatternState('user');

      const debug = state.getDebugState();

      expect(debug.pluginId).toBeNull();
      expect(debug.hasPlugin).toBe(false);
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
    it('should return pattern source with plugin for type conversions', () => {
      const state = createPatternState(true, false, 'header', mockHeaderPlugin);
      expect(state.creationSource).toBe('pattern');
      expect(state.plugin?.id).toBe('header');
      expect(state.canRevert).toBe(true);
    });

    it('should return pattern source without plugin (canRevert false)', () => {
      const state = createPatternState(true, false, 'header');
      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(false); // No plugin, cannot revert
    });

    it('should return user source for text type conversions', () => {
      const state = createPatternState(true, false, 'text');
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Inherited nodes', () => {
    it('should return inherited source with plugin for Enter on typed node', () => {
      const state = createPatternState(false, true, 'header', mockHeaderPlugin);
      expect(state.creationSource).toBe('inherited');
      expect(state.plugin?.id).toBe('header');
      expect(state.canRevert).toBe(true); // Header plugin has canRevert: true
    });

    it('should return inherited source for task nodes (canRevert: false)', () => {
      const state = createPatternState(false, true, 'task', mockTaskPlugin);
      expect(state.creationSource).toBe('inherited');
      expect(state.plugin?.id).toBe('task');
      expect(state.canRevert).toBe(false); // Task plugin has canRevert: false
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
      const inherited = createPatternState(false, true, 'quote-block', mockQuotePlugin);
      expect(inherited.creationSource).toBe('inherited');
      expect(inherited.canRevert).toBe(true);

      const pattern = createPatternState(true, false, 'quote-block', mockQuotePlugin);
      expect(pattern.creationSource).toBe('pattern');
      expect(pattern.canRevert).toBe(true);
    });

    it('should handle ordered-list nodes correctly', () => {
      const inherited = createPatternState(false, true, 'ordered-list', mockOrderedListPlugin);
      expect(inherited.creationSource).toBe('inherited');
      expect(inherited.canRevert).toBe(true);

      const pattern = createPatternState(true, false, 'ordered-list', mockOrderedListPlugin);
      expect(pattern.creationSource).toBe('pattern');
      expect(pattern.canRevert).toBe(true);
    });
  });
});

describe('Pattern Lifecycle Scenarios', () => {
  describe('Header pattern workflow', () => {
    it('should handle: user types "# " → header → deletes to "#" → text', () => {
      // 1. Start as user-created text node
      const state = new PatternState('user');
      expect(state.shouldDetectPatterns).toBe(true);

      // 2. User types "# " - pattern detected
      state.recordPluginPatternMatch(mockHeaderPlugin);
      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(true);

      // 3. User continues typing - no reversion yet
      expect(state.attemptRevert('# Hello World')).toBe(false);

      // 4. User deletes space, leaving just "#" - revert pattern matches
      const shouldRevert = state.attemptRevert('#');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
      expect(state.shouldDetectPatterns).toBe(true);
    });
  });

  describe('Inherited header workflow', () => {
    it('should revert when Enter creates new header and syntax is deleted', () => {
      // User presses Enter on a header - new node inherits header type
      const state = new PatternState('inherited', mockHeaderPlugin);
      expect(state.canRevert).toBe(true);

      // When user deletes to just "#", node reverts to text
      const shouldRevert = state.attemptRevert('#');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Inherited task workflow', () => {
    it('should NOT revert inherited task nodes (canRevert: false)', () => {
      // User presses Enter on a task - new node inherits task type
      const state = new PatternState('inherited', mockTaskPlugin);
      expect(state.canRevert).toBe(false);

      // Task nodes cannot revert regardless of content
      const shouldRevert = state.attemptRevert('');
      expect(shouldRevert).toBe(false);
      expect(state.creationSource).toBe('inherited');
    });
  });

  describe('Page load workflow', () => {
    it('should enable reversion for loaded nodes matching their pattern', () => {
      // Node loaded from DB: header with "# Title" content
      const state = new PatternState('user');

      // Initialize sets plugin pattern exists (content matches type's pattern)
      state.setPluginPatternExists(mockHeaderPlugin);

      expect(state.creationSource).toBe('pattern');
      expect(state.canRevert).toBe(true);

      // User can now delete to "#" to revert to text
      const shouldRevert = state.attemptRevert('#');
      expect(shouldRevert).toBe(true);
    });
  });

  describe('Quote block workflow', () => {
    it('should handle quote block creation and reversion', () => {
      const state = new PatternState('user');

      state.recordPluginPatternMatch(mockQuotePlugin);
      expect(state.creationSource).toBe('pattern');

      // Delete to just ">" - matches revert pattern
      const shouldRevert = state.attemptRevert('>');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });

  describe('Ordered list workflow', () => {
    it('should handle ordered list creation and reversion', () => {
      const state = new PatternState('user');

      state.recordPluginPatternMatch(mockOrderedListPlugin);
      expect(state.creationSource).toBe('pattern');

      // Delete to just "1." - matches revert pattern
      const shouldRevert = state.attemptRevert('1.');
      expect(shouldRevert).toBe(true);
      expect(state.creationSource).toBe('user');
    });
  });
});
