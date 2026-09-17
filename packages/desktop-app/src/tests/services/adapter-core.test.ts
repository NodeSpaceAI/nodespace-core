/**
 * Tests for the ExecuteQuery wire encoding in adapter-core.ts.
 *
 * A saved query's filters and ordering are executed by the backend's
 * QueryService — the frontend no longer re-implements them. This encoding is
 * what carries the definition there, so it is the seam where a saved query
 * either reaches the right SQL or silently degrades: a sort field spelled the
 * way the backend does not recognize becomes a NULL json_extract that orders
 * every row equally, which looks like "sorting did nothing" rather than an
 * error.
 *
 * Pure functions, tested directly (no adapter/transport involved).
 */

import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { QueryFilter, SortConfig } from '$lib/types/query';
import { buildExecuteQueryWire, encodeSortField, MAX_QUERY_ROWS } from '$lib/services/adapter-core';

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

describe('encodeSortField', () => {
  it('renames the metadata fields to the columns resolve_field matches', () => {
    // QueryService::resolve_field treats exactly these five as top-level SQL
    // columns, spelled snake_case. A stored definition spells them camelCase.
    expect(encodeSortField('createdAt')).toBe('created_at');
    expect(encodeSortField('modifiedAt')).toBe('modified_at');
    expect(encodeSortField('nodeType')).toBe('node_type');
    expect(encodeSortField('content')).toBe('content');
    expect(encodeSortField('title')).toBe('title');
  });

  it('leaves property names untouched', () => {
    // Property fields are stored as authored and resolve to
    // json_extract(properties, '$.<type>.<field>') under that exact spelling,
    // so renaming them would break the lookup rather than fix it.
    expect(encodeSortField('dueDate')).toBe('dueDate');
    expect(encodeSortField('priority')).toBe('priority');
    expect(encodeSortField('custom:severity')).toBe('custom:severity');
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

  it('encodes a multi-key sort in order, converting metadata field names', () => {
    const wire = buildExecuteQueryWire({
      targetType: 'task',
      sorting: [
        { field: 'priority', direction: 'asc' },
        { field: 'modifiedAt', direction: 'desc' }
      ]
    });

    expect(JSON.parse(wire.sortingJson as string)).toEqual([
      { field: 'priority', direction: 'asc' },
      { field: 'modified_at', direction: 'desc' }
    ]);
  });

  it('sends no sorting for an absent or empty sort config', () => {
    // Distinct from sorting by nothing: the proto field is optional and the ops
    // layer reads an absent one as Option::None (unsorted).
    expect(buildExecuteQueryWire({ targetType: 'task' }).sortingJson).toBeNull();
    expect(buildExecuteQueryWire({ targetType: 'task', sorting: [] }).sortingJson).toBeNull();
  });

  it('converts filter keys to the snake_case the ops layer deserializes', () => {
    // AgentFilterItem is deny_unknown_fields, so a camelCase key is a hard
    // rejection, not a silently ignored one.
    const filters: QueryFilter[] = [
      {
        type: 'property',
        operator: 'equals',
        property: 'status',
        value: 'open',
        caseSensitive: true
      },
      {
        type: 'relationship',
        operator: 'exists',
        relationshipType: 'mentioned_by',
        nodeId: 'n1'
      }
    ];
    const wire = buildExecuteQueryWire({ targetType: 'task', filters });

    expect(JSON.parse(wire.filtersJson)).toEqual([
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
        relationship_type: 'mentioned_by',
        node_id: 'n1'
      }
    ]);
  });

  it('omits absent optional filter keys instead of sending null', () => {
    // Option<T> deserializes from a missing key, not from an explicit null.
    const wire = buildExecuteQueryWire({
      targetType: 'task',
      filters: [{ type: 'content', operator: 'contains', value: 'acme' }]
    });

    const [encoded] = JSON.parse(wire.filtersJson);
    expect(encoded).toEqual({ type: 'content', operator: 'contains', value: 'acme' });
    expect('property' in encoded).toBe(false);
    expect('case_sensitive' in encoded).toBe(false);
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
