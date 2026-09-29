/**
 * Real parse-path coverage for the view-mode renderer.
 *
 * Exercises the ACTUAL parser (`parseContent` + its token walk), not a simplified
 * re-implementation — so a routing regression (e.g. a bare `[[id]]` no longer
 * becoming a `noderef`, or not surviving emphasis) can't ship green.
 */
import { describe, it, expect } from 'vitest';
import { parseContent, flattenToText, type ViewNode } from '$lib/design/components/view-mode-parser';

/** Recursively collect noderef ids, code spans, and concatenated text so tests
 *  assert on outcomes without pinning the exact block/inline nesting. */
function collect(nodes: ViewNode[]): { refs: string[]; text: string; code: string[]; kinds: Set<string> } {
  const out = { refs: [] as string[], text: '', code: [] as string[], kinds: new Set<string>() };
  const walk = (ns: ViewNode[]) => {
    for (const n of ns) {
      out.kinds.add(n.type);
      if (n.type === 'noderef') out.refs.push(n.id);
      else if (n.type === 'text') out.text += n.content;
      else if (n.type === 'code') out.code.push(n.content);
      else if ('children' in n && Array.isArray(n.children)) walk(n.children);
      else if (n.type === 'list') n.items.forEach(walk);
    }
  };
  walk(nodes);
  return out;
}

// Only UUID-shaped (or date) ids route to a noderef — mirrors the backend's
// is_valid_node_id, so `[[some title]]` stays literal text.
const ID_A = 'aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa';
const ID_B = 'bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb';

describe('view-mode-parser parseContent (real path)', () => {
  it('renders plain prose as text', () => {
    const r = collect(parseContent('hello world', true, false));
    expect(r.text).toContain('hello world');
    expect(r.refs).toEqual([]);
  });

  it('routes a bare UUID [[node-id]] to a noderef', () => {
    const r = collect(parseContent(`[[${ID_A}]]`, true, false));
    expect(r.refs).toEqual([ID_A]);
  });

  it('leaves a non-id [[title]] as literal text (matches backend is_valid_node_id)', () => {
    const r = collect(parseContent('[[some title]]', true, false));
    expect(r.refs).toEqual([]);
    expect(r.text).toContain('[[some title]]');
  });

  it('splits surrounding text from an inline noderef', () => {
    const r = collect(parseContent(`see [[${ID_A}]] now`, true, false));
    expect(r.refs).toEqual([ID_A]);
    expect(r.text).toContain('see ');
    expect(r.text).toContain(' now');
  });

  it('preserves a noderef inside bold emphasis (survives markdown)', () => {
    const r = collect(parseContent(`**bold [[${ID_A}]] text**`, true, false));
    expect(r.refs).toEqual([ID_A]);
    expect(r.kinds.has('bold')).toBe(true);
  });

  it('parses italic and inline code', () => {
    const italic = collect(parseContent('*emphasis*', true, false));
    expect(italic.kinds.has('italic')).toBe(true);
    const code = collect(parseContent('`literal`', true, false));
    expect(code.code).toContain('literal');
  });

  it('still routes a noderef when markdown is disabled', () => {
    const r = collect(parseContent(`[[${ID_A}]]`, false, false));
    expect(r.refs).toEqual([ID_A]);
  });

  it('resolves multiple distinct noderefs in order', () => {
    const r = collect(parseContent(`[[${ID_A}]] and [[${ID_B}]]`, true, false));
    expect(r.refs).toEqual([ID_A, ID_B]);
  });

  it('does NOT linkify a [[node-id]] inside a fenced code block', () => {
    const r = collect(parseContent(`\`\`\`\nsee [[${ID_A}]]\n\`\`\``, true, false));
    expect(r.refs).toEqual([]); // fenced content is literal
    expect(r.text).toContain(`[[${ID_A}]]`);
  });

  it('renders a fenced block with an interior blank line without leaking the sentinel', () => {
    const r = collect(parseContent('```\nline1\n\nline2\n```', true, false));
    expect(r.text).toContain('line1');
    expect(r.text).toContain('line2');
    expect(r.text).not.toContain('BLANK'); // the pre-lex blank-line sentinel is restored, not leaked
  });
});

describe('flattenToText', () => {
  it('reduces block children to inline text/br runs while keeping noderefs', () => {
    const flat = flattenToText(parseContent(`para with [[${ID_A}]]`, true, false));
    const r = collect(flat);
    expect(r.refs).toEqual([ID_A]);
    // No block wrappers survive flattening.
    expect(r.kinds.has('paragraph')).toBe(false);
    expect(r.kinds.has('heading')).toBe(false);
    expect(r.kinds.has('list')).toBe(false);
  });
});
