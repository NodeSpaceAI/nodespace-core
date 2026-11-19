/**
 * Shift+Enter in a quote block inserts a newline followed by the `> ` prefix, with the
 * caret after the prefix (a quote block stores its prefix on every line).
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('$lib/design/components/base-node.svelte', async () => {
  return await import('../fixtures/fake-base-node-textarea.svelte');
});

import QuoteBlockNode from '$lib/design/components/quote-block-node.svelte';
import { insertQuoteLineBreak } from '$lib/types/quote-block-node';

async function shiftEnter(content: string, start: number, end = start) {
  const changes: string[] = [];
  const { getByTestId, component } = render(QuoteBlockNode, {
    props: { nodeId: 'q1', content },
    events: { contentChanged: (e: CustomEvent<{ content: string }>) => changes.push(e.detail.content) }
  } as never);
  void component;
  const ta = getByTestId('fake-textarea') as HTMLTextAreaElement;
  ta.value = content;
  ta.setSelectionRange(start, end);
  const notCancelled = await fireEvent.keyDown(ta, { key: 'Enter', shiftKey: true });
  await tick();
  return { ta, changes, notCancelled };
}

describe('QuoteBlockNode Shift+Enter', () => {
  afterEach(() => cleanup());

  it('inserts a newline and the prefix at the end of a single line, caret after prefix', async () => {
    const { ta, changes, notCancelled } = await shiftEnter('> hello', 7);
    expect(ta.value).toBe('> hello\n> ');
    expect(ta.selectionStart).toBe(10);
    expect(ta.selectionEnd).toBe(10);
    expect(changes).toEqual(['> hello\n> ']);
    expect(notCancelled).toBe(false); // default prevented: no second, bare newline
  });

  it('splits a line mid-text and prefixes the second half', async () => {
    const { ta } = await shiftEnter('> ab', 4 - 1);
    expect(ta.value).toBe('> a\n> b');
    expect(ta.selectionStart).toBe(6);
  });

  it('works on an empty-quote block ("> ")', async () => {
    const { ta } = await shiftEnter('> ', 2);
    expect(ta.value).toBe('> \n> ');
    expect(ta.selectionStart).toBe(5);
  });

  it('ignores plain Enter', async () => {
    const { getByTestId } = render(QuoteBlockNode, { props: { nodeId: 'q1', content: '> a' } });
    const ta = getByTestId('fake-textarea') as HTMLTextAreaElement;
    const notCancelled = await fireEvent.keyDown(ta, { key: 'Enter' });
    expect(notCancelled).toBe(true);
    expect(ta.value).toBe('> a');
  });
});

describe('QuoteBlockNode Shift+Enter guards', () => {
  afterEach(() => cleanup());

  it('ignores Shift+Enter during IME composition', async () => {
    const { getByTestId } = render(QuoteBlockNode, { props: { nodeId: 'q1', content: '> a' } });
    const ta = getByTestId('fake-textarea') as HTMLTextAreaElement;
    ta.setSelectionRange(3, 3);
    const notCancelled = await fireEvent.keyDown(ta, { key: 'Enter', shiftKey: true, isComposing: true });
    expect(notCancelled).toBe(true);
    expect(ta.value).toBe('> a');
  });

  it('ignores Shift+Enter already handled (defaultPrevented) upstream', async () => {
    const { getByTestId } = render(QuoteBlockNode, { props: { nodeId: 'q1', content: '> a' } });
    const ta = getByTestId('fake-textarea') as HTMLTextAreaElement;
    ta.setSelectionRange(3, 3);
    ta.addEventListener('keydown', (e) => e.preventDefault());
    await fireEvent.keyDown(ta, { key: 'Enter', shiftKey: true });
    expect(ta.value).toBe('> a');
  });
});

describe('insertQuoteLineBreak', () => {
  it('bare ">" line followed by another line: caret stays on its own line', () => {
    expect(insertQuoteLineBreak('>\n> b', 1, 1)).toEqual({ value: '>\n> \n> b', caret: 4 });
    expect(insertQuoteLineBreak('> a\n>\n> c', 5, 5)).toEqual({ value: '> a\n>\n> \n> c', caret: 8 });
  });
  it('bare ">" as the last line', () => {
    expect(insertQuoteLineBreak('> a\n>', 5, 5)).toEqual({ value: '> a\n>\n> ', caret: 8 });
  });
  it('replaces a selection', () => {
    expect(insertQuoteLineBreak('> abcd', 3, 5)).toEqual({ value: '> a\n> d', caret: 6 });
  });
  it('does not split the prefix: caret inside it moves past it', () => {
    expect(insertQuoteLineBreak('> a', 0, 0)).toEqual({ value: '> \n> a', caret: 5 });
    expect(insertQuoteLineBreak('> a', 1, 1)).toEqual({ value: '> \n> a', caret: 5 });
  });
  it('caret at 0 of a block whose first line is empty stays on that line', () => {
    expect(insertQuoteLineBreak('\n> a', 0, 0)).toEqual({ value: '\n> \n> a', caret: 3 });
  });
  it('handles a later line and a line lacking the prefix', () => {
    expect(insertQuoteLineBreak('> a\n> b', 7, 7)).toEqual({ value: '> a\n> b\n> ', caret: 10 });
    expect(insertQuoteLineBreak('plain', 5, 5)).toEqual({ value: 'plain\n> ', caret: 8 });
    expect(insertQuoteLineBreak('', 0, 0)).toEqual({ value: '\n> ', caret: 3 });
  });
});
