/**
 * GFM table helpers shared by the table node and its pattern detection.
 *
 * The node stores the markdown source verbatim; these helpers only read it. Nothing
 * here rewrites content, so editing the source round-trips unchanged.
 */

export type TableAlignment = 'left' | 'center' | 'right';

export interface ParsedTable {
  headers: string[];
  alignments: TableAlignment[];
  rows: string[][];
}

/**
 * Split one table row into trimmed cells. Pipes inside a backtick code span and
 * backslash-escaped pipes (`\|`) belong to the cell and do not split it.
 */
export function splitTableRow(line: string): string[] {
  let text = line.trim();
  if (text.startsWith('|')) text = text.slice(1);
  if (text.endsWith('|') && !text.endsWith('\\|')) text = text.slice(0, -1);

  const cells: string[] = [];
  let current = '';
  let inCode = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === '\\' && text[i + 1] === '|') {
      current += '|';
      i++;
    } else if (ch === '`') {
      inCode = !inCode;
      current += ch;
    } else if (ch === '|' && !inCode) {
      cells.push(current.trim());
      current = '';
    } else {
      current += ch;
    }
  }
  cells.push(current.trim());
  return cells;
}

/** A delimiter row: every cell is dashes with optional leading/trailing colons. */
export function isDelimiterRow(line: string): boolean {
  if (!line.includes('|') || !line.includes('-')) return false;
  const cells = splitTableRow(line);
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell));
}

/** True when the content starts with a GFM header row followed by a delimiter row. */
export function isMarkdownTable(raw: string): boolean {
  const lines = raw.split('\n').filter((l) => l.trim().length > 0);
  if (lines.length < 2 || !lines[0].includes('|')) return false;
  return isDelimiterRow(lines[1]) && splitTableRow(lines[0]).length === splitTableRow(lines[1]).length;
}

/** Parse a GFM markdown table, or return null when the content is not one. */
export function parseTable(raw: string): ParsedTable | null {
  if (!isMarkdownTable(raw)) return null;
  const lines = raw.split('\n').filter((l) => l.trim().length > 0);

  const headers = splitTableRow(lines[0]);
  const alignments: TableAlignment[] = splitTableRow(lines[1]).map((cell) => {
    if (cell.startsWith(':') && cell.endsWith(':')) return 'center';
    if (cell.endsWith(':')) return 'right';
    return 'left';
  });
  const rows = lines.slice(2).map(splitTableRow);

  return { headers, alignments, rows };
}

/**
 * Pattern for the table plugin's `detect`: either the `| ` shortcut typed at the start
 * of a node, or a complete GFM header plus delimiter row (e.g. a pasted `a | b` table
 * or `|a|b|` with no space after the pipe). The header line may not look like another
 * block (quote, heading, fence, list item).
 */
export const TABLE_DETECT_PATTERN =
  /^(?:\|[ \t]|(?![>#`]|[-*+][ \t]|\d+\.[ \t])[^\n]*\|[^\n]*\n[ \t]*(?=[^\n]*\|)\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*(?:\n|$))/;
