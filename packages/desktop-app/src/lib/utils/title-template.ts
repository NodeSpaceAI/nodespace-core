/**
 * Title template interpolation utility.
 *
 * Mirrors the behaviour of Rust's `interpolate_title_template` in
 * `packages/core/src/utils/markdown.rs`: replaces `{fieldName}` tokens,
 * then trims and collapses internal whitespace so the output is consistent
 * with what the backend will compute and return as `node.title`.
 *
 * Token syntax: `{fieldName}` where fieldName matches `\w+` (word characters
 * only: [a-zA-Z0-9_]). Hyphenated names like `{first-name}` are NOT supported.
 */

/**
 * Interpolates a title template with the given field values.
 *
 * @param template - Template string with `{fieldName}` tokens
 * @param fieldValues - Flat map of field names to their current values
 * @returns Interpolated title with whitespace normalised and trimmed.
 *          Returns an empty string when all tokens resolve to empty values.
 */
export function evaluateTitleTemplate(
  template: string,
  fieldValues: Record<string, unknown>
): string {
  const interpolated = template.replace(/\{(\w+)\}/g, (_, fieldName) => {
    const val = fieldValues[fieldName];
    if (val === null || val === undefined) return '';
    return String(val);
  });
  // Normalize whitespace and trim (mirrors Rust WHITESPACE_RE + .trim())
  return interpolated.replace(/\s+/g, ' ').trim();
}
