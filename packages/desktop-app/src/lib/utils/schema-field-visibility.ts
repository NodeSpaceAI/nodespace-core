/**
 * Single shared predicate for "should this schema field be shown to a user?".
 *
 * `protection: 'system'` marks a field the backend owns end to end — an
 * ai-chat session bookkeeping field like `capture:transcript`. Nothing a user
 * types, and on a local-only install several of them are empty by
 * construction. They must not render as an editable control, and equally must
 * not render as a table column that can never be filled.
 *
 * Every UI surface that decides whether to *render* a field should filter
 * through this, so the detail form and the table view cannot drift apart on
 * what "visible" means. Surfaces that use `fields` for something other than
 * rendering — offering query filter properties, say — make their own call.
 */
import type { SchemaField } from '$lib/types/schema-node';

export function isUserVisibleField(field: SchemaField): boolean {
  return field.protection !== 'system';
}
