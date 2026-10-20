/**
 * Table node: detection, rendered display mode, focus/blur switch and round-trip.
 *
 * Mounts the real table-node.svelte with the real BaseNode. Display mode (not
 * editing) must render a <table> with inline formatting in cells; focusing the
 * node must swap to the markdown source in a textarea, and the content must stay
 * byte-identical through the switch.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import TableNode from '$lib/design/components/table-node.svelte';
import { focusManager } from '$lib/services/focus-manager.svelte';
import { pluginRegistry } from '$lib/plugins/plugin-registry';
import {
  isMarkdownTable,
  parseTable,
  splitTableRow,
  TABLE_DETECT_PATTERN
} from '$lib/design/components/table-markdown';

const TABLE = [
  '| Name | Notes | Score |',
  '| :--- | :---: | ---: |',
  '| **Ada** | uses `code` | [site](https://example.com) |',
  '| Bob | plain | 2 |'
].join('\n');

async function settle() {
  for (let i = 0; i < 10; i++) {
    await tick();
    await Promise.resolve();
  }
}

describe('table detection', () => {
  const detects = (content: string) =>
    pluginRegistry.detectPatternInContent(content)?.config.targetNodeType === 'table';

  it('detects the "| " shortcut', () => {
    expect(detects('| ')).toBe(true);
  });

  it('detects a full table with leading pipes and no space after the pipe', () => {
    expect(detects('|a|b|\n|---|---|\n|1|2|')).toBe(true);
  });

  it('detects a table without outer pipes', () => {
    expect(detects('a | b\n--- | ---\n1 | 2')).toBe(true);
  });

  it('does not detect plain text, a lone pipe, or a header without a delimiter row', () => {
    expect(detects('hello world')).toBe(false);
    expect(detects('|')).toBe(false);
    expect(detects('a | b\njust text')).toBe(false);
  });

  it('does not steal quotes or code fences that contain a table-looking line', () => {
    expect(detects('> a | b\n--- | ---')).toBe(false);
    expect(detects('```\na | b\n--- | ---\n```')).toBe(false);
  });

  it.each(['a | b\n---|', 'a | b\n-|', 'a | b\n:-|', 'a | b | c\n--- | ---'])(
    'does not detect a header and delimiter with different cell counts: %j',
    (content) => {
      expect(detects(content)).toBe(false);
    }
  );

  it('stays fast on a very long single line (runs on every keystroke)', () => {
    const start = performance.now();
    for (const line of ['a'.repeat(200_000), 'a|'.repeat(100_000), '|'.repeat(200_000)]) {
      detects(line);
      detects(line + '\n---');
    }
    expect(performance.now() - start).toBeLessThan(1000);
  });

  it('exports the pattern the plugin registers', () => {
    expect(TABLE_DETECT_PATTERN.test(TABLE)).toBe(true);
  });
});

describe('table parsing', () => {
  it('parses headers, alignments and rows', () => {
    const parsed = parseTable(TABLE);
    expect(parsed?.headers).toEqual(['Name', 'Notes', 'Score']);
    expect(parsed?.alignments).toEqual(['left', 'center', 'right']);
    expect(parsed?.rows).toHaveLength(2);
  });

  it('keeps pipes inside code spans and escaped pipes in the cell', () => {
    expect(splitTableRow('| `a|b` | x\\|y |')).toEqual(['`a|b`', 'x|y']);
  });

  it('rejects content without a delimiter row', () => {
    expect(isMarkdownTable('| a | b |\n| c | d |')).toBe(false);
    expect(parseTable('| ')).toBeNull();
  });
});

describe('TableNode display / edit switch', () => {
  afterEach(() => {
    cleanup();
    focusManager.clearEditing();
  });

  it('renders a table with inline formatting on blur (display mode)', async () => {
    const { container } = render(TableNode, { props: { nodeId: 'tbl-view', content: TABLE } });
    await settle();

    expect(container.querySelector('textarea')).toBeNull();
    expect(container.querySelector('table')).not.toBeNull();
    expect(container.querySelectorAll('th')).toHaveLength(3);
    expect(container.querySelectorAll('tbody tr')).toHaveLength(2);
    expect(container.querySelector('td .markdown-bold')?.textContent).toBe('Ada');
    expect(container.querySelector('td code')?.textContent).toBe('code');
    const link = container.querySelector('td a');
    expect(link?.getAttribute('href')).toBe('https://example.com');
    expect(link?.textContent).toBe('site');
    expect((container.querySelectorAll('th')[2] as HTMLElement).style.textAlign).toBe('right');
    expect(container.textContent).not.toContain('**');
    expect(container.textContent).not.toContain('|---');
  });

  it('shows the markdown source on focus and the table again on blur, content unchanged', async () => {
    const { container } = render(TableNode, { props: { nodeId: 'tbl-switch', content: TABLE } });
    await settle();
    expect(container.querySelector('table')).not.toBeNull();

    focusManager.focusNode('tbl-switch', 'pane-1');
    await settle();
    const textarea = container.querySelector('textarea') as HTMLTextAreaElement;
    expect(textarea).not.toBeNull();
    expect(textarea.value).toBe(TABLE);
    expect(container.querySelector('table')).toBeNull();

    focusManager.clearEditing();
    await settle();
    expect(container.querySelector('textarea')).toBeNull();
    expect(container.querySelector('table')).not.toBeNull();
    expect(container.querySelector('td .markdown-bold')?.textContent).toBe('Ada');
  });

  it('re-renders from edited source', async () => {
    const edited = TABLE.replace('| Bob |', '| Eve |');
    const { container, rerender } = render(TableNode, {
      props: { nodeId: 'tbl-edit', content: TABLE }
    });
    await settle();
    await rerender({ nodeId: 'tbl-edit', content: edited });
    await settle();

    const cells = Array.from(container.querySelectorAll('tbody tr:last-child td')).map(
      (td) => td.textContent
    );
    expect(cells[0]).toBe('Eve');
  });

  it('shows the source as plain text when the content is not yet a complete table', async () => {
    const { container } = render(TableNode, { props: { nodeId: 'tbl-partial', content: '| ' } });
    await settle();
    expect(container.querySelector('table')).toBeNull();
    expect(container.querySelector('.node__content--view')?.textContent).toContain('|');
  });
});
