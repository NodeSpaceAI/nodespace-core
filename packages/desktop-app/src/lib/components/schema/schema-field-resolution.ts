/**
 * Field-value resolution for the generic, schema-driven properties form.
 *
 * Storage namespaces a node's fields under its type (`properties.project.status`), but
 * every transport flattens that bucket away before a node reaches the frontend, so the
 * frontend only ever sees — and writes — bare keys (`properties.status`). The backend
 * moves bare keys back into the type's bucket on write.
 *
 * Extracted from generic-schema-form.svelte so both are unit-testable without rendering
 * the component.
 */

export interface FieldValueSource {
  nodeType: string;
  properties?: Record<string, unknown>;
}

/**
 * Read a schema field's value.
 *
 * @returns the stored value, or `null` when the field is unset
 */
export function resolveFieldValue(node: FieldValueSource, fieldName: string): unknown {
  return node.properties?.[fieldName] ?? null;
}

/**
 * Build the `properties` payload that writes `fieldName = value`.
 */
export function buildFieldWrite(
  node: FieldValueSource,
  fieldName: string,
  value: unknown
): Record<string, unknown> {
  return { ...(node.properties ?? {}), [fieldName]: value };
}
