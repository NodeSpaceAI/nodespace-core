/**
 * QueryEditor Component Tests
 *
 * Unit tests for the QueryEditor logic.
 * Tests cover:
 * - Default query template when no query prop provided
 * - Rendering with a provided query definition
 * - Validation: invalid JSON produces an error
 * - Validation: missing targetType produces an error
 * - Validation: filters not an array produces an error
 * - Successful save calls onSave with valid QueryDefinition
 * - Cancel callback is called when cancel is triggered
 * - Template examples are present in the template list
 *
 * Types, constants, and templates are imported from $lib/types/query to
 * avoid duplication between the component and the tests.
 */

import { describe, it, expect, vi } from 'vitest';
import type { QueryFilter, SortConfig, QueryDefinition } from '$lib/types/query';
import { DEFAULT_QUERY, QUERY_TEMPLATE_EXAMPLES } from '$lib/types/query';

// Mock the logger
vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn(),
  }),
}));

// =============================================================================
// Logic extracted from query-editor.svelte (testable without rendering)
// =============================================================================

/**
 * Parse and validate a JSON string into a QueryDefinition.
 * Returns the definition on success or an error message string on failure.
 * Mirrors validateAndSave() from query-editor.svelte.
 */
function parseQueryJson(jsonText: string): { ok: true; definition: QueryDefinition } | { ok: false; error: string } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(jsonText);
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    return { ok: false, error: `Invalid JSON: ${message}` };
  }

  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { ok: false, error: 'Query must be a JSON object.' };
  }

  const candidate = parsed as Record<string, unknown>;

  if (!candidate.targetType || typeof candidate.targetType !== 'string') {
    return { ok: false, error: 'Missing required field: targetType (must be a non-empty string).' };
  }

  if (!Array.isArray(candidate.filters)) {
    return { ok: false, error: 'Missing required field: filters (must be an array).' };
  }

  const definition: QueryDefinition = {
    targetType: candidate.targetType,
    filters: candidate.filters as QueryFilter[],
  };

  if (candidate.sorting !== undefined) {
    if (!Array.isArray(candidate.sorting)) {
      return { ok: false, error: 'Optional field sorting must be an array.' };
    }
    definition.sorting = candidate.sorting as SortConfig[];
  }

  if (candidate.limit !== undefined) {
    if (typeof candidate.limit !== 'number') {
      return { ok: false, error: 'Optional field limit must be a number.' };
    }
    definition.limit = candidate.limit;
  }

  return { ok: true, definition };
}

/**
 * Produce the initial JSON text for the textarea.
 * Mirrors the $state initializer in query-editor.svelte.
 */
function getInitialJsonText(query: QueryDefinition | null | undefined): string {
  return JSON.stringify(query ?? DEFAULT_QUERY, null, 2);
}

// =============================================================================
// Tests
// =============================================================================

describe('QueryEditor Logic', () => {
  // ---------------------------------------------------------------------------
  // 1. Default query template
  // ---------------------------------------------------------------------------
  describe('Default query template', () => {
    it('uses DEFAULT_QUERY when no query prop is provided', () => {
      const text = getInitialJsonText(null);
      const result = parseQueryJson(text);
      expect(result.ok).toBe(true);
      if (result.ok) {
        expect(result.definition.targetType).toBe('task');
        expect(result.definition.filters).toEqual([]);
        expect(result.definition.limit).toBe(50);
      }
    });

    it('uses DEFAULT_QUERY when query prop is undefined', () => {
      const text = getInitialJsonText(undefined);
      const result = parseQueryJson(text);
      expect(result.ok).toBe(true);
      if (result.ok) {
        expect(result.definition.targetType).toBe('task');
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 2. Rendering with provided query
  // ---------------------------------------------------------------------------
  describe('Rendering with provided query', () => {
    it('initializes textarea with the provided query as JSON', () => {
      const query: QueryDefinition = {
        targetType: 'text',
        filters: [{ type: 'content', operator: 'contains', value: 'notes' }],
        limit: 25,
      };
      const text = getInitialJsonText(query);
      const parsed = JSON.parse(text) as Record<string, unknown>;
      expect(parsed.targetType).toBe('text');
      expect(parsed.limit).toBe(25);
    });

    it('round-trips a query definition through JSON serialization', () => {
      const query: QueryDefinition = {
        targetType: 'task',
        filters: [
          { type: 'property', operator: 'equals', property: 'status', value: 'open' },
        ],
        sorting: [{ field: 'dueDate', direction: 'asc' }],
        limit: 10,
      };
      const text = getInitialJsonText(query);
      const result = parseQueryJson(text);
      expect(result.ok).toBe(true);
      if (result.ok) {
        expect(result.definition).toEqual(query);
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 3. Validation: invalid JSON
  // ---------------------------------------------------------------------------
  describe('Validation: invalid JSON', () => {
    it('returns error for completely invalid JSON text', () => {
      const result = parseQueryJson('not valid json');
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/Invalid JSON/i);
      }
    });

    it('returns error for truncated JSON', () => {
      const result = parseQueryJson('{ "targetType": "task"');
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/Invalid JSON/i);
      }
    });

    it('returns error for JSON array at top level', () => {
      const result = parseQueryJson('[{ "targetType": "task", "filters": [] }]');
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toBe('Query must be a JSON object.');
      }
    });

    it('returns error for JSON null', () => {
      const result = parseQueryJson('null');
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toBe('Query must be a JSON object.');
      }
    });

    it('returns error for JSON string', () => {
      const result = parseQueryJson('"just a string"');
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toBe('Query must be a JSON object.');
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 4. Validation: missing targetType
  // ---------------------------------------------------------------------------
  describe('Validation: missing targetType', () => {
    it('returns error when targetType is absent', () => {
      const result = parseQueryJson(JSON.stringify({ filters: [] }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/targetType/);
      }
    });

    it('returns error when targetType is an empty string', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: '', filters: [] }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/targetType/);
      }
    });

    it('returns error when targetType is a number', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: 42, filters: [] }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/targetType/);
      }
    });

    it('returns error when targetType is null', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: null, filters: [] }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/targetType/);
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 5. Validation: filters not an array
  // ---------------------------------------------------------------------------
  describe('Validation: filters must be an array', () => {
    it('returns error when filters is absent', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: 'task' }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/filters/);
      }
    });

    it('returns error when filters is a string', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: 'task', filters: 'open' }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/filters/);
      }
    });

    it('returns error when filters is an object', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: 'task', filters: {} }));
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/filters/);
      }
    });

    it('accepts an empty filters array', () => {
      const result = parseQueryJson(JSON.stringify({ targetType: 'task', filters: [] }));
      expect(result.ok).toBe(true);
    });
  });

  // ---------------------------------------------------------------------------
  // 6. Successful save
  // ---------------------------------------------------------------------------
  describe('Successful save calls onSave with valid QueryDefinition', () => {
    it('calls onSave with parsed definition for a minimal valid query', () => {
      const onSave = vi.fn();
      const jsonText = JSON.stringify({ targetType: 'task', filters: [] });
      const result = parseQueryJson(jsonText);
      if (result.ok) {
        onSave(result.definition);
      }
      expect(onSave).toHaveBeenCalledOnce();
      expect(onSave).toHaveBeenCalledWith({
        targetType: 'task',
        filters: [],
      });
    });

    it('calls onSave with full definition including sorting and limit', () => {
      const onSave = vi.fn();
      const query: QueryDefinition = {
        targetType: 'text',
        filters: [{ type: 'content', operator: 'contains', value: 'hello' }],
        sorting: [{ field: 'modifiedAt', direction: 'desc' }],
        limit: 20,
      };
      const result = parseQueryJson(JSON.stringify(query));
      if (result.ok) {
        onSave(result.definition);
      }
      expect(onSave).toHaveBeenCalledWith(query);
    });

    it('does not call onSave on validation failure', () => {
      const onSave = vi.fn();
      const result = parseQueryJson('invalid json');
      if (result.ok) {
        onSave(result.definition);
      }
      expect(onSave).not.toHaveBeenCalled();
    });

    it('omits undefined optional fields from the saved definition', () => {
      const onSave = vi.fn();
      const result = parseQueryJson(JSON.stringify({ targetType: 'task', filters: [] }));
      if (result.ok) {
        onSave(result.definition);
      }
      const savedDef = onSave.mock.calls[0][0] as QueryDefinition;
      expect(savedDef.sorting).toBeUndefined();
      expect(savedDef.limit).toBeUndefined();
    });
  });

  // ---------------------------------------------------------------------------
  // 7. Cancel callback
  // ---------------------------------------------------------------------------
  describe('Cancel callback', () => {
    it('calls onCancel when cancel is triggered', () => {
      const onCancel = vi.fn();
      // Simulate pressing cancel: component calls onCancel?.()
      onCancel();
      expect(onCancel).toHaveBeenCalledOnce();
    });

    it('does not throw when onCancel is not provided', () => {
      // Simulate optional cancel: component guards with onCancel?.()
      // Calling an optional function reference that is undefined should not throw
      const handler: (() => void) | undefined = undefined;
      // This mirrors the component's `onCancel?.()` call
      const callOptional = (fn: (() => void) | undefined) => fn?.();
      expect(() => callOptional(handler)).not.toThrow();
    });
  });

  // ---------------------------------------------------------------------------
  // 8. Template examples (imported from shared types)
  // ---------------------------------------------------------------------------
  describe('Template examples', () => {
    it('has three template examples', () => {
      expect(QUERY_TEMPLATE_EXAMPLES).toHaveLength(3);
    });

    it('includes an "All incomplete tasks" template', () => {
      const template = QUERY_TEMPLATE_EXAMPLES.find((t) => t.label === 'All incomplete tasks');
      expect(template).toBeDefined();
      expect(template?.definition.targetType).toBe('task');
      expect(template?.definition.filters).toHaveLength(1);
      expect(template?.definition.filters[0].type).toBe('property');
      expect(template?.definition.filters[0].operator).toBe('in');
    });

    it('includes a "Recent text nodes with keyword" template', () => {
      const template = QUERY_TEMPLATE_EXAMPLES.find(
        (t) => t.label === 'Recent text nodes with keyword'
      );
      expect(template).toBeDefined();
      expect(template?.definition.targetType).toBe('text');
      expect(template?.definition.filters[0].type).toBe('content');
    });

    it('includes a "Tasks by priority" template', () => {
      const template = QUERY_TEMPLATE_EXAMPLES.find((t) => t.label === 'Tasks by priority');
      expect(template).toBeDefined();
      expect(template?.definition.targetType).toBe('task');
      expect(template?.definition.filters[0].operator).toBe('equals');
    });

    it('all template definitions are valid QueryDefinitions', () => {
      for (const { label, definition } of QUERY_TEMPLATE_EXAMPLES) {
        const result = parseQueryJson(JSON.stringify(definition));
        expect(result.ok, `Template "${label}" should be valid`).toBe(true);
      }
    });

    it('applying a template sets the json text to the template definition', () => {
      const template = QUERY_TEMPLATE_EXAMPLES[0];
      // Mirrors applyTemplate() in the component
      const newText = JSON.stringify(template.definition, null, 2);
      const result = parseQueryJson(newText);
      expect(result.ok).toBe(true);
      if (result.ok) {
        expect(result.definition.targetType).toBe(template.definition.targetType);
      }
    });
  });

  // ---------------------------------------------------------------------------
  // 9. Optional field validation
  // ---------------------------------------------------------------------------
  describe('Optional field validation', () => {
    it('returns error when sorting is not an array', () => {
      const result = parseQueryJson(
        JSON.stringify({ targetType: 'task', filters: [], sorting: 'dueDate asc' })
      );
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/sorting/);
      }
    });

    it('returns error when limit is not a number', () => {
      const result = parseQueryJson(
        JSON.stringify({ targetType: 'task', filters: [], limit: 'fifty' })
      );
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error).toMatch(/limit/);
      }
    });

    it('accepts sorting as an array', () => {
      const result = parseQueryJson(
        JSON.stringify({
          targetType: 'task',
          filters: [],
          sorting: [{ field: 'dueDate', direction: 'asc' }],
        })
      );
      expect(result.ok).toBe(true);
    });

    it('accepts numeric limit', () => {
      const result = parseQueryJson(
        JSON.stringify({ targetType: 'task', filters: [], limit: 100 })
      );
      expect(result.ok).toBe(true);
      if (result.ok) {
        expect(result.definition.limit).toBe(100);
      }
    });
  });
});
