/**
 * Shared enum-field helpers: merging a schema field's coreValues + userValues
 * into one selectable list, and resolving a stored value to its display label.
 *
 * Single source of truth for "what are field X's options" and "what label
 * does value Y map to" — every property-form surface that needs to humanize
 * a raw enum value (a field control, a collapsed-header summary badge, …)
 * reads it from here instead of re-deriving its own copy of the same lookup.
 */
import type { EnumValue } from '$lib/types/schema-node';

/**
 * The minimum a field must carry to be treated as an enum here: a declared
 * value set, optionally extended by user values.
 *
 * Widened from `SchemaField` so an edge field can use the same lookup. An edge
 * enum is a closed vocabulary — it declares `coreValues` and has no
 * `userValues`/`extensible` half — but "merge what's declared, resolve a value
 * to its label" is identical work, and duplicating it is how two surfaces
 * drift apart on how the same value renders.
 */
export interface EnumFieldLike {
  coreValues?: EnumValue[] | null;
  userValues?: EnumValue[] | null;
}

/**
 * Merge a field's protected core values with its user-extensible ones, in that
 * order. A userValue whose `value` collides with an existing coreValue is
 * dropped (core wins, its label is kept) rather than appended as a second,
 * duplicate option — an extension is meant to ADD choices, not shadow one
 * that already exists.
 */
export function getEnumValues(field: EnumFieldLike): EnumValue[] {
  const values: EnumValue[] = [];
  const seen = new Set<string>();
  for (const ev of field.coreValues ?? []) {
    if (seen.has(ev.value)) continue;
    values.push(ev);
    seen.add(ev.value);
  }
  for (const ev of field.userValues ?? []) {
    if (seen.has(ev.value)) continue;
    values.push(ev);
    seen.add(ev.value);
  }
  return values;
}

/**
 * Humanize a raw value the schema doesn't (or no longer) declare a label for,
 * e.g. "in_progress" -> "In Progress". Used as the fallback when a stored
 * value isn't found among a field's current enum options.
 */
export function formatEnumFallbackLabel(value: string): string {
  return value
    .replace(/[_-]/g, ' ')
    .split(' ')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
    .join(' ');
}

/**
 * Resolve a stored enum value to its display label: the schema's declared
 * label when the value is still a current option, else a humanized fallback
 * of the raw value — never the raw key verbatim — so every surface that
 * displays this field (the control itself, a collapsed-header summary, …)
 * agrees on how a value renders. Returns undefined for an absent value.
 *
 * A falsy declared label (e.g. a malformed `label: ''` on an enum option)
 * is treated the same as "no label found" — falls through to the humanized
 * fallback — rather than surfacing a blank. Deliberately `||`, not `??`: an
 * empty string is exactly as unhelpful as a missing label.
 */
export function enumValueLabel(
  field: EnumFieldLike,
  value: string | null | undefined
): string | undefined {
  if (!value) return undefined;
  return getEnumValues(field).find((ev) => ev.value === value)?.label || formatEnumFallbackLabel(value);
}

/**
 * What a stored enum value reads as on `scopeField`, the field as a type the
 * node's own type extends declares it.
 *
 * A subtype may add values to a field it inherits, each naming the existing
 * value it maps to (`mapsTo`): an issue's `changes_requested` maps to a
 * task's `in_progress`. A view of the base type (a board of tasks) shows the
 * node under the value its own maps to. `ownField` is the field as the
 * node's own type declares it, which is where the `mapsTo` declarations are.
 *
 * Returns the value itself when the scope declares it, the value it maps to
 * otherwise, and `null` when it maps to nothing the scope declares.
 *
 * Mirrors `resolve_value_at_scope` in
 * `packages/core/src/schema/extends_chain.rs`, which the backend's queries
 * and rules read through; keep the two in step.
 */
export function enumValueAtScope(
  scopeField: EnumFieldLike,
  value: string,
  ownField: EnumFieldLike | null | undefined
): string | null {
  const declared = new Set(getEnumValues(scopeField).map((ev) => ev.value));
  if (declared.has(value)) return value;
  if (!ownField) return null;

  const own = new Map(getEnumValues(ownField).map((ev) => [ev.value, ev]));
  const seen = new Set<string>([value]);
  let current = value;
  // Each hop reaches a value declared further up; a malformed `mapsTo` cycle
  // ends on the visited set.
  for (;;) {
    const next = own.get(current)?.mapsTo;
    if (!next || seen.has(next)) return null;
    if (declared.has(next)) return next;
    seen.add(next);
    current = next;
  }
}
