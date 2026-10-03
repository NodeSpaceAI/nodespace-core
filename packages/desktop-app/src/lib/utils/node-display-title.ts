/**
 * The single rule for choosing between a node's `title` and `content` as its current
 * display value.
 *
 * `sharedNodeStore`'s cached `title` is refreshed only by a backend round-trip. Optimistic
 * content edits patch `content` and leave `title` untouched, so `title` is stale for any
 * node whose schema doesn't compute it — e.g. a title captured mid-slash-command, before a
 * node's type conversion, or simply the last-synced value for a node whose content has since
 * changed. `node.title || node.content` (the pattern this replaces) surfaces that stale value
 * on every surface using it.
 *
 * The correct rule: prefer `title` only when the node's schema is genuinely title_template-
 * driven — there `title` is computed from properties and is a legitimately different value
 * from `content`, so it's the only valid display value (an unresolved template renders empty,
 * not a fallback to content). Every other type has `content` as its source of truth.
 *
 * Callers determine `hasTitleTemplate` themselves, since the answer comes from different
 * places depending on context (a `SchemaFormLoader` instance inside a node viewer;
 * `pluginRegistry.hasTitleTemplate(nodeType)` outside one). Both cover a core type with a
 * hardcoded form, such as `person`, through its plugin's `hasTitleTemplate`, and every other
 * type through its schema's `titleTemplate` — see `schema-form-loader.svelte.ts`'s
 * `hasTitleTemplate` getter.
 */
/**
 * Matches any Unicode letter or number. Used (not the ASCII-only `\w`) so a fully-resolved
 * title in a non-Latin script (e.g. a `{first_name} {last_name}` template evaluating to a
 * CJK or Arabic name) is correctly recognized as resolved, not misclassified as empty
 * alongside a genuinely-unresolved separator-only value like `" — "` — see the guard below.
 */
export const HAS_RESOLVED_CHARACTER_RE = /[\p{L}\p{N}]/u;

export function resolveTitleOrContent(
  node: { title?: string | null; content?: string | null } | null | undefined,
  hasTitleTemplate: boolean
): string {
  if (hasTitleTemplate) {
    const title = node?.title ?? '';
    // A title with no letters or numbers in any script (e.g. the `" "` left by a
    // `"{first} {last}"` template with both fields empty, or `" — "` left by a template
    // whose only literal text is a separator) counts as unresolved.
    return HAS_RESOLVED_CHARACTER_RE.test(title) ? title : '';
  }
  return node?.content ?? '';
}
