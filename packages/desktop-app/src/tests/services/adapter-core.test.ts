/**
 * Tests for the ExecuteQuery wire encoding in adapter-core.ts.
 *
 * A saved query's filters and ordering are executed by the backend's
 * QueryService — the frontend no longer re-implements them. This encoding is
 * what carries the definition there. Filters and sorting are sent as written:
 * their keys and field names have one spelling, the stored snake_case one, so
 * nothing is respelled here.
 *
 * Pure functions, tested directly (no adapter/transport involved).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { QueryFilter, SortConfig } from '$lib/types/query';
import { buildExecuteQueryWire, MAX_QUERY_ROWS } from '$lib/services/adapter-core';

describe('MAX_QUERY_ROWS', () => {
  it('matches the row ceiling the daemon actually clamps to', () => {
    // This constant exists so callers can tell a truncated result from a
    // complete one — the daemon clamps silently, so the only signal is whether
    // the row count reached the ceiling. That makes it worthless if it drifts
    // from the daemon's value: too high and the comparison never fires (a
    // truncated set is reported as complete), too low and every result looks
    // truncated.
    //
    // Read the Rust source rather than restate the number, so a change there
    // fails here instead of silently disabling the truncation caveat.
    // __dirname-relative, not cwd-relative, so this resolves the same whether
    // vitest runs from the repo root or from packages/desktop-app:
    // src/tests/services → src/tests → src → desktop-app → packages.
    const source = readFileSync(
      resolve(__dirname, '../../../../daemon/src/services/node_service.rs'),
      'utf8'
    );
    const match = source.match(/pub const MAX_ROW_LIMIT:\s*usize\s*=\s*(\d+)/);

    // A miss means the constant was renamed or moved — fail loudly rather than
    // silently skipping the only check that keeps the two sides in step.
    expect(match, 'MAX_ROW_LIMIT not found in node_service.rs').not.toBeNull();
    expect(Number(match?.[1])).toBe(MAX_QUERY_ROWS);
  });
});

describe('buildExecuteQueryWire', () => {
  it('carries sorting to the backend rather than dropping it', () => {
    // The whole point of routing through QueryService: a sort the frontend used
    // to apply itself now has to survive the wire crossing.
    const sorting: SortConfig[] = [{ field: 'priority', direction: 'desc' }];
    const wire = buildExecuteQueryWire({ targetType: 'task', sorting });

    expect(wire.sortingJson).not.toBeNull();
    expect(JSON.parse(wire.sortingJson as string)).toEqual([
      { field: 'priority', direction: 'desc' }
    ]);
  });

  it('sends a multi-key sort in order, field names as written', () => {
    const sorting: SortConfig[] = [
      { field: 'priority', direction: 'asc' },
      { field: 'modified_at', direction: 'desc' },
      { field: 'due_date', direction: 'asc' }
    ];
    const wire = buildExecuteQueryWire({ targetType: 'task', sorting });

    expect(JSON.parse(wire.sortingJson as string)).toEqual(sorting);
  });

  it('sends no sorting for an absent or empty sort config', () => {
    // Distinct from sorting by nothing: the proto field is optional and the ops
    // layer reads an absent one as Option::None (unsorted).
    expect(buildExecuteQueryWire({ targetType: 'task' }).sortingJson).toBeNull();
    expect(buildExecuteQueryWire({ targetType: 'task', sorting: [] }).sortingJson).toBeNull();
  });

  it('sends filters as written, nested filters and paths included', () => {
    // A filter's keys are the ones the ops layer deserializes, so nothing is
    // respelled on the way out.
    const filters: QueryFilter[] = [
      {
        type: 'property',
        operator: 'equals',
        property: 'status',
        value: 'open',
        case_sensitive: true
      },
      {
        type: 'relationship',
        operator: 'exists',
        path: ['mentioned_by'],
        node_id: 'n1'
      },
      {
        type: 'related',
        operator: 'exists',
        path: [{ name: 'child_of', open_ended: true }, 'project'],
        filter: {
          type: 'property',
          operator: 'equals',
          property: 'status',
          value: 'active',
          case_sensitive: false
        }
      }
    ];
    const wire = buildExecuteQueryWire({ targetType: 'task', filters });

    expect(JSON.parse(wire.filtersJson)).toEqual(filters);
  });

  it('preserves a filter value that is an array', () => {
    // `in` carries a list; JSON round-trips it into serde_json::Value.
    const wire = buildExecuteQueryWire({
      targetType: 'task',
      filters: [
        { type: 'property', operator: 'in', property: 'status', value: ['open', 'in_progress'] }
      ]
    });

    expect(JSON.parse(wire.filtersJson)[0].value).toEqual(['open', 'in_progress']);
  });

  it('encodes an empty filter list as "[]"', () => {
    expect(buildExecuteQueryWire({ targetType: 'task' }).filtersJson).toBe('[]');
  });

  it('uses 0 as the unset limit sentinel', () => {
    // Matches the proto: 0 means "server default", not "return nothing".
    expect(buildExecuteQueryWire({ targetType: 'task' }).limit).toBe(0);
    expect(buildExecuteQueryWire({ targetType: 'task', limit: 25 }).limit).toBe(25);
  });

  it('passes the target type through, including the wildcard', () => {
    expect(buildExecuteQueryWire({ targetType: '*' }).targetType).toBe('*');
  });
});
