// Generated from `packages/nodespace-types` by `bun run gen:types`. Do not edit.

export type EnumValue = {
  value: string;
  label: string;
  /**
   * For a value added to a field this schema *inherited* via `extends`:
   * which pre-existing value it collapses to at an ancestor's scope
   * (ADR-078).
   *
   * An `issue` extending `task` may add `backlog` to the inherited
   * `status`, mapping to `todo`. A consumer reading at `task`'s scope — a
   * Play condition, a query filter, any CEL expression written against the
   * base type — then sees `todo`, the value it was written to understand,
   * rather than a `backlog` it has never heard of. Jira's status-category
   * model is the direct precedent.
   *
   * Required on every value appended to an inherited field, and neither
   * required nor meaningful on a field the schema declares itself: an own
   * field has no ancestor scope whose meaning needs preserving.
   */
  mapsTo?: string;
};
